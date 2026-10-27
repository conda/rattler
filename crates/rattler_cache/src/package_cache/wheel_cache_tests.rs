use std::{
    io,
    path::{Path, PathBuf},
};

use rattler_conda_types::{
    MatchSpec, PackageName, PackageRecord, ParseMatchSpecOptions, RepoDataRecord,
    VersionWithSource,
    package::{BuildString, DistArchiveIdentifier},
};
use rattler_digest::{Sha256, compute_bytes_digest};
use rattler_package_streaming::wheel::CONVERSION_VERSION;
use tempfile::tempdir;

#[cfg(unix)]
use super::PackageCacheError;
use super::{CacheKey, PackageCache, PackageCacheLayer, cache_lock};

fn record() -> PackageRecord {
    PackageRecord::new(
        PackageName::new_unchecked("probe"),
        "1.0".parse::<VersionWithSource>().unwrap(),
        BuildString::new_unchecked("0"),
    )
}

/// Minimal converted-package metadata for exercising cache routing, not conversion.
fn seed_wheel(path: &Path, version: u64) -> io::Result<()> {
    std::fs::create_dir_all(path.join("info"))?;
    std::fs::write(
        path.join("info/index.json"),
        r#"{"name":"probe","version":"1.0","build":"0","build_number":0,"subdir":"noarch","noarch":"python"}"#,
    )?;
    std::fs::write(
        path.join("info/paths.json"),
        r#"{"paths_version":1,"paths":[]}"#,
    )?;
    std::fs::write(
        path.join("info/wheel.json"),
        format!(r#"{{"conversion_version":{version}}}"#),
    )?;
    Ok(())
}

async fn fetch_wheel(path: PathBuf) -> io::Result<()> {
    seed_wheel(&path, CONVERSION_VERSION)
}

async fn no_fetch(_: PathBuf) -> io::Result<()> {
    panic!("a cache hit must not fetch")
}

#[tokio::test]
async fn missing_wheel_probe_does_not_create_directories() {
    let root = tempdir().unwrap();
    let layer_path = root.path().join("absent");
    let key = CacheKey::from_wheel_record(&record());
    assert!(
        PackageCacheLayer::new(&layer_path)
            .try_validate(&key)
            .await
            .is_err()
    );
    assert!(!layer_path.exists());
}

#[tokio::test]
async fn conda_and_wheel_entries_do_not_share_paths_or_sidecars() {
    let root = tempdir().unwrap();
    let cache = PackageCache::new(root.path());
    let record = record();
    let conda_key = CacheKey::from(&record);
    let wheel_key = CacheKey::from_wheel_record(&record);
    let (conda, wheel) = tokio::join!(
        cache.get_or_fetch(
            conda_key.clone(),
            |path| async move {
                seed_wheel(&path, CONVERSION_VERSION)?;
                std::fs::remove_file(path.join("info/wheel.json"))
            },
            None
        ),
        cache.get_or_fetch(wheel_key.clone(), fetch_wheel, None),
    );
    let conda = conda.unwrap();
    let wheel = wheel.unwrap();
    assert_eq!(conda.path(), root.path().join("probe-1.0-0"));
    assert_ne!(conda.path(), wheel.path());
    assert!(cache_lock::metadata_path(conda.path()).is_file());
    assert!(cache_lock::metadata_path(wheel.path()).is_file());
    let index = cache.index().await.unwrap();
    let url = "https://example.invalid/download".parse().unwrap();
    assert_eq!(index.len(), 2);
    assert!(index.contains_url(conda_key, &url));
    assert!(index.contains_url(wheel_key, &url));
}

#[tokio::test]
async fn wheel_index_tracks_url_and_path_origins() {
    for cache_origin in [false, true] {
        let root = tempdir().unwrap();
        let cache = PackageCache::from_layers([PackageCacheLayer::new(root.path())], cache_origin);
        let record = record();
        let url = "https://example.invalid/download"
            .parse::<url::Url>()
            .unwrap();
        let key = CacheKey::from_wheel_record(&record);
        let url_key = if cache_origin {
            key.clone().with_url(url.clone())
        } else {
            key.clone()
        };
        cache
            .get_or_fetch(url_key, fetch_wheel, None)
            .await
            .unwrap();
        let mut repodata = RepoDataRecord {
            package_record: record.clone(),
            identifier: DistArchiveIdentifier::try_from_filename("probe-1.0-py3-none-any.whl")
                .unwrap(),
            url: url.clone(),
            channel: None,
        };
        let index = cache.index().await.unwrap();
        assert!(index.contains_record(&repodata));
        assert!(index.contains_url(key.clone(), &url));
        assert!(!index.contains_url(&record, &url));
        assert_eq!(
            index.contains_url(
                key.clone(),
                &"https://example.invalid/other".parse().unwrap()
            ),
            !cache_origin
        );

        let path = root.path().join("probe.whl");
        let path_key = if cache_origin {
            key.with_path(&path)
        } else {
            key
        };
        cache
            .get_or_fetch(path_key, fetch_wheel, None)
            .await
            .unwrap();
        repodata.url = url::Url::from_file_path(&path).unwrap();
        let index = cache.index().await.unwrap();
        assert!(index.contains_record(&repodata));
        assert!(index.contains_path(&record, &path));
        assert_eq!(index.len(), if cache_origin { 2 } else { 1 });
    }
}

#[tokio::test]
async fn old_wheel_namespaces_are_ignored_and_preserved() {
    let root = tempdir().unwrap();
    let record = record();
    let key = CacheKey::from_wheel_record(&record);
    let legacy = root.path().join(key.to_path_segment().unwrap());
    let older = root
        .path()
        .join(format!("wheels-v{}", CONVERSION_VERSION - 1))
        .join(key.to_path_segment().unwrap());
    seed_wheel(&legacy, CONVERSION_VERSION).unwrap();
    seed_wheel(&older, CONVERSION_VERSION - 1).unwrap();
    let originals =
        [&legacy, &older].map(|path| std::fs::read(path.join("info/wheel.json")).unwrap());
    let cache = PackageCache::new(root.path());
    let url = "https://example.invalid/download".parse().unwrap();
    let before = cache.index().await.unwrap();
    assert!(!before.contains_url(key.clone(), &url));
    assert_eq!(before.len(), 1); // The legacy root entry, not the namespace directory.
    let cached = cache
        .get_or_fetch(key.clone(), fetch_wheel, None)
        .await
        .unwrap();
    assert_eq!(
        cached.path(),
        root.path().join(key.relative_path().unwrap())
    );
    assert!(cache.index().await.unwrap().contains_url(key.clone(), &url));
    assert!(!before.contains_url(key.clone(), &url)); // Snapshots do not change.
    assert_eq!(cache.index().await.unwrap().len(), 2);
    for (path, original) in [&legacy, &older].into_iter().zip(originals) {
        assert_eq!(
            std::fs::read(path.join("info/wheel.json")).unwrap(),
            original
        );
        assert!(!cache_lock::metadata_path(path).exists());
    }
    let reused = PackageCache::new(root.path())
        .get_or_fetch(key, no_fetch, None)
        .await
        .unwrap();
    assert_eq!(cached.path(), reused.path());
    assert_eq!(cached.revision(), reused.revision());
}

#[tokio::test]
async fn wheel_index_preserves_layer_filters_and_hash_matching() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    let mut record = record();
    record.sha256 = Some(compute_bytes_digest::<Sha256>(b"first archive"));
    PackageCache::new(first.path())
        .get_or_fetch(CacheKey::from_wheel_record(&record), fetch_wheel, None)
        .await
        .unwrap();
    record.sha256 = Some(compute_bytes_digest::<Sha256>(b"second archive"));
    let key = CacheKey::from_wheel_record(&record);
    let expected = PackageCache::new(second.path())
        .get_or_fetch(key.clone(), fetch_wheel, None)
        .await
        .unwrap();
    let cache = PackageCache::from_layers(
        [
            PackageCacheLayer::new(first.path()),
            PackageCacheLayer::new(second.path()),
        ],
        false,
    );
    let url = "https://example.invalid/download".parse().unwrap();
    assert!(cache.index().await.unwrap().contains_url(key.clone(), &url));
    assert_eq!(
        cache
            .get_or_fetch(key.clone(), no_fetch, None)
            .await
            .unwrap()
            .path(),
        expected.path()
    );
    let filtered = PackageCache::from_layers(
        [
            PackageCacheLayer::new(first.path()),
            PackageCacheLayer::new(second.path())
                .excluding(MatchSpec::from_str("probe", ParseMatchSpecOptions::default()).unwrap()),
        ],
        false,
    );
    assert!(!filtered.index().await.unwrap().contains_url(key, &url));
}

#[cfg(unix)]
struct Readonly {
    path: PathBuf,
    permissions: std::fs::Permissions,
}

#[cfg(unix)]
impl Readonly {
    fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let permissions = std::fs::metadata(&path).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&path, readonly).unwrap();
        Self { path, permissions }
    }
}

#[cfg(unix)]
impl Drop for Readonly {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.path, self.permissions.clone());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn readonly_wheel_hit_does_not_write_metadata() {
    let root = tempdir().unwrap();
    let key = CacheKey::from_wheel_record(&record());
    let cached = PackageCache::new(root.path())
        .get_or_fetch(key.clone(), fetch_wheel, None)
        .await
        .unwrap();
    let metadata_path = cache_lock::metadata_path(cached.path());
    let original_metadata = std::fs::read(&metadata_path).unwrap();
    let _metadata = Readonly::new(&metadata_path);
    let _namespace = Readonly::new(cached.path().parent().unwrap());
    let _layer = Readonly::new(root.path());
    let reused = PackageCache::new(root.path())
        .get_or_fetch(key, no_fetch, None)
        .await
        .unwrap();
    assert_eq!(reused.path(), cached.path());
    assert_eq!(reused.revision(), cached.revision());
    assert_eq!(std::fs::read(metadata_path).unwrap(), original_metadata);
}

#[cfg(unix)]
#[tokio::test]
async fn readonly_wheel_without_sidecar_is_validated_each_time() {
    let root = tempdir().unwrap();
    let key = CacheKey::from_wheel_record(&record());
    let path = root.path().join(key.relative_path().unwrap());
    seed_wheel(&path, CONVERSION_VERSION).unwrap();
    let _namespace = Readonly::new(path.parent().unwrap());
    let _root = Readonly::new(root.path());
    let layer = PackageCacheLayer::new(root.path());
    layer.try_validate(&key).await.unwrap();
    assert!(!cache_lock::metadata_path(&path).exists());
    std::fs::remove_file(path.join("info/index.json")).unwrap();
    assert!(layer.try_validate(&key).await.is_err());
    assert!(!cache_lock::metadata_path(&path).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn readonly_wheel_namespace_falls_back_to_next_layer() {
    let root = tempdir().unwrap();
    let fallback = tempdir().unwrap();
    let key = CacheKey::from_wheel_record(&record());
    let namespace = root.path().join(key.namespace.directory());
    std::fs::create_dir(&namespace).unwrap();
    let _namespace = Readonly::new(&namespace);
    let cache = PackageCache::from_layers(
        [
            PackageCacheLayer::new(root.path()),
            PackageCacheLayer::new(fallback.path()),
        ],
        false,
    );
    let cached = cache
        .get_or_fetch(key.clone(), fetch_wheel, None)
        .await
        .unwrap();
    assert_eq!(
        cached.path(),
        fallback.path().join(key.relative_path().unwrap())
    );
    assert_eq!(std::fs::read_dir(&namespace).unwrap().count(), 0);
    assert!(matches!(
        PackageCache::new(root.path())
            .get_or_fetch(key, no_fetch, None)
            .await,
        Err(PackageCacheError::NoWritableLayers)
    ));
}

#[cfg(unix)]
#[rstest::rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn invalid_readonly_wheel_is_preserved(#[case] current_namespace: bool) {
    let root = tempdir().unwrap();
    let fallback = tempdir().unwrap();
    let key = CacheKey::from_wheel_record(&record());
    let stale = if current_namespace {
        root.path().join(key.relative_path().unwrap())
    } else {
        root.path()
            .join(format!("wheels-v{}", CONVERSION_VERSION - 1))
            .join(key.to_path_segment().unwrap())
    };
    seed_wheel(&stale, CONVERSION_VERSION - 1).unwrap();
    let original = std::fs::read(stale.join("info/wheel.json")).unwrap();
    let _namespace = Readonly::new(stale.parent().unwrap());
    let _layer = Readonly::new(root.path());
    assert!(matches!(
        PackageCache::new(root.path())
            .get_or_fetch(key.clone(), no_fetch, None)
            .await,
        Err(PackageCacheError::NoWritableLayers)
    ));
    let cache = PackageCache::from_layers(
        [
            PackageCacheLayer::new(root.path()),
            PackageCacheLayer::new(fallback.path()),
        ],
        false,
    );
    let cached = cache
        .get_or_fetch(key.clone(), fetch_wheel, None)
        .await
        .unwrap();
    assert_eq!(
        cached.path(),
        fallback.path().join(key.relative_path().unwrap())
    );
    assert_eq!(
        std::fs::read(stale.join("info/wheel.json")).unwrap(),
        original
    );
    assert!(!cache_lock::metadata_path(&stale).exists());
}
