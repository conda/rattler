use rattler_lock::CondaLock;
use std::path::{Path, PathBuf};

fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
}

#[test]
fn can_parse() {
    assert!(CondaLock::from_path(&test_data_dir().join("conda-lock/numpy-conda-lock.yml")).is_ok());
    assert!(
        CondaLock::from_path(&test_data_dir().join("conda-lock/python-conda-lock.yml")).is_ok()
    );
}

#[test]
fn can_parse_pip() {
    match CondaLock::from_path(&test_data_dir().join("conda-lock/pypi-matplotlib-conda-lock.yml")) {
        Ok(_) => {}
        Err(e) => panic!("{e}"),
    }
}
