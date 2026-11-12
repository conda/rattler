use std::{fs, fs::File, path::Path};

use rattler_conda_types::{Subdir, compression_level::CompressionLevel};
use rattler_index::{
    IndexFsConfig, PackageRevisionAssignment, index_fs, package_record_from_archive,
};
use rattler_package_streaming::write::{write_conda_package, write_tar_bz2_package};
use serde_json::{Value, json};

const PUBLISHED: i64 = 1_700_000_000_000;

/// Writes `noarch/<name>-1.0-0.<ext>` with the given dependencies and payload,
/// so the same filename can be rewritten with different contents.
fn write_package(root: &Path, name: &str, ext: &str, depends: &[&str], payload: &str) -> String {
    let source = tempfile::tempdir().unwrap();
    fs::create_dir_all(source.path().join("info")).unwrap();
    let index = source.path().join("info/index.json");
    fs::write(
        &index,
        serde_json::to_vec(&json!({
            "name": name, "version": "1.0", "build": "0", "build_number": 0,
            "subdir": "noarch", "depends": depends
        }))
        .unwrap(),
    )
    .unwrap();
    let data = source.path().join("payload.txt");
    fs::write(&data, payload).unwrap();
    let files = vec![index, data];

    let stem = format!("{name}-1.0-0");
    let filename = format!("{stem}.{ext}");
    fs::create_dir_all(root.join("noarch")).unwrap();
    let output = File::create(root.join("noarch").join(&filename)).unwrap();
    if ext == "conda" {
        write_conda_package(
            output,
            source.path(),
            &files,
            CompressionLevel::Default,
            None,
            &stem,
            None,
            None,
        )
        .unwrap();
    } else {
        write_tar_bz2_package(
            output,
            source.path(),
            &files,
            CompressionLevel::Default,
            None,
            None,
        )
        .unwrap();
    }
    filename
}

async fn index(root: &Path, force: bool) -> Value {
    index_fs(IndexFsConfig {
        channel: root.into(),
        target_platform: Some(Subdir::NoArch),
        repodata_patch: None,
        write_zst: false,
        write_shards: false,
        repodata_revisions: Vec::new(),
        package_revision_assignment: PackageRevisionAssignment::default(),
        force,
        max_parallel: 1,
        multi_progress: None,
    })
    .await
    .unwrap();
    serde_json::from_reader(File::open(root.join("noarch/repodata.json")).unwrap()).unwrap()
}

fn key(filename: &str) -> &'static str {
    if filename.ends_with(".conda") {
        "packages.conda"
    } else {
        "packages"
    }
}

/// Overwriting a package with a different file under the same name used to
/// leave the old hashes in repodata.json, so installs failed the hash check.
#[tokio::test]
async fn reindex_picks_up_a_package_replaced_under_the_same_name() {
    for ext in ["conda", "tar.bz2"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let filename = write_package(root, "example", ext, &["a"], "first build");
        let path = root.join("noarch").join(&filename);
        let old = package_record_from_archive(&path).unwrap();
        index(root, false).await;

        write_package(
            root,
            "example",
            ext,
            &["a", "b"],
            "second build, a bit longer",
        );
        let new = package_record_from_archive(&path).unwrap();
        assert_ne!(old.size, new.size, "the rebuild must change the size");

        let repodata = index(root, false).await;
        let record = &repodata[key(&filename)][&filename];
        let expected = serde_json::to_value(&new).unwrap();
        for field in ["sha256", "md5", "size"] {
            assert_eq!(record[field], expected[field], "{ext}: stale {field}");
        }
        assert_eq!(record["depends"], json!(["a", "b"]));
    }
}

/// A replaced package counts as published again, while packages that didn't
/// change keep their indexed timestamp. This holds with and without `force`.
#[tokio::test]
async fn replaced_package_gets_a_new_indexed_timestamp() {
    for force in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let replaced = write_package(root, "replaced", "conda", &[], "first build");
        let untouched = write_package(root, "untouched", "conda", &[], "only build");

        // Simulate both packages being published a while ago.
        let mut repodata = index(root, false).await;
        for filename in [&replaced, &untouched] {
            repodata["packages.conda"][filename]["indexed_timestamp"] = json!(PUBLISHED);
        }
        fs::write(
            root.join("noarch/repodata.json"),
            serde_json::to_vec(&repodata).unwrap(),
        )
        .unwrap();

        write_package(
            root,
            "replaced",
            "conda",
            &["a"],
            "second build, a bit longer",
        );
        let before = jiff::Timestamp::now().as_millisecond();
        let repodata = index(root, force).await;
        let after = jiff::Timestamp::now().as_millisecond();

        let timestamp = repodata["packages.conda"][&replaced]["indexed_timestamp"]
            .as_i64()
            .unwrap();
        assert!(
            (before..=after).contains(&timestamp),
            "force={force}: replaced package kept {timestamp}"
        );
        assert_eq!(
            repodata["packages.conda"][&untouched]["indexed_timestamp"],
            json!(PUBLISHED),
            "force={force}"
        );
    }
}
