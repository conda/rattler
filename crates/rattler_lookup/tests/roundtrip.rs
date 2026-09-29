//! Writes an index and queries it back: from local files, over HTTP range
//! requests (with a server that rejects oversized suffix ranges, like
//! `tower_http::services::ServeDir` does, and one that clamps them), and
//! through `lookup_url` discovery.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    path::{Path, PathBuf},
};

use axum::{
    extract::{Request, State},
    middleware::{self, Next},
    response::Response,
};
use rattler_lookup::{
    Kind, Location, LookupError, Manifest, Query, SubdirIndex, WriteOptions, bulk, discovery,
    manifest::{LOOKUP_DIR, MANIFEST_FILE, manifest_location},
    write_layer,
};
use reqwest_middleware::ClientWithMiddleware;
use tower_http::services::ServeDir;
use url::Url;

const CHANNEL: &str = "https://conda.anaconda.org/conda-forge/";

fn client() -> ClientWithMiddleware {
    reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build()
}

/// Small pages and row groups so that even a small index spans several of
/// both.
fn options() -> WriteOptions {
    WriteOptions {
        compression_level: 3,
        data_page_size: 2048,
        page_rows: 32,
        row_group_size: 256,
    }
}

/// The artifacts of the base layer: many builds of a few packages, so that
/// common paths link to many artifacts and the tables have many rows.
fn base_artifacts() -> Vec<(String, Vec<String>)> {
    let mut artifacts = Vec::new();
    for build in 0..200 {
        artifacts.push((
            format!("zlib-1.3.{build}-h{build:03}_0.conda"),
            vec![
                "include/zlib.h".into(),
                "include/zconf.h".into(),
                format!("lib/libz.so.1.3.{build}"),
                "lib/libz.so".into(),
                "lib/pkgconfig/zlib.pc".into(),
            ],
        ));
        artifacts.push((
            format!("python-3.{build}.0-h{build:03}_0_cpython.conda"),
            vec![
                "bin/python".into(),
                format!("bin/python3.{build}"),
                format!("lib/python3.{build}/site-packages/README.txt"),
                format!("lib/python3.{build}/os.py"),
                "LICENSE".into(),
            ],
        ));
    }
    artifacts.push((
        "mariadb-connector-c-3.4.1-h1234_0.tar.bz2".into(),
        vec!["include/zlib.h".into(), "lib/libmariadb.so.3".into()],
    ));
    artifacts.push((
        "polars-1.44.2-pyh3138b34_0.conda".into(),
        vec![
            "site-packages/polars/__init__.py".into(),
            "site-packages/polars/io/__init__.py".into(),
            "site-packages/polars-1.44.2.dist-info/LICENSE".into(),
        ],
    ));
    artifacts
}

/// Writes a base layer plus a delta layer for `noarch` into
/// `<root>/noarch/lookup/`, with one artifact of the base layer removed, and
/// returns the manifest.
fn write_index(root: &Path) -> Manifest {
    let dir = root.join("noarch").join(LOOKUP_DIR);
    let mut manifest = Manifest::empty(CHANNEL, "noarch", &Kind::ALL);
    let base = write_layer(
        &dir,
        CHANNEL,
        "noarch",
        &Kind::ALL,
        base_artifacts(),
        &options(),
    )
    .unwrap();
    assert_eq!(base.files.len(), 3);
    assert_eq!(base.layer.packages.count, 402);
    manifest.layers.push(base.layer);

    let delta = write_layer(
        &dir,
        CHANNEL,
        "noarch",
        &Kind::ALL,
        vec![
            (
                "zlib-1.4.0-hnew_0.conda".into(),
                vec!["include/zlib.h".into(), "lib/libz.so.1.4.0".into()],
            ),
            (
                "numpy-2.0.0-py313_0.conda".into(),
                vec![
                    "site-packages/numpy/__init__.py".into(),
                    "site-packages/numpy/LICENSE.txt".into(),
                ],
            ),
        ],
        &options(),
    )
    .unwrap();
    manifest.layers.push(delta.layer);
    manifest.removed = vec!["zlib-1.3.7-h007_0.conda".into()];

    std::fs::write(dir.join(MANIFEST_FILE), manifest.to_json().unwrap()).unwrap();
    manifest
}

fn filenames(matches: &rattler_lookup::Matches, path: &str) -> Vec<String> {
    matches.paths[path].iter().cloned().collect()
}

async fn check_queries(index: &mut SubdirIndex) {
    // An exact path across both layers, minus the removed artifact.
    let matches = index
        .query(&Query::parse("include/zlib.h").unwrap(), None)
        .await
        .unwrap();
    assert_eq!(matches.paths.len(), 1);
    let found = filenames(&matches, "include/zlib.h");
    assert_eq!(found.len(), 200 + 1 + 1 - 1);
    assert!(found.contains(&"zlib-1.4.0-hnew_0.conda".to_string()));
    assert!(found.contains(&"mariadb-connector-c-3.4.1-h1234_0.tar.bz2".to_string()));
    assert!(!found.contains(&"zlib-1.3.7-h007_0.conda".to_string()));
    assert!(!matches.truncated);

    // A path that only one artifact has.
    let matches = index.find("lib/libz.so.1.3.42").await.unwrap();
    assert_eq!(
        filenames(&matches, "lib/libz.so.1.3.42"),
        ["zlib-1.3.42-h042_0.conda"]
    );

    // A path whose only artifact was removed, and a path that doesn't exist.
    assert!(index.find("lib/libz.so.1.3.7").await.unwrap().is_empty());
    assert!(index.find("does/not/exist").await.unwrap().is_empty());
    assert!(index.find("include/zlib").await.unwrap().is_empty());

    // `**/` patterns use the reversed-paths tables.
    let matches = index
        .query(&Query::parse("**/__init__.py").unwrap(), Some(1000))
        .await
        .unwrap();
    let paths: Vec<&String> = matches.paths.keys().collect();
    assert_eq!(
        paths,
        [
            "site-packages/numpy/__init__.py",
            "site-packages/polars/__init__.py",
            "site-packages/polars/io/__init__.py",
        ]
    );
    assert!(!matches.truncated);

    let matches = index
        .query(&Query::parse("**/LICENSE*").unwrap(), Some(1000))
        .await
        .unwrap();
    assert_eq!(
        matches.paths.keys().collect::<Vec<_>>(),
        [
            "LICENSE",
            "site-packages/numpy/LICENSE.txt",
            "site-packages/polars-1.44.2.dist-info/LICENSE",
        ]
    );
    assert_eq!(matches.paths["LICENSE"].len(), 200);

    let matches = index
        .query(&Query::parse("**/libz.so*").unwrap(), Some(1000))
        .await
        .unwrap();
    // `lib/libz.so.1.3.7` is dropped: its only artifact was removed.
    assert_eq!(matches.paths.len(), 1 + 200 + 1 - 1);
    assert_eq!(matches.filenames().len(), 200 + 1 - 1);

    // A limit stops the scan early.
    let matches = index
        .query(&Query::parse("**/libz.so*").unwrap(), Some(5))
        .await
        .unwrap();
    assert_eq!(matches.paths.len(), 5);
    assert!(matches.truncated);

    // A prefix pattern uses the paths tables.
    let matches = index
        .query(&Query::parse("site-packages/polars/*").unwrap(), Some(1000))
        .await
        .unwrap();
    assert_eq!(
        matches.paths.keys().collect::<Vec<_>>(),
        ["site-packages/polars/__init__.py"]
    );
    let matches = index
        .query(&Query::parse("bin/python3.1?").unwrap(), Some(1000))
        .await
        .unwrap();
    assert_eq!(matches.paths.len(), 10);
}

#[tokio::test]
async fn local_roundtrip() {
    let root = tempfile::tempdir().unwrap();
    let manifest = write_index(root.path());
    let location = manifest_location(&Location::from(root.path()), "noarch");
    let mut index = SubdirIndex::open(location, &Kind::ALL, &client())
        .await
        .unwrap();
    assert_eq!(index.manifest(), &manifest);
    assert_eq!(index.subdir(), "noarch");
    assert_eq!(index.channel(), CHANNEL);
    check_queries(&mut index).await;

    // Only the kinds that are needed are opened.
    let mut index = SubdirIndex::open(index.location().clone(), &[Kind::Paths], &client())
        .await
        .unwrap();
    assert!(!index.find("bin/python").await.unwrap().is_empty());
    let err = index
        .query(&Query::parse("**/zlib.h").unwrap(), None)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        LookupError::MissingKind {
            kind: Kind::ReversedPaths,
            ..
        }
    ));
}

#[tokio::test]
async fn rejects_tampered_indexes() {
    let root = tempfile::tempdir().unwrap();
    let mut manifest = write_index(root.path());
    let location = manifest_location(&Location::from(root.path()), "noarch");

    // A missing kind.
    manifest.kinds.retain(|kind| kind == "paths");
    let err = SubdirIndex::from_manifest(location.clone(), manifest.clone(), &Kind::ALL, &client())
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        LookupError::MissingKind {
            kind: Kind::ReversedPaths,
            ..
        }
    ));

    // A wrong size.
    manifest.kinds = vec!["paths".into(), "reversed-paths".into()];
    manifest.layers[0].packages.size += 1;
    let err = SubdirIndex::from_manifest(location.clone(), manifest.clone(), &Kind::ALL, &client())
        .await
        .unwrap_err();
    assert!(matches!(err, LookupError::SizeMismatch { .. }));

    // A table listed as the wrong kind.
    manifest.layers[0].packages.size -= 1;
    let paths = manifest.layers[0].tables["paths"].clone();
    manifest.layers[0]
        .tables
        .insert("reversed-paths".into(), paths);
    let err = SubdirIndex::from_manifest(location.clone(), manifest, &Kind::ALL, &client())
        .await
        .unwrap_err();
    assert!(matches!(err, LookupError::InvalidFile { .. }));

    // No index at all.
    let err = SubdirIndex::open(
        manifest_location(&Location::from(root.path()), "linux-64"),
        &Kind::ALL,
        &client(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, LookupError::NotFound(_)));
}

#[tokio::test]
async fn bulk_reads_match_the_input() {
    let root = tempfile::tempdir().unwrap();
    let manifest = write_index(root.path());
    let dir = root.path().join("noarch").join(LOOKUP_DIR);
    let layer = &manifest.layers[0];

    let bytes = std::fs::read(dir.join(&layer.packages.file)).unwrap();
    let location = Location::from(dir.join(&layer.packages.file));
    bulk::verify_sha256(&layer.packages.file, &bytes, &location).unwrap();
    assert!(matches!(
        bulk::verify_sha256(&layer.packages.file, b"other", &location),
        Err(LookupError::DigestMismatch { .. })
    ));
    let packages = bulk::read_packages(bytes.into(), &location).await.unwrap();
    let mut expected: Vec<String> = base_artifacts().into_iter().map(|(f, _)| f).collect();
    expected.sort();
    assert_eq!(packages, expected);

    let table = &layer.tables["paths"];
    let bytes = std::fs::read(dir.join(&table.file)).unwrap();
    let location = Location::from(dir.join(&table.file));
    let rows = bulk::read_table(bytes.clone().into(), Kind::Paths, &location)
        .await
        .unwrap();
    let mut expected: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for (filename, paths) in base_artifacts() {
        let id = packages.binary_search(&filename).unwrap() as u32;
        for path in paths {
            expected.entry(path).or_default().insert(id);
        }
    }
    let expected: Vec<(String, Vec<u32>)> = expected
        .into_iter()
        .map(|(path, ids)| (path, ids.into_iter().collect()))
        .collect();
    assert_eq!(rows, expected);
    assert!(matches!(
        bulk::read_table(bytes.into(), Kind::ReversedPaths, &location).await,
        Err(LookupError::InvalidFile { .. })
    ));
}

/// Serves `dir` on a random port. `ServeDir` answers a suffix range that
/// exceeds the file size with `416 Range Not Satisfiable`, which exercises
/// the fallback to a HEAD request; `clamp` makes it answer with the whole
/// file instead, like most CDNs.
async fn serve(dir: PathBuf, clamp: bool) -> Url {
    let mut app = axum::Router::new().fallback_service(ServeDir::new(dir));
    if clamp {
        app = app.layer(middleware::from_fn_with_state((), clamp_suffix_range));
    }
    let addr = SocketAddr::new([127, 0, 0, 1].into(), 0);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{}:{}/", addr.ip(), addr.port())
        .parse()
        .unwrap()
}

async fn clamp_suffix_range(State(()): State<()>, mut req: Request, next: Next) -> Response {
    // Every file here is smaller than the 64 KiB tail request: answer with
    // the whole file (`206` with its full range), as RFC 9110 suggests.
    if let Some(range) = req.headers().get(http::header::RANGE)
        && range.to_str().is_ok_and(|r| r.starts_with("bytes=-"))
    {
        req.headers_mut()
            .insert(http::header::RANGE, "bytes=0-".parse().unwrap());
    }
    next.run(req).await
}

#[tokio::test]
async fn http_roundtrip() {
    let root = tempfile::tempdir().unwrap();
    let manifest = write_index(root.path());
    for clamp in [false, true] {
        let base = serve(root.path().to_path_buf(), clamp).await;
        let location = manifest_location(&Location::from(base), "noarch");
        let mut index = SubdirIndex::open(location, &Kind::ALL, &client())
            .await
            .unwrap();
        assert_eq!(index.manifest(), &manifest);
        check_queries(&mut index).await;
        let (requests, bytes) = index.stats();
        assert!(requests > 0 && bytes > 0);
    }
}

fn write_repodata(root: &Path, subdir: &str, lookup_url: Option<&str>) {
    let dir = root.join(subdir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut info = serde_json::json!({ "subdir": subdir });
    if let Some(url) = lookup_url {
        info["lookup_url"] = url.into();
    }
    let repodata = serde_json::json!({ "info": info, "packages": {}, "repodata_version": 2 });
    std::fs::write(
        dir.join("repodata.json"),
        serde_json::to_vec(&repodata).unwrap(),
    )
    .unwrap();
}

fn write_shards(root: &Path, subdir: &str, lookup_url: &str) {
    let shards = serde_json::json!({
        "info": {
            "subdir": subdir,
            "base_url": "",
            "shards_base_url": "./shards/",
            "lookup_url": lookup_url,
        },
        "shards": {},
    });
    let msgpack = rmp_serde::to_vec_named(&shards).unwrap();
    let encoded = zstd::stream::encode_all(&msgpack[..], 0).unwrap();
    std::fs::write(
        root.join(subdir).join("repodata_shards.msgpack.zst"),
        encoded,
    )
    .unwrap();
}

#[tokio::test]
async fn discovers_manifests_through_lookup_url() {
    let root = tempfile::tempdir().unwrap();
    write_index(root.path());
    write_repodata(root.path(), "noarch", Some("./lookup/manifest.json"));
    write_repodata(root.path(), "linux-64", None);
    write_repodata(
        root.path(),
        "osx-64",
        Some("https://example.org/idx/osx-64/lookup/manifest.json"),
    );
    std::fs::create_dir_all(root.path().join("win-64")).unwrap();
    write_shards(root.path(), "win-64", "./lookup/manifest.json");

    let base = serve(root.path().to_path_buf(), true).await;
    for channel in [Location::from(root.path()), Location::from(base)] {
        let found = discovery::discover_manifest(&channel, "noarch", &client())
            .await
            .unwrap()
            .expect("noarch has a lookup index");
        assert_eq!(found, manifest_location(&channel, "noarch"));
        let mut index = SubdirIndex::open(found, &Kind::ALL, &client())
            .await
            .unwrap();
        assert!(!index.find("bin/python").await.unwrap().is_empty());

        assert_eq!(
            discovery::discover_manifest(&channel, "linux-64", &client())
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            discovery::discover_manifest(&channel, "osx-arm64", &client())
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            discovery::discover_manifest(&channel, "osx-64", &client())
                .await
                .unwrap(),
            Some(Location::parse(
                "https://example.org/idx/osx-64/lookup/manifest.json"
            ))
        );
        assert_eq!(
            discovery::discover_manifest(&channel, "win-64", &client())
                .await
                .unwrap(),
            Some(manifest_location(&channel, "win-64"))
        );
    }
}
