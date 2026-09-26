//! The lookup index (`<subdir>/lookup/`, which artifacts contain a file)
//! written alongside the repodata.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use rattler_conda_types::{Platform, ShardedRepodata};
use rattler_index::{IndexFsConfig, PackageRevisionAssignment, index_fs};
use rattler_lookup::{Kind, Location, Manifest, Query, SubdirIndex, bulk, discovery};
use serde_json::Value;

const SUBDIR: &str = "win-64";
const CONDA_PACKAGE: &str = "conda-22.11.1-py38haa244fe_1.conda";
const TAR_BZ2_PACKAGE: &str = "conda-22.9.0-py38haa244fe_2.tar.bz2";

fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
}

fn client() -> reqwest_middleware::ClientWithMiddleware {
    reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build()
}

/// Downloads the two test packages into `<channel>/win-64/`.
async fn populate(channel: &Path) {
    let conda = tokio::task::spawn_blocking(|| {
        tools::download_and_cache_file(
            format!("https://conda.anaconda.org/conda-forge/win-64/{CONDA_PACKAGE}")
                .parse()
                .unwrap(),
            "a8a44c5ff2b2f423546d49721ba2e3e632233c74a813c944adf8e5742834930e",
        )
    })
    .await
    .unwrap()
    .unwrap();
    let tar_bz2 = tokio::task::spawn_blocking(|| {
        tools::download_and_cache_file(
            format!("https://conda.anaconda.org/conda-forge/win-64/{TAR_BZ2_PACKAGE}")
                .parse()
                .unwrap(),
            "3c2c2e8e81bde5fb1ac4b014f51a62411feff004580c708c97a0ec2b7058cdc4",
        )
    })
    .await
    .unwrap()
    .unwrap();
    let subdir = channel.join(SUBDIR);
    fs::create_dir_all(&subdir).unwrap();
    fs::copy(&conda, subdir.join(CONDA_PACKAGE)).unwrap();
    fs::copy(&tar_bz2, subdir.join(TAR_BZ2_PACKAGE)).unwrap();
}

async fn index(channel: &Path, write_lookup: bool, force: bool) -> rattler_index::IndexStats {
    let mut config = FsConfigBuilder::new(channel, write_lookup, force).0;
    config.multi_progress = None;
    let root = channel.canonicalize().unwrap();
    let mut fs_config = opendal::services::FsConfig::default();
    fs_config.root = Some(root.to_string_lossy().to_string());
    let op = opendal::Operator::new(opendal::Configurator::into_builder(fs_config))
        .unwrap()
        .finish();
    rattler_index::index(
        config.target_platform,
        op,
        None,
        config.write_zst,
        config.write_shards,
        config.write_lookup,
        Vec::new(),
        PackageRevisionAssignment::default(),
        config.force,
        4,
        None,
        rattler_index::PreconditionChecks::Disabled,
    )
    .await
    .unwrap()
}

struct FsConfigBuilder(IndexFsConfig);

impl FsConfigBuilder {
    fn new(channel: &Path, write_lookup: bool, force: bool) -> Self {
        Self(IndexFsConfig {
            channel: channel.to_path_buf(),
            target_platform: Some(Platform::Win64),
            repodata_patch: None,
            write_zst: true,
            write_shards: true,
            write_lookup,
            repodata_revisions: Vec::new(),
            package_revision_assignment: PackageRevisionAssignment::default(),
            force,
            max_parallel: 4,
            multi_progress: None,
        })
    }
}

fn read_manifest(channel: &Path) -> Manifest {
    let path = channel.join(SUBDIR).join("lookup").join("manifest.json");
    Manifest::parse(&fs::read(&path).unwrap(), &Location::from(path)).unwrap()
}

fn read_repodata(channel: &Path) -> Value {
    serde_json::from_slice(&fs::read(channel.join(SUBDIR).join("repodata.json")).unwrap()).unwrap()
}

async fn open_index(channel: &Path) -> SubdirIndex {
    let manifest = discovery::discover_manifest(&Location::from(channel), SUBDIR, &client())
        .await
        .unwrap()
        .expect("the repodata points to the lookup index");
    SubdirIndex::open(manifest, &Kind::ALL, &client())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_index_writes_lookup_index() {
    let temp_dir = tempfile::tempdir().unwrap();
    let channel = temp_dir.path();
    populate(channel).await;

    // Without the option, nothing points to a lookup index.
    index_fs(FsConfigBuilder::new(channel, false, false).0)
        .await
        .unwrap();
    assert!(read_repodata(channel)["info"].get("lookup_url").is_none());
    assert!(!channel.join(SUBDIR).join("lookup").exists());

    // With it, a base layer with both packages is written and the repodata
    // (and the sharded index) point to the manifest.
    let stats = index(channel, true, false).await;
    let lookup_stats = stats.subdirs[&Platform::Win64].lookup.as_ref().unwrap();
    assert_eq!(lookup_stats.packages_added, 2);
    assert_eq!(lookup_stats.packages_removed, 0);
    assert_eq!(lookup_stats.layers, 1);

    let repodata = read_repodata(channel);
    assert_eq!(repodata["info"]["lookup_url"], "./lookup/manifest.json");
    let shards: ShardedRepodata = rmp_serde::from_slice(
        &zstd::decode_all(
            &fs::read(channel.join(SUBDIR).join("repodata_shards.msgpack.zst")).unwrap()[..],
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        shards.info.lookup_url.as_deref(),
        Some("./lookup/manifest.json")
    );

    let manifest = read_manifest(channel);
    assert_eq!(manifest.subdir, SUBDIR);
    assert!(manifest.channel.starts_with("file:///"));
    assert_eq!(manifest.kinds, ["paths", "reversed-paths"]);
    assert_eq!(manifest.layers.len(), 1);
    assert_eq!(manifest.layers[0].packages.count, 2);
    assert!(manifest.removed.is_empty());
    let lookup_dir = channel.join(SUBDIR).join("lookup");
    for file in manifest.files() {
        let bytes = fs::read(lookup_dir.join(file)).unwrap();
        bulk::verify_sha256(file, &bytes, &Location::from(lookup_dir.join(file))).unwrap();
    }

    // The paths table lists exactly the paths of `info/paths.json`.
    let layer = &manifest.layers[0];
    let packages = bulk::read_packages(
        fs::read(lookup_dir.join(&layer.packages.file))
            .unwrap()
            .into(),
        &Location::from(lookup_dir.join(&layer.packages.file)),
    )
    .await
    .unwrap();
    assert_eq!(packages, [CONDA_PACKAGE, TAR_BZ2_PACKAGE]);
    let table = &layer.tables["paths"];
    let rows = bulk::read_table(
        fs::read(lookup_dir.join(&table.file)).unwrap().into(),
        Kind::Paths,
        &Location::from(lookup_dir.join(&table.file)),
    )
    .await
    .unwrap();
    let tar_bz2_paths: BTreeSet<String> = rows
        .iter()
        .filter(|(_, ids)| ids.contains(&1))
        .map(|(path, _)| path.clone())
        .collect();
    let expected: Value = serde_json::from_slice(
        &fs::read(test_data_dir().join("conda-22.9.0-py38haa244fe_2-paths.json")).unwrap(),
    )
    .unwrap();
    let expected: BTreeSet<String> = expected["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["_path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(tar_bz2_paths, expected);

    // Queries through the index found via `lookup_url`.
    let mut lookup = open_index(channel).await;
    let matches = lookup
        .query(
            &Query::parse("Lib/site-packages/conda/__init__.py").unwrap(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        matches.filenames().into_iter().collect::<Vec<_>>(),
        [CONDA_PACKAGE, TAR_BZ2_PACKAGE]
    );
    let matches = lookup
        .query(
            &Query::parse("**/conda-22.9.0-py3.8.egg-info/PKG-INFO").unwrap(),
            Some(10),
        )
        .await
        .unwrap();
    assert_eq!(
        matches.paths.keys().collect::<Vec<_>>(),
        ["Lib/site-packages/conda-22.9.0-py3.8.egg-info/PKG-INFO"]
    );
    assert_eq!(
        matches.filenames().into_iter().collect::<Vec<_>>(),
        [TAR_BZ2_PACKAGE]
    );
    assert!(lookup.find("does/not/exist").await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_lookup_index_layers_removals_and_compaction() {
    let temp_dir = tempfile::tempdir().unwrap();
    let channel = temp_dir.path();
    populate(channel).await;
    let subdir = channel.join(SUBDIR);
    index(channel, true, false).await;
    let base = read_manifest(channel);

    // Re-indexing without changes adds no layer.
    let stats = index(channel, true, false).await;
    assert_eq!(
        stats.subdirs[&Platform::Win64]
            .lookup
            .as_ref()
            .unwrap()
            .packages_added,
        0
    );
    let manifest = read_manifest(channel);
    assert_eq!(manifest.layers, base.layers);

    // A deleted package is listed as removed and dropped from lookups.
    fs::remove_file(subdir.join(TAR_BZ2_PACKAGE)).unwrap();
    let stats = index(channel, true, false).await;
    assert_eq!(
        stats.subdirs[&Platform::Win64]
            .lookup
            .as_ref()
            .unwrap()
            .packages_removed,
        1
    );
    let manifest = read_manifest(channel);
    assert_eq!(manifest.layers, base.layers);
    assert_eq!(manifest.removed, [TAR_BZ2_PACKAGE]);
    let mut lookup = open_index(channel).await;
    let matches = lookup
        .find("Lib/site-packages/conda/__init__.py")
        .await
        .unwrap();
    assert_eq!(
        matches.filenames().into_iter().collect::<Vec<_>>(),
        [CONDA_PACKAGE]
    );

    // Re-uploading it takes it off the list without a new layer.
    fs::copy(
        tools::download_and_cache_file(
            format!("https://conda.anaconda.org/conda-forge/win-64/{TAR_BZ2_PACKAGE}")
                .parse()
                .unwrap(),
            "3c2c2e8e81bde5fb1ac4b014f51a62411feff004580c708c97a0ec2b7058cdc4",
        )
        .unwrap(),
        subdir.join(TAR_BZ2_PACKAGE),
    )
    .unwrap();
    index(channel, true, false).await;
    let manifest = read_manifest(channel);
    assert_eq!(manifest.layers, base.layers);
    assert!(manifest.removed.is_empty());

    // New packages go into delta layers, one per run, until the index is
    // compacted into a new base layer.
    let empty = test_data_dir().join("packages/empty-0.1.0-h4616a5c_0.conda");
    let mut extra = Vec::new();
    for i in 0..rattler_index::MAX_LOOKUP_LAYERS {
        let filename = format!("empty-0.1.{i}-h4616a5c_0.conda");
        fs::copy(&empty, subdir.join(&filename)).unwrap();
        extra.push(filename);
        let stats = index(channel, true, false).await;
        let lookup_stats = stats.subdirs[&Platform::Win64].lookup.as_ref().unwrap();
        assert_eq!(lookup_stats.packages_added, 1);
        let manifest = read_manifest(channel);
        if i + 1 < rattler_index::MAX_LOOKUP_LAYERS {
            assert!(!lookup_stats.compacted);
            assert_eq!(manifest.layers.len(), i + 2);
            assert_eq!(manifest.layers[0], base.layers[0]);
        } else {
            assert!(lookup_stats.compacted);
            assert_eq!(manifest.layers.len(), 1);
            assert_eq!(manifest.layers[0].packages.count, 2 + extra.len() as u64);
        }
        let mut lookup = open_index(channel).await;
        let matches = lookup.find("info/index.json").await.unwrap();
        assert!(
            matches.is_empty(),
            "info/ files are not part of a package's paths"
        );
        let matches = lookup
            .query(&Query::parse("**/conda/__init__.py").unwrap(), Some(10))
            .await
            .unwrap();
        assert_eq!(
            matches.filenames().into_iter().collect::<Vec<_>>(),
            [CONDA_PACKAGE, TAR_BZ2_PACKAGE]
        );
    }

    // A forced re-index writes a new base layer, without the deleted package.
    fs::remove_file(subdir.join(CONDA_PACKAGE)).unwrap();
    let stats = index(channel, true, true).await;
    let lookup_stats = stats.subdirs[&Platform::Win64].lookup.as_ref().unwrap();
    assert!(lookup_stats.compacted);
    let manifest = read_manifest(channel);
    assert_eq!(manifest.layers.len(), 1);
    assert_eq!(manifest.layers[0].packages.count, 1 + extra.len() as u64);
    assert!(manifest.removed.is_empty());
    let mut lookup = open_index(channel).await;
    let matches = lookup
        .find("Lib/site-packages/conda/__init__.py")
        .await
        .unwrap();
    assert_eq!(
        matches.filenames().into_iter().collect::<Vec<_>>(),
        [TAR_BZ2_PACKAGE]
    );
}
