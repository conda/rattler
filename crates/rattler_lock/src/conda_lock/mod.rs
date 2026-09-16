//! Offline conversion between CEP-37 and pixi lock files.
//!
//! [`LockFile::from_conda_lock`] selects exact category sets;
//! [`Environment::to_conda_lock`] converts one environment into nonoptional
//! `main` packages. Conversion preserves install artifacts, not all solver
//! metadata. Unsupported semantics return a [`CondaLockError`] rather than
//! disappearing.
//!
//! ```
//! use rattler_lock::{LockFile, conda_lock::{ExportOptions, ImportOptions}};
//! # fn example(cep: &rattler_conda_lock::LockFile) -> Result<(), rattler_lock::conda_lock::CondaLockError> {
//! let pixi = LockFile::from_conda_lock(cep, &ImportOptions::default())?;
//! let environment = pixi.default_environment().unwrap();
//! let cep = environment.to_conda_lock(&ExportOptions::default())?;
//! # Ok(())
//! # }
//! ```
//!
//! # Installing an imported lock file
//!
//! Importing yields an ordinary [`LockFile`], so the regular
//! [`Environment::conda_repodata_records`] gives installable
//! [`rattler_conda_types::RepoDataRecord`]s directly — no second conversion:
//!
//! ```
//! use rattler_conda_lock::Document;
//! use rattler_lock::{LockFile, conda_lock::{CondaLockError, ImportOptions}};
//!
//! # fn main() -> Result<(), CondaLockError> {
//! let document = Document::parse(
//!     "version: 1
//! metadata:
//!   content_hash:
//!     linux-64: 8b7df143d91c716ecfa5fc1730022f6b421b05cedee8fd52b1fc65a96030ad52
//!   channels:
//!     - url: https://conda.anaconda.org/conda-forge/
//!       used_env_vars: []
//!   platforms: [linux-64]
//!   sources: []
//! package:
//!   - name: ca-certificates
//!     version: 2025.10.5
//!     manager: conda
//!     platform: linux-64
//!     url: https://conda.anaconda.org/conda-forge/noarch/ca-certificates-2025.10.5-hbd8a1cb_0.conda
//!     hash:
//!       md5: f9e5fbc24009179e8b0409624691758a
//!       sha256: 3b5ad78b8bb61b6cdc0978a6a99f8dfb2cc789a451378d054698441005ecbdb6
//!     category: main
//!     optional: false
//! ",
//! )?;
//!
//! let pixi = LockFile::from_conda_lock_document(&document, &ImportOptions::default())?;
//! let environment = pixi.default_environment().unwrap();
//! let platform = environment.platforms().next().unwrap();
//!
//! let records = environment
//!     .conda_repodata_records(platform)
//!     .expect("binary packages convert to repodata records")
//!     .expect("the platform is part of the environment");
//! assert_eq!(records[0].package_record.name.as_normalized(), "ca-certificates");
//! assert_eq!(
//!     records[0].identifier.to_string(),
//!     "ca-certificates-2025.10.5-hbd8a1cb_0.conda"
//! );
//! # Ok(())
//! # }
//! ```

mod dependencies;
mod error;
mod export;
mod import;

pub use error::{ChecksumAlgorithm, CondaLockError, CondaLockErrorKind};
pub use export::ExportOptions;
pub use import::ImportOptions;

use rattler_conda_lock::NodePath;
use url::Url;

#[allow(unused_imports)] // Referenced from the module documentation.
use crate::{Environment, LockFile};

/// Parses a direct artifact URL, rejecting anything a package cannot be
/// downloaded from: relative paths, `git+` style source locations, and other
/// schemes that do not name a fetchable file.
fn artifact_url(value: &str, path: &NodePath) -> Result<Url, CondaLockError> {
    let url = Url::parse(value).map_err(|error| {
        CondaLockError::new(path.clone(), CondaLockErrorKind::InvalidUrl(error))
    })?;
    if !matches!(url.scheme(), "http" | "https" | "file" | "ftp" | "s3") || url.cannot_be_a_base() {
        return Err(CondaLockError::new(
            path.clone(),
            CondaLockErrorKind::NotAnArtifactUrl,
        ));
    }
    Ok(url)
}
