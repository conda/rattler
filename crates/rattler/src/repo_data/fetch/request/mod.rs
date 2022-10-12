//! Defines a builder struct ([`RequestRepoDataBuilder`]) to construct a request to download channel
//! [`RepoData`]. The request allows for all known types of source and provides adequate local
//! caching.
//!
//! The `RequestRepoDataBuilder` only fetches a single repodata source see
//! [`super::MultiRequestRepoDataBuilder`] for the ability to download from multiple sources in
//! parallel.

mod file;
mod http;

use crate::utils::default_cache_dir;
use crate::{Channel, Platform, RepoData};
use std::{io, path::PathBuf};
use tempfile::PersistError;
use tokio::task::JoinError;

const REPODATA_CHANNEL_PATH: &str = "repodata.json";

/// An error that may occur when trying the fetch repository data.
#[derive(Debug, thiserror::Error)]
pub enum RequestRepoDataError {
    #[error("error deserializing repository data: {0}")]
    DeserializeError(#[from] serde_json::Error),

    #[error("error downloading data: {0}")]
    TransportError(#[from] reqwest::Error),

    #[error("{0}")]
    IoError(#[from] io::Error),

    #[error("unsupported scheme'")]
    UnsupportedScheme,

    #[error("unable to persist temporary file: {0}")]
    PersistError(#[from] PersistError),

    #[error("invalid path")]
    InvalidPath,

    #[error("the operation was cancelled")]
    Cancelled,
}

impl From<JoinError> for RequestRepoDataError {
    fn from(err: JoinError) -> Self {
        match err.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            Err(_) => RequestRepoDataError::Cancelled,
        }
    }
}

/// When a request is processed it goes through several stages, this enum list those stages in
/// order.
#[derive(Debug, Clone)]
pub enum RepoDataRequestState {
    /// The initial state
    Pending,

    /// The request is downloading from a remote server
    Downloading(DownloadingState),

    /// The request is being deserialized
    Deserializing,

    /// The request has finished processing
    Done(DoneState),

    /// An error has occurred during downloading
    Error(String),
}

/// State information of a request when the information is being downloaded.
#[derive(Debug, Clone)]
pub struct DownloadingState {
    /// The number of bytes downloaded
    pub bytes: usize,

    /// The total number of bytes to download. `None` if the total size is unknown. This can happen
    /// if the server does not supply a `Content-Length` header.
    pub total: Option<usize>,
}

impl From<DownloadingState> for RepoDataRequestState {
    fn from(state: DownloadingState) -> Self {
        RepoDataRequestState::Downloading(state)
    }
}

/// State information of a request when the request has finished.
#[derive(Debug, Clone)]
pub struct DoneState {
    /// True if the data was fetched straight from the source and didn't come a cache.
    pub cache_miss: bool,
}

impl From<DoneState> for RepoDataRequestState {
    fn from(state: DoneState) -> Self {
        RepoDataRequestState::Done(state)
    }
}

/// The `RequestRepoDataBuilder` struct allows downloading of repodata from various sources and with
/// proper caching. Repodata fetch can become complex due to the size of some of the repodata.
/// Especially downloading only changes required to update a cached version can become quite
/// complex. This struct handles all the intricacies required to efficiently fetch up-to-date
/// repodata.
///
/// This struct uses a builder pattern which allows a user to setup certain settings other than the
/// default before actually performing the fetch.
///
/// In its simplest form you simply construct a `RequestRepoDataBuilder` through the
/// [`RequestRepoDataBuilder::new`] function, and asynchronously fetch the repodata with
/// [`RequestRepoDataBuilder::request`].
///
/// ```rust,no_run
/// # use std::path::PathBuf;
/// # use rattler::{repo_data::fetch::RequestRepoDataBuilder, Channel, Platform, ChannelConfig};
/// # tokio_test::block_on(async {
/// let channel = Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap();
/// let _repo_data = RequestRepoDataBuilder::new(channel, Platform::NoArch)
///     .request()
///     .await
///     .unwrap();
/// # })
/// ```
///
/// The `RequestRepoDataBuilder` only fetches a single repodata source, see
/// [`super::MultiRequestRepoDataBuilder`] for the ability to download from multiple sources in
/// parallel.
pub struct RequestRepoDataBuilder {
    /// The channel to download from
    pub(super) channel: Channel,

    /// The platform within the channel (also sometimes called the subdir)
    pub(super) platform: Platform,

    /// The directory to store the cache
    pub(super) cache_dir: Option<PathBuf>,

    /// An optional [`reqwest::Client`] that is used to perform the request. When performing
    /// multiple requests its useful to reuse a single client.
    pub(super) http_client: Option<reqwest::Client>,

    /// An optional listener
    pub(super) listener: Option<RequestRepoDataListener>,
}

/// A listener function that is called when a state change of the request occurred.
pub type RequestRepoDataListener = Box<dyn FnMut(RepoDataRequestState) + Send>;

impl RequestRepoDataBuilder {
    /// Constructs a new builder to request repodata for the given channel and platform.
    pub fn new(channel: Channel, platform: Platform) -> Self {
        Self {
            channel,
            platform,
            cache_dir: None,
            http_client: None,
            listener: None,
        }
    }

    /// Sets a default cache directory that will be used for caching requests.
    pub fn set_default_cache_dir(self) -> anyhow::Result<Self> {
        let cache_dir = default_cache_dir()?;
        std::fs::create_dir_all(&cache_dir)?;
        Ok(self.set_cache_dir(cache_dir))
    }

    /// Sets the directory that will be used for caching requests.
    pub fn set_cache_dir(mut self, cache_dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(cache_dir.into());
        self
    }

    /// Sets the [`reqwest::Client`] that is used to perform HTTP requests. If this is not called
    /// a new client is created for each request. When performing multiple requests its more
    /// efficient to reuse a single client across multiple requests.
    pub fn set_http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = Some(client);
        self
    }

    /// Adds a state listener to the builder. This is invoked every time the state of the request
    /// changes. See the [`RepoDataRequestState`] for more information.
    pub fn set_listener(mut self, listener: RequestRepoDataListener) -> Self {
        self.listener = Some(listener);
        self
    }

    /// Consumes self and starts an async request to fetch the repodata.
    pub async fn request(self) -> Result<RepoData, RequestRepoDataError> {
        // Get the url to the subdirectory index. Note that the subdirectory is the platform name.
        let platform_url = self
            .channel
            .platform_url(self.platform)
            .join(REPODATA_CHANNEL_PATH)
            .expect("repodata.json is a valid json path");

        // Construct a new listener function that wraps the optional listener. This allows us to
        // call the listener from anywhere without having to check if there actually is a listener.
        let mut listener = self.listener;
        let mut listener = move |state| {
            if let Some(listener) = listener.as_deref_mut() {
                listener(state)
            }
        };

        // Perform the actual request. This is done in an anonymous function to ensure that any
        // try's do not propagate straight to the outer function. We catch any errors and notify
        // the listener.
        let borrowed_listener = &mut listener;
        let result = (move || async move {
            match platform_url.scheme() {
                "https" | "http" => {
                    // Download the repodata from the subdirectory url
                    let http_client = self.http_client.unwrap_or_else(reqwest::Client::new);
                    http::fetch_repodata(
                        platform_url,
                        http_client,
                        self.cache_dir.as_deref(),
                        borrowed_listener,
                    )
                    .await
                }
                "file" => {
                    let path = platform_url
                        .to_file_path()
                        .map_err(|_| RequestRepoDataError::InvalidPath)?;
                    file::fetch_repodata(&path, borrowed_listener).await
                }
                _ => Err(RequestRepoDataError::UnsupportedScheme),
            }
        })()
        .await;

        // Update the listener accordingly
        match result {
            Ok((repodata, done_state)) => {
                listener(done_state.into());
                Ok(repodata)
            }
            Err(e) => {
                listener(RepoDataRequestState::Error(format!("{}", &e)));
                Err(e)
            }
        }
    }
}
