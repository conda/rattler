#![deny(missing_docs)]

//! `rattler_repodata_gateway` is a crate that provides functionality to interact with Conda
//! repodata. It currently provides functionality to download and cache `repodata.json` files
//! through the [`fetch::fetch_repo_data`] function.
//!
//! In the future this crate will also provide more high-level functionality to query information
//! about specific packages from different sources.

pub mod fetch;
#[cfg(feature = "sparse")]
pub mod sparse;

mod utils;
