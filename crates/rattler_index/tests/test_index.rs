use rattler_conda_types::Platform;
use rattler_index::index;
use serde_json::Value;
use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};

fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
}

#[test]
fn test_index() {
    let temp_dir = tempfile::tempdir().unwrap();
    let subdir_path = Path::new("win-64");
    let conda_file_path = Path::new("conda-22.11.1-py38haa244fe_1.conda");
    let index_json_path = Path::new("conda-22.11.1-py38haa244fe_1-index.json");
    let tar_bz2_file_path = Path::new("conda-22.9.0-py38haa244fe_2.tar.bz2");

    fs::create_dir(temp_dir.path().join(subdir_path)).unwrap();
    fs::copy(
        test_data_dir().join(conda_file_path),
        temp_dir.path().join(subdir_path).join(conda_file_path),
    )
    .unwrap();
    fs::copy(
        test_data_dir().join(tar_bz2_file_path),
        temp_dir.path().join(subdir_path).join(tar_bz2_file_path),
    )
    .unwrap();

    let res = index(temp_dir.path(), Some(&Platform::Win64));
    assert!(res.is_ok());

    let repodata_path = temp_dir.path().join(subdir_path).join("repodata.json");
    let repodata_json: Value = serde_json::from_reader(File::open(repodata_path).unwrap()).unwrap();

    let expected_repodata_entry: Value =
        serde_json::from_reader(File::open(test_data_dir().join(index_json_path)).unwrap())
            .unwrap();

    assert_eq!(
        repodata_json
            .get("info")
            .unwrap()
            .get("subdir")
            .unwrap()
            .as_str(),
        Some("win-64")
    );
    assert!(repodata_json
        .get("packages")
        .unwrap()
        .get("conda-22.9.0-py38haa244fe_2.tar.bz2")
        .is_some());
    assert_eq!(
        repodata_json
            .get("packages.conda")
            .unwrap()
            .get("conda-22.11.1-py38haa244fe_1.conda")
            .unwrap(),
        &expected_repodata_entry
    );
}

#[test]
fn test_index_empty_directory() {
    let temp_dir = tempfile::tempdir().unwrap();
    let res = index(temp_dir.path(), None);
    assert!(res.is_ok());
    assert_eq!(fs::read_dir(temp_dir).unwrap().count(), 0);
}
