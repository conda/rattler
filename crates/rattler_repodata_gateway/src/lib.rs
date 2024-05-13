#![deny(missing_docs)]

//! `rattler_repodata_gateway` is a crate that provides functionality to interact with Conda
//! repodata. It currently provides functionality to download and cache `repodata.json` files
//! through the [`fetch::fetch_repo_data`] function.
//!
//! In the future this crate will also provide more high-level functionality to query information
//! about specific packages from different sources.
//!
//! # Install
//! Add the following to your *Cargo.toml*:
//!
//! ```toml
//! [dependencies]
//! rattler_repodata_gateway = "0.2.0"
//! ```
//!
//! or run
//!
//! ```bash
//! cargo add rattler_repodata_gateway
//! ```
//!
//! # Examples
//! Below is a basic example that shows how to retrieve and cache the repodata for a conda channel
//! using the [`fetch::fetch_repo_data`] function:
//!
//! ```no_run
//! use std::{path::PathBuf, default::Default};
//! use reqwest::Client;
//! use reqwest_middleware::ClientWithMiddleware;
//! use url::Url;
//! use rattler_repodata_gateway::fetch;
//!
//! #[tokio::main]
//! async fn main() {
//!     let repodata_url = Url::parse("https://conda.anaconda.org/conda-forge/osx-64/").unwrap();
//!     let client = ClientWithMiddleware::from(Client::new());
//!     let cache = PathBuf::from("./cache");
//!
//!     let result = fetch::fetch_repo_data(
//!         repodata_url,
//!         client,
//!         cache,
//!         fetch::FetchRepoDataOptions { ..Default::default() },
//!         None,
//!     ).await;
//!
//!     let result = match result {
//!         Err(err) => {
//!             panic!("{:?}", err);
//!         }
//!         Ok(result) => result
//!     };
//!
//!     println!("{:?}", result.cache_state);
//!     println!("{:?}", result.cache_result);
//!     println!("{:?}", result.lock_file);
//!     println!("{:?}", result.repo_data_json_path);
//! }
//! ```

pub mod fetch;
mod reporter;
#[cfg(feature = "sparse")]
pub mod sparse;
mod utils;
pub use reporter::Reporter;

#[cfg(feature = "gateway")]
mod gateway;

#[cfg(feature = "gateway")]
pub use gateway::{
    ChannelConfig, Gateway, GatewayBuilder, GatewayError, RepoData, SourceConfig, SubdirSelection,
};
