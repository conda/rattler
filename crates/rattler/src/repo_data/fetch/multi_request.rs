//! Defines the [`MultiRequestRepoDataBuilder`] struct. This struct enables async fetching channel
//! repodata from multiple source in parallel.

use crate::{
    repo_data::fetch::request::{
        RepoDataRequestState, RequestRepoDataBuilder, RequestRepoDataError, RequestRepoDataListener,
    },
    Channel, Platform, RepoData,
};
use futures::{stream::FuturesUnordered, StreamExt};
use std::path::PathBuf;

/// The `MultiRequestRepoDataBuilder` handles fetching data from multiple conda channels and
/// for multiple platforms. Internally it dispatches all requests to [`RequestRepoDataBuilder`]s
/// which ensure that only the latest changes are fetched.
///
/// A `MultiRequestRepoDataBuilder` also provides very explicit user feedback through the
/// [`MultiRequestRepoDataBuilder::set_listener`] method. An example of its usage can be found in
/// the [`super::terminal_progress`] which disables multiple CLI progress bars while the requests
/// are being performed.
///
/// ```rust,no_run
/// # use std::path::PathBuf;
/// # use rattler::{repo_data::fetch::MultiRequestRepoDataBuilder, Channel, Platform, ChannelConfig};
/// # tokio_test::block_on(async {
/// let _repo_data = MultiRequestRepoDataBuilder::default()
///     .add_channel(Channel::from_str("conda-forge", &ChannelConfig::default()).unwrap())
///     .request()
///     .await;
/// # })
/// ```
pub struct MultiRequestRepoDataBuilder {
    /// All the source to fetch
    sources: Vec<(Channel, Platform)>,

    /// The directory to store the cache
    cache_dir: Option<PathBuf>,

    /// An optional [`reqwest::Client`] that is used to perform the request. When performing
    /// multiple requests its useful to reuse a single client.
    http_client: Option<reqwest::Client>,

    /// True to fail as soon as one of the queries fails. If this is set to false the other queries
    /// continue. Defaults to `true`.
    fail_fast: bool,

    /// An optional listener
    listener: Option<MultiRequestRepoDataListener>,
}

impl Default for MultiRequestRepoDataBuilder {
    fn default() -> Self {
        Self {
            sources: vec![],
            cache_dir: None,
            http_client: None,
            fail_fast: true,
            listener: None,
        }
    }
}

/// A listener function that is called for a request source ([`Channel`] and [`Platform`]) when a
/// state change of the request occurred.
pub type MultiRequestRepoDataListener =
    Box<dyn FnMut(Channel, Platform, RepoDataRequestState) + Send>;

impl MultiRequestRepoDataBuilder {
    /// Adds the specific platform of the given channel to the list of sources to fetch.
    pub fn add_channel_and_platform(mut self, channel: Channel, platform: Platform) -> Self {
        self.sources.push((channel, platform));
        self
    }

    /// Adds the specified channel to the list of source to fetch. The platforms specified in the
    /// channel or the default platforms are added as defined by [`Channel::platforms_or_default`].
    pub fn add_channel(mut self, channel: Channel) -> Self {
        for platform in channel.platforms_or_default() {
            self.sources.push((channel.clone(), *platform));
        }
        self
    }

    /// Adds multiple channels to the request builder. For each channel the platforms or the default
    /// set of platforms are added (see: [`Channel::platforms_or_default`]).
    pub fn add_channels(mut self, channels: impl IntoIterator<Item = Channel>) -> Self {
        for channel in channels.into_iter() {
            for platform in channel.platforms_or_default() {
                self.sources.push((channel.clone(), *platform));
            }
        }
        self
    }

    /// Sets a default cache directory that will be used for caching requests.
    pub fn set_default_cache_dir(mut self) -> anyhow::Result<Self> {
        let cache_dir = dirs::cache_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine cache directory for current platform"))?
            .join("rattler/cache");
        std::fs::create_dir_all(&cache_dir)?;
        Ok(self.set_cache_dir(cache_dir))
    }

    /// Sets the directory that will be used for caching requests.
    pub fn set_cache_dir(mut self, cache_dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(cache_dir.into());
        self
    }

    /// Sets the [`reqwest::Client`] that is used to perform HTTP requests. If this is not called
    /// a new client is created for this entire instance. The created client is shared for all
    /// requests. When performing multiple requests its more efficient to reuse a single client
    /// across multiple requests.
    pub fn set_http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = Some(client);
        self
    }

    /// Adds a state listener to the builder. This is invoked every time the state of the request
    /// changes. See the [`RepoDataRequestState`] for more information.
    pub fn set_listener(mut self, listener: MultiRequestRepoDataListener) -> Self {
        self.listener = Some(listener);
        self
    }

    /// Sets a boolean indicating whether or not to stop processing the rest of the queries if one
    /// of them fails. By default this is `true`.
    pub fn set_fail_fast(mut self, fail_fast: bool) -> Self {
        self.fail_fast = fail_fast;
        self
    }

    /// Asynchronously fetches repodata information from the sources added to this instance. A
    /// vector is returned that contains the state of each source. The returned state is returned in
    /// the same order as they were added.
    pub async fn request(
        mut self,
    ) -> Vec<(Channel, Platform, Result<RepoData, RequestRepoDataError>)> {
        // Construct an http client for the requests if none has been specified by the user.
        let http_client = self.http_client.unwrap_or_else(reqwest::Client::new);

        // Channel that will receive events from the different sources. Each source spawns a new
        // future, this channel ensures that all events arrive in the same place so we can handle
        // them.
        let (state_sender, mut state_receiver) = tokio::sync::mpsc::unbounded_channel();

        // Construct a query for every source
        let mut futures = FuturesUnordered::new();
        let mut results = Vec::with_capacity(self.sources.len());
        for (idx, (channel, platform)) in self.sources.into_iter().enumerate() {
            // Create a result for this source that is initially cancelled. If we return from this
            // function before the a result is computed this is the correct response.
            results.push((
                channel.clone(),
                platform,
                Err(RequestRepoDataError::Cancelled),
            ));

            // If there is a listener active for this instance, construct a listener for this
            // specific source request that funnels all state changes to a channel.
            let listener = if self.listener.is_some() {
                // Construct a closure that captures the index of the current source. State changes
                // for the current source request are added to an unbounded channel which is
                // processed on the main task.
                let sender = state_sender.clone();
                let mut request_listener: RequestRepoDataListener =
                    Box::new(move |request_state| {
                        // Silently ignore send errors. It probably means the receiving end was
                        // dropped, which is perfectly fine.
                        let _ = sender.send((idx, request_state));
                    });

                // Notify the listener immediately about a pending state. This is done on the main
                // task to ensure that the listener is notified about all the sources in the correct
                // order. Since the source requests are spawned they may run on a background thread
                // where potentially the order of the source is lost. Firing an initial state change
                // here ensures that the listener is notified of all the sources in the same order
                // they were added to this instance.
                request_listener(RepoDataRequestState::Pending);

                Some(request_listener)
            } else {
                None
            };

            // Construct a `RequestRepoDataBuilder` for this source that will perform the actual
            // request.
            let source_request = RequestRepoDataBuilder {
                channel,
                platform,
                cache_dir: self.cache_dir.clone(),
                http_client: Some(http_client.clone()),
                listener,
            };

            // Spawn a future that will await the request. This future is "spawned" which means
            // it is executed on a different thread. The JoinHandle is pushed to the `futures`
            // collection which allows us the asynchronously wait for all results in parallel.
            let request_future = tokio::spawn(async move { (idx, source_request.request().await) });
            futures.push(request_future);
        }

        // Drop the event sender, this will ensure that only RequestRepoDataBuilder listeners could
        // have a sender. Once all requests have finished they will drop their sender handle, which
        // will eventually close all senders and therefor the receiver. If this wouldn't be the case
        // the select below would wait indefinitely until it received an event.
        drop(state_sender);

        // Loop over two streams until they both complete. The `select!` macro selects the first
        // future that becomes ready from the two sources.
        //
        // 1. The `state_receiver` is a channel that contains `RepoDataRequestState`s from each
        //    source as it executes. This only contains data if this instance has a listener.
        // 2. The `futures` is an `UnorderedFutures` collection that yields results from individual
        //    source requests as they become available.
        loop {
            tokio::select! {
                Some((idx, state_change)) = state_receiver.recv() => {
                    let listener = self
                        .listener
                        .as_mut()
                        .expect("there must be a listener at this point");
                    let channel = results[idx].0.clone();
                    let platform = results[idx].1;
                    listener(channel, platform, state_change);
                },
                Some(result) = futures.next() => match result {
                    Ok((idx, result)) => {
                        // Store the result in the results container. This overwrites the value that
                        // is currently already there. The initial value is a Cancelled result.
                        results[idx].2 = result;

                        // If the result contains an error and we want to fail fast, break right
                        // away, this will drop the rest of the futures, cancelling them.
                        if results[idx].2.is_err() && self.fail_fast {
                            break;
                        }
                    },
                    Err(err) => {
                        // If a panic occurred in the source request we want to propagate it here.
                        if let Ok(reason) = err.try_into_panic() {
                            std::panic::resume_unwind(reason);
                        }
                        break;
                    }
                },
                else => break,
            }
        }

        results
    }
}
