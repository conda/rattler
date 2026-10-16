//! Ensures that the checked-in C header matches the Rust sources.
//!
//! Run with `RATTLER_UPDATE_HEADER=1` (or `pixi run generate-header`) to
//! regenerate `include/rattler/rattler.h`.

use std::path::Path;

#[test]
fn header_is_up_to_date() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let header_path = crate_dir.join("include/rattler/rattler.h");

    let config = cbindgen::Config::from_root_or_default(crate_dir);
    let mut generated = Vec::new();
    cbindgen::Builder::new()
        .with_crate(crate_dir)
        .with_config(config)
        .generate()
        .expect("failed to generate the C header")
        .write(&mut generated);
    let generated = String::from_utf8(generated).unwrap();

    if std::env::var_os("RATTLER_UPDATE_HEADER").is_some() {
        std::fs::write(&header_path, generated).unwrap();
        return;
    }

    let existing = std::fs::read_to_string(&header_path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        existing == generated,
        "{} is out of date, regenerate it with `pixi run generate-header`",
        header_path.display()
    );
}
