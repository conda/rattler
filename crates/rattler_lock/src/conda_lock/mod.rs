//! Offline conversion between CEP-37 and pixi lock files.
//!
//! Import selects exact category sets; export converts one environment into
//! nonoptional `main` packages. Conversion preserves install artifacts, not all
//! solver metadata. Unsupported semantics return errors rather than disappearing.
//!
//! ```
//! use rattler_lock::conda_lock::{import, export, ImportOptions, ExportOptions};
//! # fn example(cep: &rattler_conda_lock::LockFile) -> Result<(), rattler_conda_lock::Error> {
//! let pixi = import(cep, &ImportOptions::default())?;
//! let environment = pixi.default_environment().unwrap();
//! let cep = export(environment, &ExportOptions::default())?;
//! # Ok(())
//! # }
//! ```

mod dependencies;
mod export;
mod import;

pub use export::{ExportOptions, export};
pub use import::{ImportOptions, import, import_document};

use rattler_conda_lock::Error;
use url::Url;

fn artifact_url(value: &str, path: &str) -> Result<Url, Error> {
    let url = Url::parse(value).map_err(|error| Error::new(path, error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https" | "file" | "ftp" | "s3") || url.cannot_be_a_base() {
        return Err(Error::new(
            path,
            "expected a direct artifact URL, not a source or local path",
        ));
    }
    Ok(url)
}
