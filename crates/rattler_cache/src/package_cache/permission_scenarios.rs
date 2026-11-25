use std::{
    cell::Cell,
    fs::{self, Permissions},
    io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use super::{CacheKey, PackageCache, PackageCacheError};
use crate::validation::ValidationMode;
use rattler_conda_types::{PackageName, PackageRecord, VersionWithSource, package::BuildString};
use rattler_digest::Sha256Hash;
use tempfile::tempdir;

struct RestorePermissions {
    path: PathBuf,
    permissions: Permissions,
}

impl RestorePermissions {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            permissions: fs::metadata(path).unwrap().permissions(),
        }
    }
}

impl Drop for RestorePermissions {
    fn drop(&mut self) {
        fs::set_permissions(&self.path, self.permissions.clone()).unwrap();
    }
}

fn archive() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/clobber/clobber-python-0.1.0-cpython.conda")
}

fn record() -> PackageRecord {
    PackageRecord::new(
        PackageName::new_unchecked("clobber-python"),
        "0.1.0".parse::<VersionWithSource>().unwrap(),
        BuildString::new_unchecked("cpython"),
    )
}

async fn extract(destination: PathBuf) -> io::Result<Option<Sha256Hash>> {
    rattler_package_streaming::tokio::fs::extract(&archive(), &destination)
        .await
        .map(|result| Some(result.sha256))
        .map_err(io::Error::other)
}

fn layered(first: &Path, second: &Path) -> PackageCache {
    PackageCache::new_layered([first, second], false, ValidationMode::default())
}

fn access_is_denied(path: &Path, access: rustix::fs::Access) -> bool {
    match rustix::fs::accessat(rustix::fs::CWD, path, access, rustix::fs::AtFlags::EACCESS) {
        Err(rustix::io::Errno::ACCESS) => true,
        Ok(()) => {
            eprintln!("Skipping permission denial test: process overrides Unix permissions");
            false
        }
        Err(error) => panic!("unexpected access check failure: {error}"),
    }
}

#[tokio::test]
async fn non_searchable_ancestor_routes_package_and_global_lock_to_local_cache() {
    let parent = tempdir().unwrap();
    let local = tempdir().unwrap();
    let inaccessible = parent.path().join("shared/pkgs");
    fs::create_dir_all(&inaccessible).unwrap();
    let restore = RestorePermissions::new(parent.path());
    fs::set_permissions(parent.path(), Permissions::from_mode(0o666)).unwrap();
    if !access_is_denied(parent.path(), rustix::fs::Access::EXEC_OK) {
        return;
    }

    let cache = layered(&inaccessible, local.path());
    assert!(cache.layers()[0].is_readonly());
    let entry = cache
        .get_or_fetch_from_path(&archive(), None, None)
        .await
        .unwrap();
    assert!(entry.path().starts_with(local.path()));
    assert_eq!(
        fs::read(entry.path().join("bin/python")).unwrap(),
        b"cpython\n"
    );
    let lock = cache.acquire_global_lock().await.unwrap();
    assert!(local.path().join(".cache.lock").is_file());
    drop(lock);
    drop(restore);
    assert!(fs::read_dir(&inaccessible).unwrap().next().is_none());
}

#[tokio::test]
async fn readonly_wheel_namespace_does_not_disable_conda_or_global_lock() {
    let shared = tempdir().unwrap();
    let local = tempdir().unwrap();
    let wheel_key = CacheKey::from_wheel_record(&record());
    let relative = wheel_key.relative_path().unwrap();
    let namespace = shared.path().join(relative.parent().unwrap());
    fs::create_dir(&namespace).unwrap();
    let _restore = RestorePermissions::new(&namespace);
    fs::set_permissions(&namespace, Permissions::from_mode(0o555)).unwrap();

    let cache = layered(shared.path(), local.path());
    assert!(!cache.layers()[0].is_readonly());
    // Wheel keys use converted conda metadata.
    let wheel = cache
        .get_or_fetch(wheel_key.clone(), extract, None)
        .await
        .unwrap();
    assert_eq!(wheel.path(), local.path().join(&relative));
    assert!(fs::read_dir(&namespace).unwrap().next().is_none());
    let conda = cache
        .get_or_fetch_from_path(&archive(), None, None)
        .await
        .unwrap();
    assert!(conda.path().starts_with(shared.path()));
    assert_eq!(
        fs::read(conda.path().join("bin/python")).unwrap(),
        b"cpython\n"
    );
    let lock = cache.acquire_global_lock().await.unwrap();
    assert!(shared.path().join(".cache.lock").is_file());
    assert!(!local.path().join(".cache.lock").exists());
    drop(lock);

    let reopened = layered(shared.path(), local.path());
    let hit = reopened
        .get_or_fetch(
            wheel_key,
            |_| async {
                Err::<Option<Sha256Hash>, _>(io::Error::other(
                    "a valid converted wheel must not be fetched again",
                ))
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(hit.path(), wheel.path());
    assert_eq!(hit.sha256, wheel.sha256);
}

#[tokio::test]
async fn permission_revoked_during_fetch_retries_with_the_same_non_sync_callback() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    let restore = RestorePermissions::new(first.path());
    fs::set_permissions(first.path(), Permissions::from_mode(0o555)).unwrap();
    if !access_is_denied(first.path(), rustix::fs::Access::WRITE_OK) {
        return;
    }
    fs::set_permissions(first.path(), restore.permissions.clone()).unwrap();

    let first_path = first.path().to_owned();
    let destinations = Arc::new(Mutex::new(Vec::new()));
    let observed = destinations.clone();
    let calls = Cell::new(0usize);
    let key = CacheKey::from(&record());
    let cache = layered(first.path(), second.path());
    let entry = cache
        .get_or_fetch(
            key.clone(),
            move |destination| {
                let call = calls.get();
                calls.set(call + 1);
                observed.lock().unwrap().push(destination.clone());
                let first_path = first_path.clone();
                async move {
                    if call == 0 {
                        fs::set_permissions(&first_path, Permissions::from_mode(0o555))?;
                        fs::write(first_path.join("fetch-sidecar"), b"sidecar")?;
                        panic!("the first destination should reject writes");
                    }
                    extract(destination).await
                }
            },
            None,
        )
        .await
        .unwrap();
    assert!(entry.path().starts_with(second.path()));
    assert!(entry.sha256.is_some());
    assert_eq!(
        fs::read(entry.path().join("bin/python")).unwrap(),
        b"cpython\n"
    );
    {
        let destinations = destinations.lock().unwrap();
        assert_eq!(destinations.len(), 2);
        assert!(destinations[0].starts_with(first.path()));
        assert!(destinations[1].starts_with(second.path()));
    }
    assert!(!first.path().join(key.relative_path().unwrap()).exists());
    drop(restore);

    let hit = PackageCache::new(second.path())
        .get_or_fetch_from_path(&archive(), None, None)
        .await
        .unwrap();
    assert_eq!(hit.path(), entry.path());
    assert_eq!(hit.sha256, entry.sha256);
}

#[tokio::test]
async fn non_permission_publish_failure_cleans_extraction_without_trying_next_layer() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    let key = CacheKey::from(&record());
    let target = first.path().join(key.relative_path().unwrap());
    let obstruction = target.clone();
    let extracted = Arc::new(Mutex::new(None));
    let observed = extracted.clone();
    let error = layered(first.path(), second.path())
        .get_or_fetch(
            key,
            move |destination| {
                *observed.lock().unwrap() = Some(destination.clone());
                let obstruction = obstruction.clone();
                async move {
                    let hash = extract(destination).await?;
                    fs::write(obstruction, b"not a directory")?;
                    Ok::<_, io::Error>(hash)
                }
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, PackageCacheError::LayerError(_)));
    assert_eq!(fs::read(&target).unwrap(), b"not a directory");
    assert!(!extracted.lock().unwrap().as_ref().unwrap().exists());
    assert!(fs::read_dir(second.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn all_readonly_layers_report_no_writable_layer_without_fetching() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    let _first_restore = RestorePermissions::new(first.path());
    let _second_restore = RestorePermissions::new(second.path());
    for path in [first.path(), second.path()] {
        fs::set_permissions(path, Permissions::from_mode(0o555)).unwrap();
    }
    let cache = layered(first.path(), second.path());
    let error = cache
        .get_or_fetch(CacheKey::from(&record()), extract, None)
        .await
        .unwrap_err();
    assert!(matches!(error, PackageCacheError::NoWritableLayers));
    assert!(matches!(
        cache.acquire_global_lock().await,
        Err(PackageCacheError::NoWritableLayers)
    ));
    assert!(fs::read_dir(first.path()).unwrap().next().is_none());
    assert!(fs::read_dir(second.path()).unwrap().next().is_none());
}
