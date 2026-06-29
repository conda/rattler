use crate::metadata::{CustomPrefixPlaceholder, FSMetadata};
use anyhow::anyhow;
use rattler_cache::package_cache::{CacheKey, PackageCache};
use rattler_conda_types::{Platform, package::PathsJson};
use rattler_lock::{LockFile, LockedPackage, UrlOrPath};
use rattler_networking::LazyClient;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

pub mod backends;
pub mod codesign;
pub mod metadata;
pub mod mount;
pub mod prefix_replacement;
pub mod tests;
pub mod virtual_fs_core;

pub use mount::{MountBackend, MountSession};

pub async fn mount_environment(
    pixi_lock: PathBuf,
    cache_origin: PathBuf,
    mount_dir: PathBuf,
    backend: MountBackend,
    environment_name: String,
    download_if_missing: bool,
) -> anyhow::Result<Box<dyn MountSession>> {
    let package_refs = solve_environment(&pixi_lock, &environment_name)?;

    let package_dirs = resolve_package_dirs(package_refs, cache_origin, download_if_missing)?;

    let mut metadata = vec![FSMetadata::new_directory(PathBuf::from("."), 0)];
    let mut directory_indices = HashMap::new();
    directory_indices.insert(PathBuf::from("."), 0);

    for package_dir in package_dirs {
        let paths_json = get_paths_json(&package_dir)?;
        path_parse(
            paths_json,
            package_dir,
            &mut metadata,
            &mut directory_indices,
            &mount_dir,
        )?;
    }

    backends::generate_mount(backend, metadata, mount_dir).await
}

fn resolve_package_dirs(
    package_refs: Vec<LockedPackage>,
    cache_origin: PathBuf,
    download_if_missing: bool,
) -> anyhow::Result<Vec<PathBuf>> {
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                rt.block_on(async move {
                    let package_cache = PackageCache::new(&cache_origin);
                    let client = LazyClient::default();
                    let mut dirs = Vec::with_capacity(package_refs.len());
                    for package_ref in &package_refs {
                        dirs.push(
                            ensure_package_cached(
                                package_ref,
                                &package_cache,
                                download_if_missing,
                                &client,
                            )
                            .await?,
                        );
                    }
                    anyhow::Ok(dirs)
                })
            })
            .join()
            .map_err(|_| anyhow!("package cache resolution thread panicked"))?
    })
}

async fn ensure_package_cached(
    package_ref: &LockedPackage,
    package_cache: &PackageCache,
    download_if_missing: bool,
    client: &LazyClient,
) -> anyhow::Result<PathBuf> {
    let package_data = package_ref
        .as_binary_conda()
        .ok_or_else(|| anyhow!("only binary conda packages can be mounted"))?;
    let cache_key = CacheKey::from(&package_data.package_record);

    if download_if_missing {
        let url = match &package_data.location {
            UrlOrPath::Url(u) => u.clone(),
            UrlOrPath::Path(p) => {
                return Err(anyhow!(
                    "package '{}' lockfile location is a local path ({}); cannot download",
                    package_ref.name(),
                    p
                ));
            }
        };

        println!("Resolving {} from {}", package_ref.name(), url);
        let metadata = package_cache
            .get_or_fetch_from_url(cache_key, url, client.clone(), None, None)
            .await
            .map_err(|e| anyhow!("failed to fetch package '{}': {e}", package_ref.name()))?;
        Ok(metadata.path().to_path_buf())
    } else {
        let pkg_name = package_ref.name().to_string();
        let metadata = package_cache
            .get_or_fetch(
                cache_key,
                move |_| {
                    let pkg_name = pkg_name.clone();
                    async move {
                        Err::<(), _>(std::io::Error::other(format!(
                            "package '{pkg_name}' is not in the cache; pass --DOWNLOAD to fetch it"
                        )))
                    }
                },
                None,
            )
            .await
            .map_err(|e| {
                anyhow!(
                    "package '{}' could not be loaded from the cache: {e}",
                    package_ref.name()
                )
            })?;
        Ok(metadata.path().to_path_buf())
    }
}

pub fn solve_environment(
    pixi_lock: &Path,
    environment_name: &str,
) -> anyhow::Result<Vec<LockedPackage>> {
    let lockfile = LockFile::from_path(pixi_lock)?;

    let env = lockfile
        .environment(environment_name)
        .ok_or_else(|| anyhow!("environment not found"))?;

    let platform_name = Platform::current().to_string();
    let platform = lockfile
        .platform(&platform_name)
        .ok_or_else(|| anyhow!("lockfile does not contain platform {platform_name}"))?;

    let packages = env
        .packages(platform)
        .ok_or_else(|| anyhow!("environment does not contain packages for current platform"))?;

    Ok(packages.cloned().collect())
}

pub fn get_paths_json(package_dir: &Path) -> anyhow::Result<PathsJson> {
    Ok(PathsJson::from_package_directory_with_deprecated_fallback(
        package_dir,
    )?)
}

pub fn path_parse(
    paths_json: PathsJson,
    package_dir: PathBuf,
    env_paths: &mut Vec<FSMetadata>,
    directory_indices: &mut HashMap<PathBuf, usize>,
    _mount_point: &Path,
) -> anyhow::Result<()> {
    for path in &paths_json.paths {
        let cache_base: Arc<Path> = package_dir.clone().into();
        let parent_directory = path.relative_path.parent().unwrap_or(Path::new("."));

        let mut parent_index = 0;
        for component in parent_directory.components() {
            let current_path = env_paths[parent_index]
                .as_directory()
                .expect("first element is always the root directory")
                .prefix_path
                .join(component);

            parent_index = match directory_indices.get(&current_path) {
                Some(&index) => index,
                None => {
                    let new_dir = FSMetadata::new_directory(current_path.clone(), parent_index);
                    let child_index = env_paths.len();
                    env_paths.push(new_dir);
                    env_paths[parent_index]
                        .as_directory_mut()
                        .expect("parent is a directory")
                        .children
                        .push(child_index);
                    directory_indices.insert(current_path, child_index);
                    child_index
                }
            };
        }

        let file_name = path
            .relative_path
            .file_name()
            .expect("files always have names");
        let file_path = (*cache_base).join(&path.relative_path);

        let prefix_placeholder = path.prefix_placeholder.clone().map(|pp| {
            let source_bytes = std::fs::read(&file_path).unwrap_or_default();
            CustomPrefixPlaceholder::from_placeholder(pp, &source_bytes)
        });

        let file_index = env_paths.len();
        env_paths.push(FSMetadata::new_file(
            file_name.into(),
            parent_index,
            cache_base,
            path.path_type.clone(),
            prefix_placeholder,
        ));

        env_paths[parent_index]
            .as_directory_mut()
            .expect("parents are always directories")
            .children
            .push(file_index);
    }

    Ok(())
}
