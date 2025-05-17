//! Indexing of packages in a output folder to create up to date repodata.json
//! files
#![deny(missing_docs)]

use anyhow::{Context, Result};
use bytes::buf::Buf;
use fs_err::{self as fs};
use futures::{stream::FuturesUnordered, StreamExt};
use fxhash::FxHashMap;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rattler_conda_types::{
    package::{ArchiveIdentifier, ArchiveType, IndexJson, PackageFile, RunExportsJson},
    ChannelInfo, PackageRecord, PatchInstructions, Platform, RepoData, Shard, ShardedRepodata,
    ShardedSubdirInfo,
};
use rattler_digest::Sha256Hash;
use rattler_networking::{Authentication, AuthenticationStorage};
use rattler_package_streaming::{
    read,
    seek::{self, stream_conda_content},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read, Seek},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};
use tokio::sync::Semaphore;
use url::Url;

use opendal::{
    layers::RetryLayer,
    services::{FsConfig, S3Config},
    Configurator, Operator,
};

const REPODATA_FROM_PACKAGES: &str = "repodata_from_packages.json";
const REPODATA: &str = "repodata.json";
const REPODATA_SHARDS: &str = "repodata_shards.msgpack.zst";

/// Extract the package record from an `index.json` file.
pub fn package_record_from_index_json<T: Read>(
    package_as_bytes: impl AsRef<[u8]>,
    index_json_reader: &mut T,
) -> std::io::Result<PackageRecord> {
    let index = IndexJson::from_reader(index_json_reader)?;

    let sha256_result =
        rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(&package_as_bytes);
    let md5_result = rattler_digest::compute_bytes_digest::<rattler_digest::Md5>(&package_as_bytes);
    let size = package_as_bytes.as_ref().len();

    let package_record = PackageRecord {
        name: index.name,
        version: index.version,
        build: index.build,
        build_number: index.build_number,
        subdir: index.subdir.unwrap_or_else(|| "unknown".to_string()),
        md5: Some(md5_result),
        sha256: Some(sha256_result),
        size: Some(size as u64),
        arch: index.arch,
        platform: index.platform,
        depends: index.depends,
        extra_depends: std::collections::BTreeMap::new(),
        constrains: index.constrains,
        track_features: index.track_features,
        features: index.features,
        noarch: index.noarch,
        license: index.license,
        license_family: index.license_family,
        timestamp: index.timestamp,
        python_site_packages_path: index.python_site_packages_path,
        legacy_bz2_md5: None,
        legacy_bz2_size: None,
        purls: index.purls,
        run_exports: None,
    };

    Ok(package_record)
}

fn repodata_patch_from_conda_package_stream<'a>(
    package: impl Read + Seek + 'a,
) -> anyhow::Result<rattler_conda_types::RepoDataPatch> {
    let mut subdirs = FxHashMap::default();

    let mut content_reader = stream_conda_content(package)?;
    let entries = content_reader.entries()?;
    for entry in entries {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err(anyhow::anyhow!(
                "Expected repodata patch package to be a file"
            ));
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        let path = entry.path()?;
        let components = path.components().collect::<Vec<_>>();
        let subdir =
            if components.len() == 2 && components[1].as_os_str() == "patch_instructions.json" {
                let subdir_str = components[0]
                    .as_os_str()
                    .to_str()
                    .context("Could not convert OsStr to str")?;
                let _ = Platform::from_str(subdir_str)?;
                subdir_str.to_string()
            } else {
                return Err(anyhow::anyhow!(
                    "Expected files of form <subdir>/patch_instructions.json, but found {}",
                    path.display()
                ));
            };

        let instructions: PatchInstructions = serde_json::from_slice(&buf)?;
        subdirs.insert(subdir, instructions);
    }

    Ok(rattler_conda_types::RepoDataPatch { subdirs })
}

/// Extract the package record from a `.tar.bz2` package file.
/// This function will look for the `info/index.json` file in the conda package
/// and extract the package record from it.
pub fn package_record_from_tar_bz2(file: &Path) -> std::io::Result<PackageRecord> {
    let reader = fs::File::open(file)?;
    package_record_from_tar_bz2_reader(reader)
}

/// Extract the package record from a `.tar.bz2` package file.
/// This function will look for the `info/index.json` file in the conda package
/// and extract the package record from it.
pub fn package_record_from_tar_bz2_reader(reader: impl Read) -> std::io::Result<PackageRecord> {
    let bytes = reader.bytes().collect::<Result<Vec<u8>, _>>()?;
    let reader = Cursor::new(&bytes);
    let mut archive = read::stream_tar_bz2(reader);
    for entry in archive.entries()?.flatten() {
        let mut entry = entry;
        let path = entry.path()?;
        if path.as_os_str().eq("info/index.json") {
            return package_record_from_index_json(&bytes, &mut entry);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "No index.json found",
    ))
}

/// Extract the package record from a `.conda` package file.
/// This function will look for the `info/index.json` file in the conda package
/// and extract the package record from it.
pub fn package_record_from_conda(file: &Path) -> std::io::Result<PackageRecord> {
    let reader = fs::File::open(file)?;
    package_record_from_conda_reader(reader)
}

fn read_index_json_from_archive(
    bytes: &Vec<u8>,
    archive: &mut tar::Archive<impl Read>,
) -> std::io::Result<PackageRecord> {
    let mut index_json = None;
    let mut run_exports_json = None;
    for entry in archive.entries()?.flatten() {
        let mut entry = entry;
        let path = entry.path()?;
        if path.as_os_str().eq("info/index.json") {
            index_json = Some(package_record_from_index_json(bytes, &mut entry)?);
        } else if path.as_os_str().eq("info/run_exports.json") {
            run_exports_json = Some(RunExportsJson::from_reader(&mut entry)?);
        }
    }

    if let Some(mut index_json) = index_json {
        index_json.run_exports = run_exports_json;
        return Ok(index_json);
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "No index.json found",
    ))
}

/// Extract the package record from a `.conda` package file content.
/// This function will look for the `info/index.json` file in the conda package
/// and extract the package record from it.
pub fn package_record_from_conda_reader(reader: impl Read) -> std::io::Result<PackageRecord> {
    let bytes = reader.bytes().collect::<Result<Vec<u8>, _>>()?;
    let reader = Cursor::new(&bytes);
    let mut archive = seek::stream_conda_info(reader).expect("Could not open conda file");
    read_index_json_from_archive(&bytes, &mut archive)
}

#[allow(clippy::too_many_arguments)]
async fn index_subdir(
    subdir: Platform,
    op: Operator,
    force: bool,
    write_zst: bool,
    write_shards: bool,
    repodata_patch: Option<PatchInstructions>,
    progress: Option<MultiProgress>,
    semaphore: Arc<Semaphore>,
) -> Result<()> {
    let mut registered_packages: FxHashMap<String, PackageRecord> = HashMap::default();
    if !force {
        let repodata_bytes = if repodata_patch.is_some() {
            op.read(&format!("{subdir}/{REPODATA_FROM_PACKAGES}")).await
        } else {
            op.read(&format!("{subdir}/{REPODATA}")).await
        };
        let repodata: RepoData = match repodata_bytes {
            Ok(bytes) => serde_json::from_slice(&bytes.to_vec())?,
            Err(e) => {
                if e.kind() != opendal::ErrorKind::NotFound {
                    return Err(e.into());
                }
                tracing::info!("Could not find repodata.json. Creating new one.");
                RepoData {
                    info: Some(ChannelInfo {
                        subdir: Some(subdir.to_string()),
                        base_url: None,
                    }),
                    packages: HashMap::default(),
                    conda_packages: HashMap::default(),
                    removed: HashSet::default(),
                    version: Some(2),
                }
            }
        };
        registered_packages.extend(repodata.packages.into_iter());
        registered_packages.extend(repodata.conda_packages.into_iter());
        tracing::debug!(
            "Found {} already registered packages in {}/repodata.json.",
            registered_packages.len(),
            subdir
        );
    }
    let uploaded_packages: HashSet<String> = op
        .list_with(&format!("{}/", subdir.as_str()))
        .await?
        .iter()
        .filter_map(|entry| {
            if entry.metadata().mode().is_file() {
                let filename = entry.name().to_string();
                // Check if the file is an archive package file.
                ArchiveType::try_from(&filename).map(|_| filename)
            } else {
                None
            }
        })
        .collect();

    tracing::debug!(
        "Found {} already uploaded packages in subdir {}.",
        uploaded_packages.len(),
        subdir
    );

    let packages_to_delete = registered_packages
        .keys()
        .cloned()
        .collect::<HashSet<_>>()
        .difference(&uploaded_packages)
        .cloned()
        .collect::<Vec<_>>();

    tracing::debug!(
        "Deleting {} packages from subdir {}.",
        packages_to_delete.len(),
        subdir
    );

    for filename in packages_to_delete {
        registered_packages.remove(&filename);
    }

    let packages_to_add = uploaded_packages
        .difference(&registered_packages.keys().cloned().collect::<HashSet<_>>())
        .cloned()
        .collect::<Vec<_>>();

    tracing::info!(
        "Adding {} packages to subdir {}.",
        packages_to_add.len(),
        subdir
    );

    let pb = if let Some(progress) = progress {
        progress.add(ProgressBar::new(packages_to_add.len() as u64))
    } else {
        ProgressBar::hidden()
    };

    let sty = ProgressStyle::with_template(
        "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
    )
    .unwrap()
    .progress_chars("##-");
    pb.set_style(sty);

    let mut tasks = FuturesUnordered::new();
    for filename in packages_to_add.iter() {
        let task = {
            let op = op.clone();
            let filename = filename.clone();
            let pb = pb.clone();
            let semaphore = semaphore.clone();
            {
                async move {
                    let _permit = semaphore
                        .acquire()
                        .await
                        .expect("Semaphore was unexpectedly closed");
                    pb.set_message(format!(
                        "Indexing {} {}",
                        subdir.as_str(),
                        console::style(filename.clone()).dim()
                    ));
                    let file_path = format!("{subdir}/{filename}");
                    let buffer = op.read(&file_path).await?;
                    let reader = buffer.reader();
                    // We already know it's not None
                    let archive_type = ArchiveType::try_from(&filename).unwrap();
                    let record = match archive_type {
                        ArchiveType::TarBz2 => package_record_from_tar_bz2_reader(reader),
                        ArchiveType::Conda => package_record_from_conda_reader(reader),
                    }?;
                    pb.inc(1);
                    Ok::<(String, PackageRecord), std::io::Error>((filename.clone(), record))
                }
            }
        };
        tasks.push(tokio::spawn(task));
    }
    let mut results = Vec::new();
    while let Some(join_result) = tasks.next().await {
        match join_result {
            Ok(Ok(result)) => results.push(result),
            Ok(Err(e)) => {
                tasks.clear();
                tracing::error!("Failed to process package: {}", e);
                pb.abandon_with_message(format!(
                    "{} {}",
                    console::style("Failed to index").red(),
                    console::style(subdir.as_str()).dim()
                ));
                return Err(e.into());
            }
            Err(join_err) => {
                tasks.clear();
                tracing::error!("Task panicked: {}", join_err);
                pb.abandon_with_message(format!(
                    "{} {}",
                    console::style("Failed to index").red(),
                    console::style(subdir.as_str()).dim()
                ));
                return Err(anyhow::anyhow!("Task panicked: {}", join_err));
            }
        }
    }
    pb.finish_with_message(format!(
        "{} {}",
        console::style("Finished").green(),
        subdir.as_str()
    ));

    tracing::info!(
        "Successfully added {} packages to subdir {}.",
        results.len(),
        subdir
    );

    for (filename, record) in results {
        registered_packages.insert(filename, record);
    }

    let mut packages: FxHashMap<String, PackageRecord> = HashMap::default();
    let mut conda_packages: FxHashMap<String, PackageRecord> = HashMap::default();
    for (filename, package) in registered_packages {
        match ArchiveType::try_from(&filename) {
            Some(ArchiveType::TarBz2) => {
                packages.insert(filename, package);
            }
            Some(ArchiveType::Conda) => {
                conda_packages.insert(filename, package);
            }
            _ => panic!("Unknown archive type"),
        }
    }

    // TODO: don't serialize run_exports and purls but in their own files
    let repodata = RepoData {
        info: Some(ChannelInfo {
            subdir: Some(subdir.to_string()),
            base_url: None,
        }),
        packages,
        conda_packages,
        removed: HashSet::default(),
        version: Some(2),
    };

    write_repodata(
        repodata,
        repodata_patch,
        write_zst,
        write_shards,
        subdir,
        op,
    )
    .await
}

fn serialize_msgpack_zst<T>(val: &T) -> Result<Vec<u8>>
where
    T: Serialize + ?Sized,
{
    let msgpack = rmp_serde::to_vec_named(val)?;
    let encoded = zstd::stream::encode_all(&msgpack[..], 0)?;
    Ok(encoded)
}

/// Write a `repodata.json` for all packages in the given configurator's root.
pub async fn write_repodata(
    repodata: RepoData,
    repodata_patch: Option<PatchInstructions>,
    write_zst: bool,
    write_shards: bool,
    subdir: Platform,
    op: Operator,
) -> Result<()> {
    if repodata_patch.is_some() {
        let unpatched_repodata_path = format!("{subdir}/{REPODATA_FROM_PACKAGES}");
        tracing::info!("Writing unpatched repodata to {unpatched_repodata_path}");
        let unpatched_repodata_bytes = serde_json::to_vec(&repodata)?;
        op.write(&unpatched_repodata_path, unpatched_repodata_bytes)
            .await?;
    }

    let repodata = if let Some(instructions) = repodata_patch {
        tracing::info!("Patching repodata");
        let mut patched_repodata = repodata.clone();
        patched_repodata.apply_patches(&instructions);
        patched_repodata
    } else {
        repodata
    };

    let repodata_bytes = serde_json::to_vec(&repodata)?;
    if write_zst {
        tracing::info!("Compressing repodata bytes");
        let repodata_zst_bytes = zstd::stream::encode_all(&repodata_bytes[..], 19)?;
        let repodata_zst_path = format!("{subdir}/{REPODATA}.zst");
        tracing::info!("Writing zst repodata to {repodata_zst_path}");
        op.write(&repodata_zst_path, repodata_zst_bytes).await?;
    }

    let repodata_path = format!("{subdir}/{REPODATA}");
    tracing::info!("Writing repodata to {repodata_path}");
    op.write(&repodata_path, repodata_bytes).await?;

    if write_shards {
        // See CEP 16 <https://github.com/conda/ceps/blob/main/cep-0016.md>
        tracing::info!("Creating sharded repodata");
        let mut shards_by_package_names: HashMap<String, Shard> = HashMap::new();
        for (k, package_record) in repodata.conda_packages {
            let package_name = package_record.name.as_normalized();
            let shard = shards_by_package_names
                .entry(package_name.into())
                .or_default();
            shard.conda_packages.insert(k, package_record);
        }
        for (k, package_record) in repodata.packages {
            let package_name = package_record.name.as_normalized();
            let shard = shards_by_package_names
                .entry(package_name.into())
                .or_default();
            shard.packages.insert(k, package_record);
        }
        for package in repodata.removed {
            let package_name = ArchiveIdentifier::try_from_filename(package.as_str())
                .context("Could not determine archive identifier for {package}")?
                .name;
            let shard = shards_by_package_names.entry(package_name).or_default();
            shard.removed.insert(package);
        }

        // calculate digests for shards
        let shards = shards_by_package_names
            .iter()
            .map(|(k, shard)| {
                serialize_msgpack_zst(shard).map(|encoded| {
                    let mut hasher = Sha256::new();
                    hasher.update(&encoded);
                    let digest: Sha256Hash = hasher.finalize();
                    (k, (digest, encoded))
                })
            })
            .collect::<Result<HashMap<_, _>>>()?;

        let sharded_repodata = ShardedRepodata {
            info: ShardedSubdirInfo {
                subdir,
                base_url: "".into(),
                shards_base_url: "./shards/".into(),
            },
            shards: shards
                .iter()
                .map(|(&k, (digest, _))| (k.clone(), *digest))
                .collect(),
        };

        let mut tasks = FuturesUnordered::new();
        // todo max parallel
        for (_, (digest, encoded_shard)) in shards {
            let op = op.clone();
            let future = async move || {
                let shard_path = format!("{subdir}/shards/{digest:x}.msgpack.zst");
                tracing::trace!("Writing repodata shard to {shard_path}");
                op.write(&shard_path, encoded_shard).await
            };
            tasks.push(tokio::spawn(future()));
        }
        while let Some(join_result) = tasks.next().await {
            match join_result {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => Err(e)?,
                Err(join_err) => Err(join_err)?,
            }
        }

        let repodata_shards_path = format!("{subdir}/{REPODATA_SHARDS}");
        tracing::trace!("Writing repodata shards to {repodata_shards_path}");
        let sharded_repodata_encoded = serialize_msgpack_zst(&sharded_repodata)?;
        op.write(&repodata_shards_path, sharded_repodata_encoded)
            .await?;
    }
    Ok(())
}

/// Create a new `repodata.json` for all packages in the channel at the given directory.
#[allow(clippy::too_many_arguments)]
pub async fn index_fs(
    channel: impl Into<PathBuf>,
    target_platform: Option<Platform>,
    repodata_patch: Option<String>,
    write_zst: bool,
    write_shards: bool,
    force: bool,
    max_parallel: usize,
    multi_progress: Option<MultiProgress>,
) -> anyhow::Result<()> {
    let mut config = FsConfig::default();
    config.root = Some(channel.into().canonicalize()?.to_string_lossy().to_string());
    index(
        target_platform,
        config,
        repodata_patch,
        write_zst,
        write_shards,
        force,
        max_parallel,
        multi_progress,
    )
    .await
}

/// Create a new `repodata.json` for all packages in the channel at the given S3 URL.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub async fn index_s3(
    channel: Url,
    region: String,
    endpoint_url: Url,
    force_path_style: bool,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    session_token: Option<String>,
    target_platform: Option<Platform>,
    repodata_patch: Option<String>,
    write_zst: bool,
    write_shards: bool,
    force: bool,
    max_parallel: usize,
    multi_progress: Option<MultiProgress>,
) -> anyhow::Result<()> {
    let mut s3_config = S3Config::default();
    s3_config.root = Some(channel.path().to_string());
    s3_config.bucket = channel
        .host_str()
        .ok_or(anyhow::anyhow!("No bucket in S3 URL"))?
        .to_string();
    s3_config.region = Some(region);
    s3_config.endpoint = Some(endpoint_url.to_string());
    s3_config.enable_virtual_host_style = !force_path_style;
    // Use credentials from the CLI if they are provided.
    if let (Some(access_key_id), Some(secret_access_key)) = (access_key_id, secret_access_key) {
        s3_config.secret_access_key = Some(secret_access_key);
        s3_config.access_key_id = Some(access_key_id);
        s3_config.session_token = session_token;
    } else {
        // If they're not provided, check rattler authentication storage for credentials.
        let auth_storage = AuthenticationStorage::from_env_and_defaults()?;
        let auth = auth_storage.get_by_url(channel)?;
        if let (
            _,
            Some(Authentication::S3Credentials {
                access_key_id,
                secret_access_key,
                session_token,
            }),
        ) = auth
        {
            s3_config.access_key_id = Some(access_key_id);
            s3_config.secret_access_key = Some(secret_access_key);
            s3_config.session_token = session_token;
        }
    }
    index(
        target_platform,
        s3_config,
        repodata_patch,
        write_zst,
        write_shards,
        force,
        max_parallel,
        multi_progress,
    )
    .await
}

/// Create a new `repodata.json` for all packages in the given configurator's root.
/// If `target_platform` is `Some`, only that specific subdir is indexed.
/// Otherwise indexes all subdirs and creates a `repodata.json` for each.
///
/// The process is the following:
/// 1. Get all subdirs and create `noarch` and `target_platform` if they do not exist.
/// 2. Iterate subdirs and index each subdir.
///    Therefore, we need to:
///    1. Collect all uploaded packages in subdir
///    2. Collect all registered packages from `repodata.json` (if exists)
///    3. Determine which packages to add to and to delete from `repodata.json`
///    4. Write `repodata.json` back
#[allow(clippy::too_many_arguments)]
pub async fn index<T: Configurator>(
    target_platform: Option<Platform>,
    config: T,
    repodata_patch: Option<String>,
    write_zst: bool,
    write_shards: bool,
    force: bool,
    max_parallel: usize,
    multi_progress: Option<MultiProgress>,
) -> anyhow::Result<()> {
    let builder = config.into_builder();

    // Get all subdirs
    let op = Operator::new(builder)?.layer(RetryLayer::new()).finish();
    let entries = op.list_with("").await?;

    // If requested `target_platform` subdir does not exist, we create it.
    let mut subdirs = if let Some(target_platform) = target_platform {
        if !op.exists(&format!("{}/", target_platform.as_str())).await? {
            tracing::debug!("Did not find {target_platform} subdir, creating.");
            op.create_dir(&format!("{}/", target_platform.as_str()))
                .await?;
        }
        // Limit subdirs to only the requested `target_platform`.
        HashSet::from([target_platform])
    } else {
        entries
            .iter()
            .filter_map(|entry| {
                if entry.metadata().mode().is_dir() && entry.name() != "/" {
                    // Directory entries always end with `/`.
                    Some(entry.name().trim_end_matches('/').to_string())
                } else {
                    None
                }
            })
            .filter_map(|s| Platform::from_str(&s).ok())
            .collect::<HashSet<_>>()
    };

    if !op
        .exists(&format!("{}/", Platform::NoArch.as_str()))
        .await?
    {
        // If `noarch` subdir does not exist, we create it.
        tracing::debug!("Did not find noarch subdir, creating.");
        op.create_dir(&format!("{}/", Platform::NoArch.as_str()))
            .await?;
        subdirs.insert(Platform::NoArch);
    }

    let repodata_patch = if let Some(path) = repodata_patch {
        match ArchiveType::try_from(path.clone()) {
            Some(ArchiveType::Conda) => {}
            Some(ArchiveType::TarBz2) | None => {
                return Err(anyhow::anyhow!(
                    "Only .conda packages are supported for repodata patches. Got: {}",
                    path
                ))
            }
        }
        let repodata_patch_path = format!("noarch/{path}");
        let repodata_patch_bytes = op.read(&repodata_patch_path).await?.to_bytes();
        let reader = Cursor::new(repodata_patch_bytes);
        let repodata_patch = repodata_patch_from_conda_package_stream(reader)?;
        Some(repodata_patch)
    } else {
        None
    };

    let semaphore = Semaphore::new(max_parallel);
    let semaphore = Arc::new(semaphore);

    let mut tasks = FuturesUnordered::new();
    for subdir in subdirs.iter() {
        let task = index_subdir(
            *subdir,
            op.clone(),
            force,
            write_zst,
            write_shards,
            repodata_patch
                .as_ref()
                .and_then(|p| p.subdirs.get(&subdir.to_string()).cloned()),
            multi_progress.clone(),
            semaphore.clone(),
        );
        tasks.push(tokio::spawn(task));
    }

    while let Some(join_result) = tasks.next().await {
        match join_result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                tracing::error!("Failed to process subdir: {}", e);
                tasks.clear();
                return Err(e);
            }
            Err(join_err) => {
                tracing::error!("Task panicked: {}", join_err);
                tasks.clear();
                return Err(anyhow::anyhow!("Task panicked: {}", join_err));
            }
        }
    }
    Ok(())
}
