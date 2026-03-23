use std::path::PathBuf;

pub(crate) fn test_package_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-data/packages/empty-0.1.0-h4616a5c_0.conda")
}
