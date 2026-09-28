//! End-to-end tests: write an index, then look paths up in it, on disk and over
//! HTTP range requests.

mod test_server;

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use rattler_conda_types::Platform;
use rattler_lookup::{
    Kind, LayerBuilder, LookupError, Manifest, PathLookup, Query, Search, SubdirPathLookup,
    WriteOptions,
    manifest::{LOOKUP_DIR, MANIFEST_FILE},
};
use reqwest_middleware::ClientWithMiddleware;
use tempfile::TempDir;
use url::Url;

const CHANNEL: &str = "https://example.org/channel/";

fn client() -> ClientWithMiddleware {
    reqwest::Client::new().into()
}

/// The directory of the index of `subdir` below `root`.
fn index_dir(root: &Path, subdir: &str) -> PathBuf {
    root.join(subdir).join(LOOKUP_DIR)
}

/// Writes one layer with the given artifacts and their paths into the index of
/// `subdir`, appending it to the manifest.
fn add_layer(root: &Path, subdir: &str, packages: &[(&str, &[&str])]) {
    add_layer_with_kinds(root, subdir, packages, &Kind::ALL);
}

/// The same, for an index that only has the given kinds of lookup tables.
fn add_layer_with_kinds(root: &Path, subdir: &str, packages: &[(&str, &[&str])], kinds: &[Kind]) {
    let dir = index_dir(root, subdir);
    fs::create_dir_all(&dir).unwrap();

    let mut builder = LayerBuilder::new(CHANNEL, subdir);
    for (file_name, paths) in packages {
        builder.add_package(
            (*file_name).to_string(),
            paths.iter().map(|path| (*path).to_string()),
        );
    }
    let layer = builder
        .finish_with_kinds(kinds, &WriteOptions::default())
        .unwrap()
        .expect("a layer");
    for file in layer.files() {
        fs::write(dir.join(&file.file_name), &file.bytes).unwrap();
    }

    let mut manifest = read_manifest(root, subdir)
        .unwrap_or_else(|| Manifest::empty(CHANNEL, subdir, kinds.iter().copied()));
    manifest.layers.push(layer.manifest_layer());
    write_manifest(root, subdir, &manifest);
}

fn read_manifest(root: &Path, subdir: &str) -> Option<Manifest> {
    let bytes = fs::read(index_dir(root, subdir).join(MANIFEST_FILE)).ok()?;
    Some(Manifest::from_bytes(&bytes).unwrap())
}

fn write_manifest(root: &Path, subdir: &str, manifest: &Manifest) {
    fs::write(
        index_dir(root, subdir).join(MANIFEST_FILE),
        manifest.to_bytes().unwrap(),
    )
    .unwrap();
}

/// An index of `noarch` with two layers.
fn index() -> TempDir {
    let root = tempfile::tempdir().unwrap();
    add_layer(
        root.path(),
        "noarch",
        &[
            ("a-1.0-0.conda", &["bin/a", "lib/shared", "share/a/data"]),
            ("b-1.0-0.conda", &["bin/b", "lib/shared"]),
            ("empty-1.0-0.conda", &[]),
        ],
    );
    add_layer(
        root.path(),
        "noarch",
        &[("c-2.0-0.conda", &["bin/c", "lib/shared"])],
    );
    root
}

async fn open(base: &Url) -> PathLookup {
    PathLookup::for_index_base(base, &[Platform::NoArch], client())
        .await
        .unwrap()
}

fn file_names(matches: &[rattler_lookup::PathMatch]) -> Vec<&str> {
    matches
        .iter()
        .map(|found| found.file_name.as_str())
        .collect()
}

/// The paths a search found, each with the artifacts that contain it.
fn found(search: &Search) -> Vec<(&str, Vec<&str>)> {
    search
        .paths
        .iter()
        .map(|(path, artifacts)| (path.as_str(), file_names(artifacts)))
        .collect()
}

async fn search(lookup: &PathLookup, pattern: &str, limit: usize) -> Search {
    let query = Query::parse(pattern).unwrap();
    lookup.search(&query, limit).await.unwrap()
}

#[tokio::test]
async fn finds_paths_on_disk() {
    let root = index();
    let lookup = open(&Url::from_directory_path(root.path()).unwrap()).await;
    assert_eq!(lookup.subdirs().len(), 1);
    assert_eq!(lookup.subdirs()[0].num_layers(), 2);
    assert_eq!(lookup.subdirs()[0].channel(), CHANNEL);

    // A path in one artifact, in several artifacts across layers, and in none.
    assert_eq!(
        file_names(&lookup.find("bin/a").await.unwrap()),
        ["a-1.0-0.conda"]
    );
    assert_eq!(
        file_names(&lookup.find("lib/shared").await.unwrap()),
        ["a-1.0-0.conda", "b-1.0-0.conda", "c-2.0-0.conda"]
    );
    assert!(lookup.find("bin/does-not-exist").await.unwrap().is_empty());
    // Paths just outside of the ones that exist, to catch off-by-one page bounds.
    assert!(lookup.find("bin/").await.unwrap().is_empty());
    assert!(lookup.find("bin/a2").await.unwrap().is_empty());
    assert!(lookup.find("zzz").await.unwrap().is_empty());

    // A leading `./` or `/` is not part of the stored path.
    assert_eq!(
        file_names(&lookup.find("./bin/a").await.unwrap()),
        ["a-1.0-0.conda"]
    );
    assert_eq!(
        file_names(&lookup.find("/bin/a").await.unwrap()),
        ["a-1.0-0.conda"]
    );

    let found = &lookup.find("bin/a").await.unwrap()[0];
    assert_eq!(found.subdir, "noarch");
    assert_eq!(found.package_name(), "a");
    assert_eq!(
        found.url().unwrap().as_str(),
        "https://example.org/channel/noarch/a-1.0-0.conda"
    );
}

#[tokio::test]
async fn finds_patterns_on_disk() {
    let root = index();
    let lookup = open(&Url::from_directory_path(root.path()).unwrap()).await;

    // A prefix scan of the `paths` table, over both layers.
    let result = search(&lookup, "bin/*", 100).await;
    assert!(!result.truncated);
    assert_eq!(
        found(&result),
        [
            ("bin/a", vec!["a-1.0-0.conda"]),
            ("bin/b", vec!["b-1.0-0.conda"]),
            ("bin/c", vec!["c-2.0-0.conda"]),
        ]
    );

    // A prefix scan of the `reversed-paths` table.
    let result = search(&lookup, "**/shared", 100).await;
    assert_eq!(
        found(&result),
        [(
            "lib/shared",
            vec!["a-1.0-0.conda", "b-1.0-0.conda", "c-2.0-0.conda"]
        )]
    );
    // `**/a` is the last component, so `share/a/data` does not match.
    assert_eq!(
        found(&search(&lookup, "**/a", 100).await),
        [("bin/a", vec!["a-1.0-0.conda"])]
    );
    assert_eq!(
        found(&search(&lookup, "**/a/data", 100).await),
        [("share/a/data", vec!["a-1.0-0.conda"])]
    );
    assert_eq!(
        found(&search(&lookup, "share/**", 100).await),
        [("share/a/data", vec!["a-1.0-0.conda"])]
    );
    assert!(search(&lookup, "bin/a*b", 100).await.paths.is_empty());

    // Every artifact of every matching path, deduplicated.
    let result = search(&lookup, "**/s*", 100).await;
    assert_eq!(
        file_names(&result.artifacts()),
        ["a-1.0-0.conda", "b-1.0-0.conda", "c-2.0-0.conda"]
    );
}

#[tokio::test]
async fn a_search_stops_at_its_limit() {
    let root = index();
    let lookup = open(&Url::from_directory_path(root.path()).unwrap()).await;

    let result = search(&lookup, "bin/*", 2).await;
    assert!(result.truncated, "there is a third match");
    assert_eq!(result.paths.len(), 2);

    let result = search(&lookup, "bin/*", 3).await;
    assert!(!result.truncated);
    assert_eq!(result.paths.len(), 3);
}

#[tokio::test]
async fn a_pattern_without_a_literal_start_is_rejected() {
    // Answering it would mean reading the whole index.
    for pattern in ["*", "*/a", "**/*.h"] {
        assert!(
            matches!(Query::parse(pattern), Err(LookupError::InvalidQuery { .. })),
            "{pattern} should be rejected"
        );
    }
}

#[tokio::test]
async fn a_missing_kind_is_not_an_empty_result() {
    let root = tempfile::tempdir().unwrap();
    add_layer_with_kinds(
        root.path(),
        "noarch",
        &[("a-1.0-0.conda", &["bin/a"])],
        &[Kind::Paths],
    );
    let lookup = open(&Url::from_directory_path(root.path()).unwrap()).await;

    // The index can answer what it has a table for.
    assert_eq!(
        file_names(&lookup.find("bin/a").await.unwrap()),
        ["a-1.0-0.conda"]
    );
    assert_eq!(found(&search(&lookup, "bin/*", 10).await).len(), 1);

    // But a `**/…` pattern needs the `reversed-paths` table.
    assert!(lookup.missing_kinds(Kind::ReversedPaths));
    let query = Query::parse("**/a").unwrap();
    let error = lookup
        .search(&query, 10)
        .await
        .map(|_| ())
        .expect_err("the index has no reversed-paths table");
    assert!(
        matches!(error, LookupError::KindUnavailable(Kind::ReversedPaths)),
        "{error:?}"
    );
}

#[tokio::test]
async fn removed_artifacts_are_not_reported() {
    let root = index();
    let mut manifest = read_manifest(root.path(), "noarch").unwrap();
    manifest.removed = BTreeSet::from(["b-1.0-0.conda".to_string()]);
    write_manifest(root.path(), "noarch", &manifest);

    let lookup = open(&Url::from_directory_path(root.path()).unwrap()).await;
    assert_eq!(
        file_names(&lookup.find("lib/shared").await.unwrap()),
        ["a-1.0-0.conda", "c-2.0-0.conda"]
    );
    assert!(lookup.find("bin/b").await.unwrap().is_empty());
    // A path that only a removed artifact contains is no match at all.
    assert!(search(&lookup, "bin/b*", 10).await.paths.is_empty());
}

#[tokio::test]
async fn a_subdir_without_a_manifest_is_not_indexed() {
    let root = index();
    let lookup = PathLookup::for_index_base(
        &Url::from_directory_path(root.path()).unwrap(),
        &[Platform::Linux64, Platform::NoArch],
        client(),
    )
    .await
    .unwrap();
    assert_eq!(lookup.subdirs().len(), 1, "only noarch is indexed");

    let empty = tempfile::tempdir().unwrap();
    let lookup = open(&Url::from_directory_path(empty.path()).unwrap()).await;
    assert!(lookup.is_empty());
    assert!(lookup.find("bin/a").await.unwrap().is_empty());
    // Without an index there is nothing to report a missing kind about.
    assert!(!lookup.missing_kinds(Kind::ReversedPaths));
}

#[tokio::test]
async fn a_layer_of_the_wrong_size_is_rejected() {
    let root = index();
    let mut manifest = read_manifest(root.path(), "noarch").unwrap();
    manifest.layers[0]
        .tables
        .get_mut(Kind::Paths.name())
        .unwrap()
        .size += 1;
    write_manifest(root.path(), "noarch", &manifest);

    let url = Url::from_directory_path(root.path()).unwrap();
    let lookup = open(&url).await;
    let error = lookup
        .find("bin/a")
        .await
        .map(|_| ())
        .expect_err("the manifest does not match the layer");
    assert!(
        matches!(error, LookupError::SizeMismatch { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_manifest_of_an_unknown_version_is_rejected() {
    let root = index();
    let path = index_dir(root.path(), "noarch").join(MANIFEST_FILE);
    let manifest = fs::read_to_string(&path)
        .unwrap()
        .replace("\"version\": 1", "\"version\": 2");
    fs::write(&path, manifest).unwrap();

    let url = Url::from_directory_path(root.path()).unwrap();
    let error = PathLookup::for_index_base(&url, &[Platform::NoArch], client())
        .await
        .map(|_| ())
        .expect_err("the manifest version is not supported");
    assert!(
        matches!(error, LookupError::InvalidManifest { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn finds_paths_over_http() {
    let root = index();
    let base = test_server::serve_dir(root.path()).await;
    let lookup = open(&base).await;

    assert_eq!(
        file_names(&lookup.find("lib/shared").await.unwrap()),
        ["a-1.0-0.conda", "b-1.0-0.conda", "c-2.0-0.conda"]
    );
    assert!(lookup.find("bin/does-not-exist").await.unwrap().is_empty());

    let stats = lookup.stats();
    assert!(stats.requests > 0, "nothing was requested");
    assert!(
        stats.bytes < 1 << 20,
        "a lookup should read a few kilobytes, not {} bytes",
        stats.bytes
    );
}

/// A lookup only reads the table of the kind it needs, so an index with many
/// kinds does not make an exact lookup more expensive.
#[tokio::test]
async fn a_lookup_only_opens_the_table_it_needs() {
    let root = index();
    // The manifest still lists the kind, but its tables are gone.
    let dir = index_dir(root.path(), "noarch");
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("reversed-paths-"))
        {
            fs::remove_file(path).unwrap();
        }
    }

    let lookup = open(&Url::from_directory_path(root.path()).unwrap()).await;
    assert_eq!(
        file_names(&lookup.find("bin/a").await.unwrap()),
        ["a-1.0-0.conda"],
        "an exact lookup is answered by the `paths` table alone"
    );
    let query = Query::parse("**/a").unwrap();
    assert!(
        lookup.search(&query, 10).await.is_err(),
        "the table is gone"
    );
}

/// Servers like `JFrog` Artifactory answer suffix ranges that exceed the file
/// size with `416`; the reader falls back to a `HEAD` request.
#[tokio::test]
async fn finds_paths_over_http_without_suffix_ranges() {
    let root = index();
    let base = test_server::serve_dir_without_suffix_ranges(root.path()).await;
    let lookup = open(&base).await;
    assert_eq!(
        file_names(&lookup.find("bin/c").await.unwrap()),
        ["c-2.0-0.conda"]
    );
}

#[tokio::test]
async fn discovers_the_index_through_the_repodata() {
    let root = index();
    fs::write(
        root.path().join("noarch").join("repodata.json"),
        br#"{"info":{"subdir":"noarch","lookup_url":"./lookup/manifest.json"},
            "packages":{},"packages.conda":{},"repodata_version":2}"#,
    )
    .unwrap();
    let channel = test_server::serve_dir(root.path()).await;

    let lookup = PathLookup::for_channel(&channel, &[Platform::NoArch], client())
        .await
        .unwrap();
    assert_eq!(
        file_names(&lookup.find("bin/a").await.unwrap()),
        ["a-1.0-0.conda"]
    );
}

#[tokio::test]
async fn a_channel_without_the_field_has_no_index() {
    let root = index();
    fs::write(
        root.path().join("noarch").join("repodata.json"),
        br#"{"info":{"subdir":"noarch"},"packages":{},"repodata_version":2}"#,
    )
    .unwrap();
    let channel = test_server::serve_dir(root.path()).await;

    let lookup = PathLookup::for_channel(&channel, &[Platform::NoArch], client())
        .await
        .unwrap();
    assert!(lookup.is_empty(), "the channel advertises no lookup index");
}

#[tokio::test]
async fn opens_a_manifest_directly() {
    let root = index();
    let url = Url::from_file_path(index_dir(root.path(), "noarch").join(MANIFEST_FILE)).unwrap();
    let lookup = SubdirPathLookup::open(&url, &client())
        .await
        .unwrap()
        .expect("a manifest");
    assert_eq!(lookup.subdir(), "noarch");
    assert!(lookup.has_kind(Kind::ReversedPaths));
    assert_eq!(lookup.find("bin/b").await.unwrap(), ["b-1.0-0.conda"]);
}
