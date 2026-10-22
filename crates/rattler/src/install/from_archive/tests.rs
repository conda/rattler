//! Tests that linking packages from their archives results in the same prefix as the regular
//! installer.

use std::{
    collections::BTreeMap,
    env::temp_dir,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use rattler_cache::{
    archive_cache::{ArchiveCache, ArchiveHash},
    package_cache::PackageCache,
};
use rattler_conda_types::{
    ExplicitEnvironmentSpec, PackageArchiveHash, PrefixRecord, RepoDataRecord, Subdir,
    prefix_record,
};
use rattler_digest::{Sha256, Sha256Hash};

use super::{ArchiveLinkOptions, ArchivePackage, FromArchiveError, LinkPlan, PrefixSink};
use crate::{
    get_repodata_record, get_test_data_dir,
    install::{ClobberMode, Installer},
};

/// The prefix that both installers patch into the files, so that the results are comparable.
const TARGET_PREFIX: &str = "/opt/from-archive-test";

/// Writes the files to a directory on disk.
struct DirectorySink {
    root: PathBuf,
}

impl DirectorySink {
    fn destination(&self, path: &Path) -> io::Result<PathBuf> {
        assert!(path.is_relative(), "{} is not relative", path.display());
        let destination = self.root.join(path);
        fs::create_dir_all(destination.parent().unwrap())?;
        Ok(destination)
    }
}

impl PrefixSink for DirectorySink {
    fn add_file(
        &mut self,
        path: &Path,
        mode: u32,
        size: u64,
        contents: &mut dyn Read,
    ) -> io::Result<()> {
        let destination = self.destination(path)?;
        let mut file = fs::File::create(&destination)?;
        let written = io::copy(contents, &mut file)?;
        assert_eq!(written, size, "wrong size for {}", path.display());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&destination, fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        Ok(())
    }

    fn add_symlink(&mut self, path: &Path, target: &Path) -> io::Result<()> {
        let destination = self.destination(path)?;
        #[cfg(unix)]
        return std::os::unix::fs::symlink(target, destination);
        #[cfg(windows)]
        return std::os::windows::fs::symlink_file(target, destination);
    }

    fn add_directory(&mut self, path: &Path, _mode: u32) -> io::Result<()> {
        fs::create_dir_all(self.root.join(path))
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Node {
    File {
        sha256: Sha256Hash,
        executable: bool,
    },
    Symlink(PathBuf),
    EmptyDirectory,
}

/// Everything in the prefix except for `conda-meta` and the `CACHEDIR.TAG` the regular installer
/// adds.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Node> {
    let mut nodes = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let mut is_empty = true;
        for entry in fs::read_dir(&dir).unwrap() {
            is_empty = false;
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if relative == Path::new("conda-meta") || relative == Path::new("CACHEDIR.TAG") {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_symlink() {
                nodes.insert(relative, Node::Symlink(fs::read_link(&path).unwrap()));
            } else if metadata.is_dir() {
                pending.push(path);
            } else {
                #[cfg(unix)]
                let executable = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let executable = false;
                nodes.insert(
                    relative,
                    Node::File {
                        sha256: rattler_digest::compute_file_digest::<Sha256>(&path).unwrap(),
                        executable,
                    },
                );
            }
        }
        if is_empty && dir != root {
            nodes.insert(
                dir.strip_prefix(root).unwrap().to_path_buf(),
                Node::EmptyDirectory,
            );
        }
    }
    nodes
}

/// The paths recorded in `conda-meta`, by record file name.
fn conda_meta(root: &Path) -> BTreeMap<String, Vec<prefix_record::PathsEntry>> {
    PrefixRecord::collect_from_prefix::<PrefixRecord>(root)
        .unwrap()
        .into_iter()
        .map(|record| {
            let file_name = record.file_name();
            let mut paths = record.paths_data.paths;
            // The regular installer does not sort the paths again after moving clobbered files.
            paths.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
            (file_name, paths)
        })
        .collect()
}

async fn install_regular(records: &[RepoDataRecord], platform: Subdir) -> tempfile::TempDir {
    let prefix = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    Installer::new()
        .with_package_cache(PackageCache::new(cache.path()))
        .with_target_platform(platform)
        .with_alternative_target_prefix(TARGET_PREFIX)
        .with_execute_link_scripts(false)
        .install(prefix.path(), records.iter().cloned())
        .await
        .unwrap();
    prefix
}

fn plan(
    records: &[RepoDataRecord],
    archives: &[PathBuf],
    platform: Subdir,
) -> Result<LinkPlan, FromArchiveError> {
    let packages = records
        .iter()
        .zip(archives)
        .map(|(record, archive)| Ok((record.clone(), ArchivePackage::from_path(archive)?)))
        .collect::<Result<Vec<_>, FromArchiveError>>()?;
    LinkPlan::new(packages, ArchiveLinkOptions::new(TARGET_PREFIX, platform))
}

fn install_from_archive(
    records: &[RepoDataRecord],
    archives: &[PathBuf],
    platform: Subdir,
) -> tempfile::TempDir {
    let prefix = tempfile::tempdir().unwrap();
    let plan = plan(records, archives, platform).unwrap();
    let mut sink = DirectorySink {
        root: prefix.path().to_path_buf(),
    };
    fs::create_dir_all(prefix.path().join("conda-meta")).unwrap();
    for package in plan.packages() {
        let record = package.link(&mut sink).unwrap();
        record
            .write_to_path(
                prefix.path().join("conda-meta").join(record.file_name()),
                true,
            )
            .unwrap();
    }
    prefix
}

async fn assert_same_as_regular_installer(archives: &[PathBuf], platform: Subdir) {
    let records: Vec<_> = archives.iter().map(get_repodata_record).collect();
    let regular = install_regular(&records, platform).await;
    let from_archive = install_from_archive(&records, archives, platform);

    let regular_files = snapshot(regular.path());
    let from_archive_files = snapshot(from_archive.path());
    assert!(!regular_files.is_empty());
    for (path, node) in &regular_files {
        // The regular installer leaves empty directories behind when it moves clobbered files.
        if *node == Node::EmptyDirectory && !from_archive_files.contains_key(path) {
            continue;
        }
        assert_eq!(
            Some(node),
            from_archive_files.get(path),
            "difference at {}",
            path.display()
        );
    }
    for path in from_archive_files.keys() {
        assert!(
            regular_files.contains_key(path),
            "{} is only created when linking from the archive",
            path.display()
        );
    }

    let regular_meta = conda_meta(regular.path());
    let from_archive_meta = conda_meta(from_archive.path());
    assert_eq!(
        regular_meta.keys().collect::<Vec<_>>(),
        from_archive_meta.keys().collect::<Vec<_>>()
    );
    for (name, paths) in &regular_meta {
        assert_eq!(paths, &from_archive_meta[name], "difference in {name}");
    }
}

fn clobber_packages(names: &[&str]) -> Vec<PathBuf> {
    names
        .iter()
        .map(|name| get_test_data_dir().join("clobber").join(name))
        .collect()
}

#[tokio::test]
async fn test_file_clobbers() {
    assert_same_as_regular_installer(
        &clobber_packages(&[
            "clobber-1-0.1.0-h4616a5c_0.tar.bz2",
            "clobber-2-0.1.0-h4616a5c_0.tar.bz2",
            "clobber-3-0.1.0-h4616a5c_0.tar.bz2",
        ]),
        Subdir::current().unwrap(),
    )
    .await;
}

#[tokio::test]
async fn test_nested_clobbers() {
    assert_same_as_regular_installer(
        &clobber_packages(&[
            "clobber-nested-1-0.1.0-h4616a5c_0.tar.bz2",
            "clobber-nested-2-0.1.0-h4616a5c_0.tar.bz2",
            "clobber-nested-3-0.1.0-h4616a5c_0.tar.bz2",
        ]),
        Subdir::current().unwrap(),
    )
    .await;
}

/// Links the clobber test packages `names` from their archives and returns the files and
/// symlinks in the prefix.
fn clobber_layout(names: &[&str]) -> Vec<PathBuf> {
    let archives = clobber_packages(names);
    let records: Vec<_> = archives.iter().map(get_repodata_record).collect();
    let prefix = install_from_archive(&records, &archives, Subdir::current().unwrap());
    snapshot(prefix.path()).into_keys().collect()
}

// The regular installer does not always produce the same layout when a file clobbers a
// directory: depending on the order in which the packages are linked it can leave a copy of a
// file in `__clobbers__` behind. So these tests check the layout directly instead of comparing it
// with the regular installer.

#[test]
fn test_directory_wins_over_file() {
    assert_eq!(
        clobber_layout(&[
            "clobber-with-dir-0.1.0-h4616a5c_0.conda",
            "clobber-without-dir-0.1.0-h4616a5c_0.conda",
        ]),
        ["__clobbers__/clobber-without-dir/dir", "dir/clobber.txt"].map(PathBuf::from)
    );
    assert_eq!(
        clobber_layout(&[
            "clobber-fd-rev-1-0.1.0-h4616a5c_0.conda",
            "clobber-fd-rev-2-0.1.0-h4616a5c_0.conda",
            "clobber-fd-rev-3-0.1.0-h4616a5c_0.conda",
        ]),
        [
            "__clobbers__/clobber-fd-rev-3/clobber",
            "clobber/clobber-fd-rev-1.txt",
            "clobber/clobber-fd-rev-2.txt",
        ]
        .map(PathBuf::from)
    );
}

#[test]
fn test_file_wins_over_directory() {
    assert_eq!(
        clobber_layout(&[
            "clobber-fd-1-0.1.0-h4616a5c_0.conda",
            "clobber-fd-2-0.1.0-h4616a5c_0.conda",
            "clobber-fd-3-0.1.0-h4616a5c_0.conda",
        ]),
        [
            "__clobbers__/clobber-fd-2/clobber/clobber-fd-2.txt",
            "__clobbers__/clobber-fd-3/clobber/clobber-fd-3.txt",
            "clobber",
        ]
        .map(PathBuf::from)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_symlink_clobbers() {
    assert_same_as_regular_installer(
        &clobber_packages(&[
            "clobber-with-symlink-a-0.1.0-h4616a5c_0.conda",
            "clobber-with-symlink-b-0.1.0-h4616a5c_0.conda",
        ]),
        Subdir::current().unwrap(),
    )
    .await;
}

#[test]
fn test_clobber_mode_error() {
    let archives = clobber_packages(&[
        "clobber-1-0.1.0-h4616a5c_0.tar.bz2",
        "clobber-2-0.1.0-h4616a5c_0.tar.bz2",
    ]);
    let records: Vec<_> = archives.iter().map(get_repodata_record).collect();
    let packages = records
        .iter()
        .zip(&archives)
        .map(|(record, archive)| (record.clone(), ArchivePackage::from_path(archive).unwrap()))
        .collect();
    let options = ArchiveLinkOptions {
        clobber_mode: ClobberMode::Error,
        ..ArchiveLinkOptions::new(TARGET_PREFIX, Subdir::current().unwrap())
    };
    let result = LinkPlan::new(packages, options);
    assert!(
        matches!(result, Err(FromArchiveError::ClobberingDetected(_))),
        "{result:?}"
    );
}

#[test]
fn test_noarch_python_requires_python() {
    let archives = clobber_packages(&["clobber-pynoarch-1-0.1.0-pyh4616a5c_0.tar.bz2"]);
    let records: Vec<_> = archives.iter().map(get_repodata_record).collect();
    let result = plan(&records, &archives, Subdir::Linux64);
    assert!(
        matches!(result, Err(FromArchiveError::MissingPythonInfo(_))),
        "{result:?}"
    );
}

/// Downloads the packages of an explicit environment for `platform` into the archive cache.
async fn download_explicit_environment(platform: Subdir) -> Vec<PathBuf> {
    let spec = ExplicitEnvironmentSpec::from_path(
        &get_test_data_dir().join(format!("python/explicit-env-{platform}.txt")),
    )
    .unwrap();
    let cache = ArchiveCache::new(temp_dir().join("rattler").join("from-archive-archives"));
    let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
    let mut archives = Vec::new();
    for package in spec.packages {
        let hash = match package.package_archive_hash().unwrap() {
            Some(PackageArchiveHash::Md5(md5)) => Some(ArchiveHash::Md5(md5)),
            Some(PackageArchiveHash::Sha256(sha256)) => Some(ArchiveHash::Sha256(sha256)),
            None => None,
        };
        let mut url = package.url.clone();
        url.set_fragment(None);
        archives.push(
            cache
                .get_or_fetch_from_url(&url, hash, &client, None)
                .await
                .unwrap(),
        );
    }
    archives
}

/// A real environment with python (text and binary prefix replacement), `noarch: python` packages
/// with entry points, symlinks, `.conda` and `.tar.bz2` archives.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_python_environment() {
    let platform = Subdir::current().unwrap();
    let archives = download_explicit_environment(platform).await;
    assert_same_as_regular_installer(&archives, platform).await;
}

/// Collects the files and symlinks in memory.
struct Collect(
    BTreeMap<PathBuf, (u32, Vec<u8>)>,
    BTreeMap<PathBuf, PathBuf>,
);
impl PrefixSink for Collect {
    fn add_file(
        &mut self,
        path: &Path,
        mode: u32,
        _size: u64,
        contents: &mut dyn Read,
    ) -> io::Result<()> {
        let mut bytes = Vec::new();
        contents.read_to_end(&mut bytes)?;
        self.0.insert(path.to_path_buf(), (mode, bytes));
        Ok(())
    }

    fn add_symlink(&mut self, path: &Path, target: &Path) -> io::Result<()> {
        self.1.insert(path.to_path_buf(), target.to_path_buf());
        Ok(())
    }

    fn add_directory(&mut self, _path: &Path, _mode: u32) -> io::Result<()> {
        Ok(())
    }
}

/// Linking from an archive does not depend on the host, so a Linux environment can be
/// installed on any host.
#[tokio::test]
async fn test_cross_platform_environment() {
    let archives = download_explicit_environment(Subdir::Linux64).await;
    let records: Vec<_> = archives.iter().map(get_repodata_record).collect();
    let plan = plan(&records, &archives, Subdir::Linux64).unwrap();

    let mut sink = Collect(BTreeMap::new(), BTreeMap::new());
    for package in plan.packages() {
        package.link(&mut sink).unwrap();
    }
    let (files, symlinks) = (sink.0, sink.1);

    // The interpreter keeps its executable bit and the shared library its symlinks.
    let (mode, _) = &files[Path::new("bin/python3.11")];
    assert_eq!(mode & 0o111, 0o111);
    assert_eq!(symlinks[Path::new("bin/python")], Path::new("python3.11"));
    assert_eq!(
        symlinks[Path::new("lib/libz.so.1")],
        Path::new("libz.so.1.2.13")
    );

    // Entry points of noarch python packages point to the python of the target prefix.
    let (mode, pip) = &files[Path::new("bin/pip")];
    assert_eq!(mode & 0o111, 0o111);
    assert!(pip.starts_with(format!("#!{TARGET_PREFIX}/bin/python3.11").as_bytes()));
    assert!(files.contains_key(Path::new("lib/python3.11/site-packages/pip/__init__.py")));

    // Prefix placeholders are replaced in text files.
    let (_, sysconfig) =
        &files[Path::new("lib/python3.11/_sysconfigdata__linux_x86_64-linux-gnu.py")];
    let sysconfig = String::from_utf8_lossy(sysconfig);
    assert!(sysconfig.contains(TARGET_PREFIX));
    assert!(!sysconfig.contains("placehold_placehold"));

    // Files whose names only differ in case are kept apart.
    assert!(files.contains_key(Path::new("share/terminfo/N/NCR260VT300WPP")));
    assert_eq!(
        symlinks[Path::new("share/terminfo/n/ncr260vt300wpp")],
        Path::new("../N/NCR260VT300WPP")
    );
}
