// mod macos;
// mod linux;
mod render;
mod schema;

pub mod utils;

#[cfg(test)]
pub mod test {
    use std::path::{Path, PathBuf};

    pub(crate) fn test_data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data/menuinst")
    }
}
