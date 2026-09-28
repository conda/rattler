//! Indexing the file paths contained in the packages of a channel, the
//! `lookup` feature of `rattler_index`.

use std::{
    fs::{self, File},
    path::Path,
};

use rattler_conda_types::{Platform, compression_level::CompressionLevel};
use rattler_index::ChannelMetadata;
use rattler_index::{
    IndexFsConfig, LookupOptions, PackageRevisionAssignment, index_fs,
    index_fs_with_channel_metadata,
};
use rattler_lookup::{Manifest, PathLookup, Query};
use rattler_package_streaming::write::write_tar_bz2_package;
use serde_json::Value;
use url::Url;

/// How a package lists the files it contains.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listing {
    /// `info/paths.json`, as every current package does.
    PathsJson,
    /// Only `info/files`, as packages from before `paths.json` do.
    Files,
}

/// Writes a `noarch` package that contains `paths`.
fn write_package(subdir_path: &Path, name: &str, paths: &[&str], listing: Listing) {
    let build_dir = subdir_path.join("package-build");
    let info_dir = build_dir.join("info");
    fs::create_dir_all(&info_dir).unwrap();

    let index_json = serde_json::json!({
        "build": "0",
        "build_number": 0,
        "name": name,
        "noarch": "generic",
        "subdir": "noarch",
        "version": "1.0",
    });
    fs::write(
        info_dir.join("index.json"),
        serde_json::to_vec(&index_json).unwrap(),
    )
    .unwrap();

    match listing {
        Listing::PathsJson => {
            let paths_json = serde_json::json!({
                "paths_version": 1,
                "paths": paths.iter().map(|path| serde_json::json!({
                    "_path": path,
                    "path_type": "hardlink",
                    "sha256": "0".repeat(64),
                    "size_in_bytes": 0,
                })).collect::<Vec<_>>(),
            });
            fs::write(
                info_dir.join("paths.json"),
                serde_json::to_vec(&paths_json).unwrap(),
            )
            .unwrap();
        }
        Listing::Files => {
            fs::write(info_dir.join("files"), format!("{}\n", paths.join("\n"))).unwrap();
        }
    }

    let mut files = vec![info_dir.join("index.json")];
    files.push(match listing {
        Listing::PathsJson => info_dir.join("paths.json"),
        Listing::Files => info_dir.join("files"),
    });
    for path in paths {
        let file = build_dir.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, name).unwrap();
        files.push(file);
    }

    write_tar_bz2_package(
        File::create(subdir_path.join(format!("{name}-1.0-0.tar.bz2"))).unwrap(),
        &build_dir,
        &files,
        CompressionLevel::Default,
        None,
        None,
    )
    .unwrap();
    fs::remove_dir_all(&build_dir).unwrap();
}

fn index_config(channel: &Path, lookup: LookupOptions) -> IndexFsConfig {
    IndexFsConfig {
        channel: channel.to_path_buf(),
        target_platform: Some(Platform::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: true,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force: false,
        max_parallel: 4,
        multi_progress: None,
        lookup: Some(lookup),
    }
}

fn read_manifest(channel: &Path) -> Manifest {
    let bytes = fs::read(channel.join("noarch/lookup/manifest.json")).unwrap();
    Manifest::from_bytes(&bytes).unwrap()
}

/// The filenames of the artifacts of the index in `channel` that contain `path`.
async fn find(channel: &Path, path: &str) -> Vec<String> {
    let base = Url::from_directory_path(channel).unwrap();
    let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
    let lookup = PathLookup::for_index_base(&base, &[Platform::NoArch], client)
        .await
        .unwrap();
    lookup
        .find(path)
        .await
        .unwrap()
        .into_iter()
        .map(|found| found.file_name)
        .collect()
}

/// The paths of the index in `channel` that match `pattern`, each with the
/// artifacts that contain them.
async fn search(channel: &Path, pattern: &str) -> Vec<(String, Vec<String>)> {
    let base = Url::from_directory_path(channel).unwrap();
    let client = reqwest_middleware::ClientWithMiddleware::from(reqwest::Client::new());
    let lookup = PathLookup::for_index_base(&base, &[Platform::NoArch], client)
        .await
        .unwrap();
    let query = Query::parse(pattern).unwrap();
    lookup
        .search(&query, 100)
        .await
        .unwrap()
        .paths
        .into_iter()
        .map(|(path, artifacts)| {
            (
                path,
                artifacts.into_iter().map(|found| found.file_name).collect(),
            )
        })
        .collect()
}

/// Every layer file the manifest refers to exists and has the size it promises,
/// and every layer has a table of every kind of the index.
fn assert_layer_files(channel: &Path, manifest: &Manifest) {
    let dir = channel.join("noarch/lookup");
    assert_eq!(manifest.kinds, ["paths", "reversed-paths"]);
    for layer in &manifest.layers {
        let tables = manifest.kinds.iter().map(|kind| {
            let table = layer.tables.get(kind).expect("a table of every kind");
            (&table.file, table.size)
        });
        for (file, size) in tables.chain([(&layer.packages.file, layer.packages.size)]) {
            let metadata = fs::metadata(dir.join(file)).unwrap();
            assert_eq!(metadata.len(), size, "{file} has an unexpected size");
        }
    }
}

/// Indexing a channel writes a lookup index that the repodata points at, grows it
/// by one layer per run, compacts it, and forgets deleted packages.
#[tokio::test]
async fn test_lookup_index() {
    let temp_dir = tempfile::tempdir().unwrap();
    let channel = temp_dir.path();
    let subdir_path = channel.join("noarch");
    fs::create_dir(&subdir_path).unwrap();

    write_package(
        &subdir_path,
        "alpha",
        &["bin/alpha", "share/common.txt"],
        Listing::PathsJson,
    );
    // Packages that predate `info/paths.json` only list their files.
    write_package(
        &subdir_path,
        "beta",
        &["bin/beta", "share/common.txt"],
        Listing::Files,
    );

    index_fs(index_config(channel, LookupOptions::default()))
        .await
        .unwrap();

    let manifest = read_manifest(channel);
    assert_eq!(manifest.version, 1);
    assert_eq!(manifest.subdir, "noarch");
    assert_eq!(manifest.layers.len(), 1);
    assert_eq!(manifest.layers[0].packages.count, 2);
    assert!(manifest.removed.is_empty());
    assert_layer_files(channel, &manifest);

    // The repodata has to point at the manifest, both in `repodata.json` and in
    // the sharded index.
    let repodata: Value =
        serde_json::from_slice(&fs::read(subdir_path.join("repodata.json")).unwrap()).unwrap();
    assert_eq!(
        repodata["info"]["lookup_url"].as_str(),
        Some("./lookup/manifest.json")
    );
    let shards = zstd::decode_all(
        fs::read(subdir_path.join("repodata_shards.msgpack.zst"))
            .unwrap()
            .as_slice(),
    )
    .unwrap();
    let shards: rattler_conda_types::ShardedRepodata = rmp_serde::from_slice(&shards).unwrap();
    assert_eq!(
        shards.info.lookup_url.as_deref(),
        Some("./lookup/manifest.json")
    );

    assert_eq!(find(channel, "bin/alpha").await, ["alpha-1.0-0.tar.bz2"]);
    assert_eq!(
        find(channel, "share/common.txt").await,
        ["alpha-1.0-0.tar.bz2", "beta-1.0-0.tar.bz2"]
    );
    assert!(find(channel, "bin/gamma").await.is_empty());

    // Patterns are answered by the `paths` table, `**/…` ones by the
    // `reversed-paths` table the indexer writes next to it.
    assert_eq!(
        search(channel, "bin/*").await,
        [
            ("bin/alpha".to_string(), vec!["alpha-1.0-0.tar.bz2".into()]),
            ("bin/beta".to_string(), vec!["beta-1.0-0.tar.bz2".into()]),
        ]
    );
    assert_eq!(
        search(channel, "**/common.txt").await,
        [(
            "share/common.txt".to_string(),
            vec![
                "alpha-1.0-0.tar.bz2".to_string(),
                "beta-1.0-0.tar.bz2".into()
            ]
        )]
    );

    // A second run only indexes what is not covered yet, in a new layer.
    write_package(
        &subdir_path,
        "gamma",
        &["bin/gamma", "share/common.txt"],
        Listing::PathsJson,
    );
    index_fs(index_config(channel, LookupOptions::default()))
        .await
        .unwrap();

    let manifest = read_manifest(channel);
    assert_eq!(manifest.layers.len(), 2);
    assert_eq!(manifest.layers[1].packages.count, 1);
    assert!(manifest.removed.is_empty());
    assert_layer_files(channel, &manifest);
    assert_eq!(find(channel, "bin/gamma").await, ["gamma-1.0-0.tar.bz2"]);
    assert_eq!(
        find(channel, "share/common.txt").await,
        [
            "alpha-1.0-0.tar.bz2",
            "beta-1.0-0.tar.bz2",
            "gamma-1.0-0.tar.bz2"
        ]
    );

    // Indexing again changes nothing, so the manifest is left alone.
    let before = fs::read(channel.join("noarch/lookup/manifest.json")).unwrap();
    index_fs(index_config(channel, LookupOptions::default()))
        .await
        .unwrap();
    assert_eq!(
        fs::read(channel.join("noarch/lookup/manifest.json")).unwrap(),
        before,
        "an unchanged index is not rewritten"
    );

    // Above the threshold all layers are merged into one.
    index_fs(index_config(
        channel,
        LookupOptions {
            compact_threshold: 1,
            compact: false,
        },
    ))
    .await
    .unwrap();

    let manifest = read_manifest(channel);
    assert_eq!(manifest.layers.len(), 1);
    assert_eq!(manifest.layers[0].packages.count, 3);
    assert!(manifest.removed.is_empty());
    assert_layer_files(channel, &manifest);
    assert_eq!(
        find(channel, "share/common.txt").await,
        [
            "alpha-1.0-0.tar.bz2",
            "beta-1.0-0.tar.bz2",
            "gamma-1.0-0.tar.bz2"
        ]
    );

    // A deleted package stays in its layer, so the manifest excludes it.
    fs::remove_file(subdir_path.join("alpha-1.0-0.tar.bz2")).unwrap();
    index_fs(index_config(channel, LookupOptions::default()))
        .await
        .unwrap();

    let manifest = read_manifest(channel);
    assert_eq!(manifest.layers.len(), 1);
    assert_eq!(
        manifest.removed.iter().collect::<Vec<_>>(),
        ["alpha-1.0-0.tar.bz2"]
    );
    assert!(find(channel, "bin/alpha").await.is_empty());
    assert_eq!(
        find(channel, "share/common.txt").await,
        ["beta-1.0-0.tar.bz2", "gamma-1.0-0.tar.bz2"]
    );

    // Compacting drops the removed artifact entirely.
    index_fs(index_config(
        channel,
        LookupOptions {
            compact_threshold: 8,
            compact: true,
        },
    ))
    .await
    .unwrap();

    let manifest = read_manifest(channel);
    assert_eq!(manifest.layers.len(), 1);
    assert_eq!(manifest.layers[0].packages.count, 2);
    assert!(manifest.removed.is_empty());
    assert_layer_files(channel, &manifest);
    assert!(find(channel, "bin/alpha").await.is_empty());
    assert_eq!(
        find(channel, "share/common.txt").await,
        ["beta-1.0-0.tar.bz2", "gamma-1.0-0.tar.bz2"]
    );
}

/// A channel that is indexed without the lookup index does not advertise one.
#[tokio::test]
async fn test_index_without_lookup() {
    let temp_dir = tempfile::tempdir().unwrap();
    let channel = temp_dir.path();
    let subdir_path = channel.join("noarch");
    fs::create_dir(&subdir_path).unwrap();
    write_package(&subdir_path, "alpha", &["bin/alpha"], Listing::PathsJson);

    let mut config = index_config(channel, LookupOptions::default());
    config.lookup = None;
    index_fs_with_channel_metadata(config, ChannelMetadata::default())
        .await
        .unwrap();

    assert!(!channel.join("noarch/lookup").exists());
    let repodata: Value =
        serde_json::from_slice(&fs::read(subdir_path.join("repodata.json")).unwrap()).unwrap();
    assert!(repodata["info"].get("lookup_url").is_none());
}
