use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use tempfile::tempdir;

use super::{
    CacheKey, PackageCache, PackageCacheError, PackageCacheLayer, PackageCacheLayerError,
    cache_lock,
};
use crate::validation::ValidationMode;
use rattler_conda_types::package::CondaArchiveIdentifier;

struct PermissionsGuard {
    path: PathBuf,
    original: std::fs::Permissions,
}

impl Drop for PermissionsGuard {
    fn drop(&mut self) {
        std::fs::set_permissions(&self.path, self.original.clone()).unwrap();
    }
}

fn restrict_permissions(path: &Path, mode: u32) -> Option<PermissionsGuard> {
    let guard = PermissionsGuard {
        path: path.to_owned(),
        original: std::fs::metadata(path).unwrap().permissions(),
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    if rustix::fs::accessat(
        rustix::fs::CWD,
        path,
        rustix::fs::Access::WRITE_OK,
        rustix::fs::AtFlags::EACCESS,
    )
    .is_ok()
    {
        eprintln!("Skipping permission denial test: process can override Unix permissions");
        return None;
    }
    Some(guard)
}

fn package(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/clobber")
        .join(name)
}

fn key(archive: &Path) -> CacheKey {
    CondaArchiveIdentifier::try_from_path(archive)
        .unwrap()
        .into()
}

#[tokio::test]
async fn effective_permissions_preserve_hits_and_route_misses() {
    let shared = tempdir().unwrap();
    let local = tempdir().unwrap();
    let cached = package("clobber-python-0.1.0-cpython.conda");
    let missing = package("clobber-pypy-0.1.0-h4616a5c_0.conda");
    PackageCache::new(shared.path())
        .get_or_fetch_from_path(&cached, None, None)
        .await
        .unwrap();

    // Write bits exist, but none grant access to the directory's owner.
    let Some(_permissions) = restrict_permissions(shared.path(), 0o577) else {
        return;
    };
    let cache = PackageCache::new_layered(
        [shared.path(), local.path()],
        false,
        ValidationMode::default(),
    );
    let hit = cache
        .get_or_fetch_from_path(&cached, None, None)
        .await
        .unwrap();
    assert!(hit.path().starts_with(shared.path()));
    assert_eq!(
        std::fs::read(hit.path().join("bin/python")).unwrap(),
        b"cpython\n"
    );
    let miss = cache
        .get_or_fetch_from_path(&missing, None, None)
        .await
        .unwrap();
    assert!(miss.path().starts_with(local.path()));
    assert!(
        !shared
            .path()
            .join(key(&missing).relative_path().unwrap())
            .exists()
    );
    let _lock = cache.acquire_global_lock().await.unwrap();
    assert!(local.path().join(".cache.lock").is_file());
    assert!(!shared.path().join(".cache.lock").exists());
}

#[tokio::test]
async fn missing_layer_checks_its_existing_parent() {
    let shared = tempdir().unwrap();
    let local = tempdir().unwrap();
    let Some(_permissions) = restrict_permissions(shared.path(), 0o577) else {
        return;
    };
    let missing = shared.path().join("not-created/pkgs");
    let cache = PackageCache::new_layered(
        [missing.as_path(), local.path()],
        false,
        ValidationMode::default(),
    );
    assert!(cache.layers()[0].is_readonly());
    let archive = package("clobber-python-0.1.0-cpython.conda");
    let entry = cache
        .get_or_fetch_from_path(&archive, None, None)
        .await
        .unwrap();
    assert!(entry.path().starts_with(local.path()));
    assert!(!missing.exists());
    let _lock = cache.acquire_global_lock().await.unwrap();
    assert!(local.path().join(".cache.lock").is_file());
}

#[tokio::test]
async fn unwritable_metadata_falls_back_to_next_eligible_layer() {
    let first = tempdir().unwrap();
    let filtered = tempdir().unwrap();
    let last = tempdir().unwrap();
    let archive = package("clobber-python-0.1.0-cpython.conda");
    let metadata_path =
        cache_lock::metadata_path(&first.path().join(key(&archive).relative_path().unwrap()));
    std::fs::write(&metadata_path, 0u64.to_be_bytes()).unwrap();
    let Some(_permissions) = restrict_permissions(&metadata_path, 0o444) else {
        return;
    };
    let cache = PackageCache::from_layers(
        [
            PackageCacheLayer::new(first.path()),
            PackageCacheLayer::new(filtered.path()).excluding("clobber-python".parse().unwrap()),
            PackageCacheLayer::new(last.path()),
        ],
        false,
    );
    assert!(!cache.layers()[0].is_readonly());
    let entry = cache
        .get_or_fetch_from_path(&archive, None, None)
        .await
        .unwrap();
    assert!(entry.path().starts_with(last.path()));
    assert_eq!(
        std::fs::read(entry.path().join("bin/python")).unwrap(),
        b"cpython\n"
    );
    assert!(
        !filtered
            .path()
            .join(key(&archive).relative_path().unwrap())
            .exists()
    );

    let error = PackageCache::new(first.path())
        .get_or_fetch_from_path(&archive, None, None)
        .await
        .unwrap_err();
    assert!(matches!(error, PackageCacheError::LayerError(error)
        if error.downcast_ref::<PackageCacheLayerError>().unwrap().is_permission_error()));
}

#[tokio::test]
async fn unwritable_global_lock_does_not_use_an_unrelated_lock() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    let lock_path = first.path().join(".cache.lock");
    std::fs::write(&lock_path, []).unwrap();
    let Some(_permissions) = restrict_permissions(&lock_path, 0o444) else {
        return;
    };
    let cache = PackageCache::new_layered(
        [first.path(), second.path()],
        false,
        ValidationMode::default(),
    );
    let error = cache.acquire_global_lock().await.unwrap_err();
    assert!(!second.path().join(".cache.lock").exists());
    assert!(matches!(error, PackageCacheError::LayerError(error)
        if error.downcast_ref::<PackageCacheLayerError>().unwrap().is_permission_error()));
}

#[tokio::test]
async fn unwritable_stale_entry_falls_back_and_cleans_extraction() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    let archive = package("clobber-python-0.1.0-cpython.conda");
    let stale = first.path().join(key(&archive).relative_path().unwrap());
    let protected = stale.join("protected");
    std::fs::create_dir_all(&protected).unwrap();
    std::fs::write(protected.join("file"), b"old content").unwrap();
    let Some(_permissions) = restrict_permissions(&protected, 0o555) else {
        return;
    };
    let cache = PackageCache::new_layered(
        [first.path(), second.path()],
        false,
        ValidationMode::default(),
    );
    let entry = cache
        .get_or_fetch_from_path(&archive, None, None)
        .await
        .unwrap();
    assert!(entry.path().starts_with(second.path()));
    assert_eq!(
        std::fs::read(entry.path().join("bin/python")).unwrap(),
        b"cpython\n"
    );
    assert_eq!(
        std::fs::read(protected.join("file")).unwrap(),
        b"old content"
    );
    assert!(std::fs::read_dir(first.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".clobber-python")
    }));
}

#[tokio::test]
async fn unrelated_cache_errors_do_not_fall_back() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    let archive = package("clobber-python-0.1.0-cpython.conda");
    let metadata_path =
        cache_lock::metadata_path(&first.path().join(key(&archive).relative_path().unwrap()));
    std::fs::create_dir(&metadata_path).unwrap();
    let cache = PackageCache::new_layered(
        [first.path(), second.path()],
        false,
        ValidationMode::default(),
    );
    let error = cache
        .get_or_fetch_from_path(&archive, None, None)
        .await
        .unwrap_err();
    assert!(matches!(error, PackageCacheError::LayerError(error)
        if !error.downcast_ref::<PackageCacheLayerError>().unwrap().is_permission_error()));
    assert!(
        !second
            .path()
            .join(key(&archive).relative_path().unwrap())
            .exists()
    );
}

#[tokio::test]
async fn source_permission_errors_do_not_fall_back() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    let source = tempdir().unwrap();
    let archive = source.path().join("clobber-python-0.1.0-cpython.conda");
    std::fs::copy(package("clobber-python-0.1.0-cpython.conda"), &archive).unwrap();
    let Some(_permissions) = restrict_permissions(&archive, 0o000) else {
        return;
    };
    let cache = PackageCache::new_layered(
        [first.path(), second.path()],
        false,
        ValidationMode::default(),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let fetch_calls = calls.clone();
    let error = cache
        .get_or_fetch(
            key(&archive),
            move |_| {
                fetch_calls.fetch_add(1, Ordering::Relaxed);
                let result = std::fs::read(&archive).map(|_| None);
                std::future::ready(result)
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, PackageCacheError::LayerError(error)
        if error.downcast_ref::<PackageCacheLayerError>().unwrap().is_permission_error()));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(std::fs::read_dir(second.path()).unwrap().next().is_none());
}
