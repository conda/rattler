//! Middleware to handle `gcs://` URLs to pull artifacts from an GCS
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use google_cloud_auth::credentials::{
    Builder as AccessTokenCredentialBuilder, CacheableResource, Credentials, EntityTag,
};
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MiddlewareResult};
use tokio::sync::Notify;
use url::Url;

/// The auth headers and the `EntityTag` assigned by the credential library.
///
/// The `EntityTag` is an opaque process-local token: the library generates one
/// per token lifetime and reuses it until the token is refreshed.  Passing it
/// back to `Credentials::headers()` lets the library confirm our cached copy is
/// still current (`NotModified`) or hand us a freshly-minted one (`New`).
struct CachedResource {
    entity_tag: EntityTag,
    headers: http::HeaderMap,
}

/// Token-cache state machine.
enum CacheState {
    /// No resource has been fetched yet and no refresh is in flight.
    Empty,
    /// One task is fetching a new token; others wait on `GCSInner::refresh_done`.
    ///
    /// The [`Weak`] is a liveness token: it points to an [`Arc`] owned by the
    /// [`RefreshGuard`] that the refreshing task holds.  When the refreshing
    /// task is cancelled (future dropped), the `Arc` is dropped, the `Weak`
    /// becomes dead, and any waiter or new caller that observes a dead `Weak`
    /// knows the refresher is gone and can reset the state to [`Empty`] and
    /// retry — without requiring the `Drop` impl to acquire the mutex.
    Refreshing(Weak<()>),
    /// A valid resource is available.
    Ready(CachedResource),
}

/// Shared, ref-counted state owned by every clone of a [`GCSMiddleware`].
struct GCSInner {
    /// Credential source built once and reused across all requests.
    credential: Mutex<Option<Credentials>>,
    /// Cache state machine guarded by a mutex.
    cache: Mutex<CacheState>,
    /// Woken every time a refresh completes (success or failure) so that
    /// waiters can re-inspect the cache state.
    refresh_done: Notify,
}

/// GCS middleware to authenticate requests.
///
/// A single [`GCSMiddleware`] instance (or any clone of one) shares one
/// `OAuth2` credential and one token cache.  At most one token fetch is in
/// flight at a time regardless of how many concurrent requests are being
/// processed (singleflight).  Subsequent requests reuse the cached resource
/// until the credential library signals that the token has changed, at which
/// point the cache is transparently refreshed.
#[derive(Clone)]
pub struct GCSMiddleware {
    inner: Arc<GCSInner>,
}

/// Outcome of a single synchronous inspection of the token cache.
///
/// Returned by [`GCSMiddleware::poll_cache`], which holds and releases the
/// `std::sync::Mutex` entirely within a non-`async` context.  This ensures
/// that no `MutexGuard` ever appears in the state machine of the surrounding
/// `async fn`, keeping the future `Send`.
enum PollResult<'a> {
    /// A refresh is in flight.  The contained future has already been enabled
    /// (via [`tokio::sync::futures::Notified::enable`]) while the lock was
    /// held, so awaiting it cannot miss the completion signal.
    Wait(Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>),
    /// The previous refresher was cancelled; the cache has been reset to
    /// [`CacheState::Empty`].  The caller should loop immediately.
    Retry,
    /// A cached token is available.  The caller should validate it with the
    /// credential library (no lock held).
    Validate {
        entity_tag: EntityTag,
        headers: http::HeaderMap,
    },
    /// The cache was empty and the caller has claimed the refresh slot.  The
    /// contained `Arc` is the liveness token; its `Weak` counterpart is now
    /// stored in [`CacheState::Refreshing`].
    StartRefresh(Arc<()>),
}

impl Default for GCSMiddleware {
    fn default() -> Self {
        Self {
            inner: Arc::new(GCSInner {
                credential: Mutex::new(None),
                cache: Mutex::new(CacheState::Empty),
                refresh_done: Notify::new(),
            }),
        }
    }
}

#[cfg(test)]
impl GCSMiddleware {
    /// Test-only constructor that pre-seeds the credential so no ADC discovery
    /// is needed.
    fn with_credentials(cred: Credentials) -> Self {
        Self {
            inner: Arc::new(GCSInner {
                credential: Mutex::new(Some(cred)),
                cache: Mutex::new(CacheState::Empty),
                refresh_done: Notify::new(),
            }),
        }
    }
}

#[async_trait]
impl Middleware for GCSMiddleware {
    /// Create a new authentication middleware for GCS
    async fn handle(
        &self,
        mut req: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> MiddlewareResult<Response> {
        if req.url().scheme() == "gcs" {
            let mut url = req.url().clone();
            let bucket_name = url.host_str().expect("Host should be present in GCS URL");
            let new_url = format!(
                "https://storage.googleapis.com/{}{}",
                bucket_name,
                url.path()
            );
            url = Url::parse(&new_url).expect("Failed to parse URL");
            *req.url_mut() = url;
            req = self.authenticate(req).await?;
        }
        next.run(req, extensions).await
    }
}

impl GCSMiddleware {
    /// Add GCS authentication headers to `req`, drawing from the token cache
    /// when available and fetching a new token only when necessary.
    async fn authenticate(&self, mut req: Request) -> MiddlewareResult<Request> {
        let headers = self.get_or_refresh_token().await?;
        req.headers_mut().extend(headers);
        Ok(req)
    }

    /// Lazily initialise the `Credentials` object (once per middleware
    /// lifetime) and return a cheap `Arc`-clone of it.
    async fn get_credential(&self) -> MiddlewareResult<Credentials> {
        let mut guard = self.inner.credential.lock().unwrap();
        if guard.is_none() {
            let scopes = ["https://www.googleapis.com/auth/devstorage.read_only"];
            let c = AccessTokenCredentialBuilder::default()
                .with_scopes(scopes)
                .build()
                .map_err(|e| reqwest_middleware::Error::Middleware(anyhow::Error::new(e)))?;
            *guard = Some(c);
        }
        // Credentials is Arc-backed; clone is a cheap refcount bump.
        Ok(guard.as_ref().unwrap().clone())
    }

    /// Inspect and (if necessary) update the token cache under the lock, then
    /// return a [`PollResult`] describing what the caller should do next.
    ///
    /// This is a plain (`!async`) function so that the `std::sync::MutexGuard`
    /// is created and dropped entirely within synchronous code.  The guard
    /// never appears in an `async` state machine, keeping every future that
    /// calls this method `Send`.
    ///
    /// ## Liveness-token protocol
    ///
    /// `mem::replace` takes ownership of the current state before any match
    /// arm runs, so no pattern binding borrows from `guard`.  This avoids the
    /// borrow-checker cycle that would otherwise prevent re-assigning `*guard`
    /// inside the same match.
    fn poll_cache<'a>(&'a self) -> PollResult<'a> {
        let mut guard = self.inner.cache.lock().unwrap();

        // Take ownership of the state, leaving a harmless placeholder.
        // Every arm below restores or updates `*guard` before releasing it.
        let state = std::mem::replace(&mut *guard, CacheState::Empty);

        match state {
            // ── Refreshing (live) ────────────────────────────────────────────
            // Put the state back, subscribe to the completion signal while the
            // lock is still held (so we cannot miss `notify_waiters()`), then
            // release the lock.
            CacheState::Refreshing(weak) if weak.upgrade().is_some() => {
                *guard = CacheState::Refreshing(weak);
                let mut notified = Box::pin(self.inner.refresh_done.notified());
                notified.as_mut().enable();
                drop(guard);
                PollResult::Wait(notified)
            }

            // ── Refreshing (cancelled) ───────────────────────────────────────
            // The Arc that backs the Weak is gone.  Leave the cache as Empty
            // (the placeholder set by `mem::replace`) so the caller can retry.
            CacheState::Refreshing(_dead) => PollResult::Retry,

            // ── Ready ────────────────────────────────────────────────────────
            // Clone the cached values, restore the state, then tell the caller
            // to validate the token without the lock.
            CacheState::Ready(r) => {
                let entity_tag = r.entity_tag.clone();
                let headers = r.headers.clone();
                *guard = CacheState::Ready(r);
                PollResult::Validate {
                    entity_tag,
                    headers,
                }
            }

            // ── Empty ────────────────────────────────────────────────────────
            // Claim the refresh slot atomically under the lock.
            CacheState::Empty => {
                let token = Arc::new(());
                *guard = CacheState::Refreshing(Arc::downgrade(&token));
                PollResult::StartRefresh(token)
            }
        }
    }

    /// Return cached auth headers, refreshing exactly once when necessary.
    ///
    /// ## Concurrency model
    ///
    /// The cache cycles through three states stored in `GCSInner::cache`:
    ///
    /// * **`Empty`** – the first caller atomically transitions to `Refreshing`
    ///   (under the lock) and performs the network fetch; all other concurrent
    ///   callers see `Refreshing` and wait on `refresh_done`.
    /// * **`Refreshing(weak)`** – callers subscribe to `refresh_done` *before*
    ///   releasing the lock (via `Notified::enable`), guaranteeing that the
    ///   wake-up from `notify_waiters` cannot arrive between the lock release
    ///   and the subscription.  They then loop to re-read the updated state.
    /// * **`Ready`** – callers clone the cached `EntityTag` + headers, drop
    ///   the lock, and ask the credential library whether the token is still
    ///   current.  The library returns `NotModified` (fast path) or `New`
    ///   (token was silently refreshed); in the latter case the cache is
    ///   overwritten.  No lock is ever held across an `await`.
    ///
    /// ## Cancellation safety
    ///
    /// The `Refreshing` variant carries a `Weak<()>` whose corresponding
    /// strong `Arc<()>` is owned by the [`RefreshGuard`] that the active
    /// refreshing task holds.  If that task is dropped (cancelled) at any
    /// `.await` point, the `Arc` is released, making the `Weak` dead.
    ///
    /// Any waiter or new caller that subsequently observes `Refreshing(dead)`
    /// resets the state to `Empty` itself and retries.  The `Drop` impl of
    /// [`RefreshGuard`] calls `notify_waiters()` to wake existing waiters
    /// without needing to acquire the mutex.
    async fn get_or_refresh_token(&self) -> MiddlewareResult<http::HeaderMap> {
        loop {
            // `poll_cache` is synchronous: the MutexGuard is acquired and
            // dropped entirely inside it, so it never appears in this async
            // state machine.  All variants of `PollResult` are `Send`.
            match self.poll_cache() {
                // ── Wait ─────────────────────────────────────────────────────
                // The contained future was already enabled while the lock was
                // held; awaiting it is race-free.
                PollResult::Wait(notified) => {
                    notified.await;
                }

                // ── Retry ────────────────────────────────────────────────────
                // Refresher was cancelled; loop to claim the slot ourselves.
                PollResult::Retry => {}

                // ── Validate ─────────────────────────────────────────────────
                // Check whether the cached token is still current.
                PollResult::Validate {
                    entity_tag,
                    headers,
                } => {
                    let cred = self.get_credential().await?;
                    let mut ext = http::Extensions::new();
                    ext.insert(entity_tag);

                    return match cred
                        .headers(ext)
                        .await
                        .map_err(|e| reqwest_middleware::Error::Middleware(anyhow::Error::new(e)))?
                    {
                        CacheableResource::NotModified => Ok(headers),
                        CacheableResource::New { entity_tag, data } => {
                            *self.inner.cache.lock().unwrap() = CacheState::Ready(CachedResource {
                                entity_tag,
                                headers: data.clone(),
                            });
                            Ok(data)
                        }
                    };
                }

                // ── StartRefresh ──────────────────────────────────────────────
                // We claimed the slot; perform the fetch and update the cache.
                PollResult::StartRefresh(token) => {
                    let mut refresh_guard = RefreshGuard {
                        inner: Arc::clone(&self.inner),
                        _token: token,
                        defused: false,
                    };

                    let cred = self.get_credential().await?;
                    let fetch = cred
                        .headers(http::Extensions::new())
                        .await
                        .map_err(|e| reqwest_middleware::Error::Middleware(anyhow::Error::new(e)));

                    match fetch {
                        Ok(CacheableResource::New { entity_tag, data }) => {
                            let out = data.clone();
                            *self.inner.cache.lock().unwrap() = CacheState::Ready(CachedResource {
                                entity_tag,
                                headers: data,
                            });
                            refresh_guard.defused = true;
                            self.inner.refresh_done.notify_waiters();
                            return Ok(out);
                        }
                        // We passed no ETag, so `NotModified` is impossible.
                        Ok(CacheableResource::NotModified) => unreachable!(
                            "no entity tag was provided in extensions, \
                             so NotModified cannot be returned"
                        ),
                        Err(e) => {
                            *self.inner.cache.lock().unwrap() = CacheState::Empty;
                            refresh_guard.defused = true;
                            self.inner.refresh_done.notify_waiters();
                            return Err(e);
                        }
                    }
                }
            }
        }
    }
}

/// RAII guard that signals cancellation to waiters when dropped before being
/// explicitly defused.
///
/// The guard owns the strong [`Arc<()>`] liveness token that is paired with
/// the [`Weak<()>`] stored in [`CacheState::Refreshing`].  When the guard is
/// dropped without defusing (i.e. the refreshing future is cancelled), the
/// strong count drops to zero, making the `Weak` dead.  Any waiter or new
/// caller that observes a dead `Weak` in `CacheState::Refreshing` resets the
/// state to [`CacheState::Empty`] and retries — no mutex acquisition is needed
/// inside this `Drop` impl.
///
/// The guard also calls [`Notify::notify_waiters`] on drop so that any tasks
/// already suspended on `refresh_done` wake up and discover the dead token.
struct RefreshGuard {
    inner: Arc<GCSInner>,
    /// Owning strong reference to the liveness token.  Dropped when the guard
    /// is dropped, making the corresponding `Weak` in the cache dead.
    _token: Arc<()>,
    defused: bool,
}

impl Drop for RefreshGuard {
    fn drop(&mut self) {
        if !self.defused {
            // `_token` is dropped automatically after this block, making the
            // `Weak` in `CacheState::Refreshing` dead.  Wake all current
            // waiters so they can re-inspect the state and detect the
            // dead token.  New callers will also detect it on their first
            // lock acquisition without needing to be explicitly woken.
            self.inner.refresh_done.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use google_cloud_auth::credentials::{CacheableResource, CredentialsProvider, EntityTag};
    use google_cloud_auth::errors::CredentialsError;
    use reqwest::Client;
    use tempfile;
    use tokio::sync::Barrier;

    use super::*;

    // ── Mock credential provider ─────────────────────────────────────────────

    /// Shared, externally-controllable "current token version" for the mock.
    type SharedEtag = Arc<std::sync::Mutex<EntityTag>>;

    /// A fake `CredentialsProvider` that behaves like the real Google library:
    ///
    /// * Keeps an internal "current" `EntityTag` (shared so tests can rotate it).
    /// * Returns `NotModified` when the caller presents the same tag.
    /// * Returns `New` (with the current tag) otherwise.
    /// * Counts every call that issues new headers (i.e. not `NotModified`).
    #[derive(Debug)]
    struct MockProvider {
        /// Monotonically-increasing call count for non-`NotModified` responses.
        refresh_count: Arc<AtomicUsize>,
        /// The current "token version"; tests rotate it via the shared handle.
        current_etag: SharedEtag,
        /// Optional barrier used by concurrency tests to synchronise callers.
        barrier: Option<Arc<Barrier>>,
    }

    impl MockProvider {
        /// Returns the provider, a counter handle, and an etag handle.
        /// Rotating the etag (via the handle) simulates a server-side token
        /// refresh so the next caller receives `New` instead of `NotModified`.
        fn new() -> (Self, Arc<AtomicUsize>, SharedEtag) {
            let count = Arc::new(AtomicUsize::new(0));
            let etag: SharedEtag = Arc::new(std::sync::Mutex::new(EntityTag::new()));
            let p = Self {
                refresh_count: count.clone(),
                current_etag: Arc::clone(&etag),
                barrier: None,
            };
            (p, count, etag)
        }

        fn with_barrier(barrier: Arc<Barrier>) -> (Self, Arc<AtomicUsize>, SharedEtag) {
            let (mut p, count, etag) = Self::new();
            p.barrier = Some(barrier);
            (p, count, etag)
        }
    }

    impl CredentialsProvider for MockProvider {
        async fn headers(
            &self,
            extensions: http::Extensions,
        ) -> Result<CacheableResource<http::HeaderMap>, CredentialsError> {
            let current = self.current_etag.lock().unwrap().clone();

            // Simulate the library's ETag protocol.
            if let Some(caller_tag) = extensions.get::<EntityTag>() {
                if *caller_tag == current {
                    return Ok(CacheableResource::NotModified);
                }
            }

            self.refresh_count.fetch_add(1, Ordering::SeqCst);

            // If a barrier was provided, wait here so concurrent callers pile
            // up and see the `Refreshing` state while this task is suspended.
            if let Some(ref b) = self.barrier {
                b.wait().await;
            }

            let mut map = http::HeaderMap::new();
            map.insert(
                http::header::AUTHORIZATION,
                "Bearer mock-token".parse().unwrap(),
            );
            Ok(CacheableResource::New {
                entity_tag: current,
                data: map,
            })
        }

        async fn universe_domain(&self) -> Option<String> {
            None
        }
    }

    // ── Unit tests ───────────────────────────────────────────────────────────

    /// The credential provider is called exactly once on the first request; all
    /// subsequent requests are answered from the cache without hitting the
    /// provider again.
    #[tokio::test]
    async fn test_cache_reuses_valid_token() {
        let (provider, count, _etag) = MockProvider::new();
        let mw = GCSMiddleware::with_credentials(Credentials::from(provider));

        let h1 = mw.get_or_refresh_token().await.unwrap();
        let h2 = mw.get_or_refresh_token().await.unwrap();
        let h3 = mw.get_or_refresh_token().await.unwrap();

        // Only one actual fetch should have occurred.
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // All responses carry the same headers.
        assert_eq!(h1, h2);
        assert_eq!(h2, h3);
    }

    /// When the mock rotates its `EntityTag` (simulating a server-side token
    /// refresh), the middleware transparently picks up the new headers.
    #[tokio::test]
    async fn test_cache_refreshes_on_token_change() {
        let (provider, count, etag_handle) = MockProvider::new();
        let mw = GCSMiddleware::with_credentials(Credentials::from(provider));

        mw.get_or_refresh_token().await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);

        // Rotate the token – the middleware's next ETag validation will get
        // `New` back and update the cache.
        *etag_handle.lock().unwrap() = EntityTag::new();

        mw.get_or_refresh_token().await.unwrap();
        // A second fetch must have occurred.
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    /// When many tasks call `get_or_refresh_token` simultaneously from an empty
    /// cache, exactly one of them reaches the credential provider for the
    /// initial fetch; the rest wait on the `Notify` and are served from the
    /// cache once the fetch completes.
    #[tokio::test]
    async fn test_singleflight_under_concurrent_load() {
        const TASKS: usize = 10;

        // The barrier has size 2: the single task that enters `headers()` plus
        // the test driver.  This lets the driver confirm that exactly one task
        // is inside the provider before unblocking it, which in turn ensures
        // all other tasks have had a chance to see the `Refreshing` state.
        let barrier = Arc::new(Barrier::new(2));
        let (provider, count, _etag) = MockProvider::with_barrier(Arc::clone(&barrier));
        let mw = GCSMiddleware::with_credentials(Credentials::from(provider));

        // Spawn all tasks before any of them are awaited.
        let handles: Vec<_> = (0..TASKS)
            .map(|_| {
                let mw = mw.clone();
                tokio::spawn(async move { mw.get_or_refresh_token().await.unwrap() })
            })
            .collect();

        // Rendezvous with the one task that reached the provider, then release
        // it so the rest can be woken.
        barrier.wait().await;

        // Collect all results.
        for handle in handles {
            handle.await.unwrap();
        }

        // Only one call should have entered the credential provider.
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "expected exactly 1 refresh call, got {}",
            count.load(Ordering::SeqCst)
        );
    }

    /// Cancelling the task that owns the `Refreshing` state must not leave
    /// subsequent callers blocked forever on `refresh_done`.
    ///
    /// Before the `RefreshGuard` + `Weak` liveness-token fix, dropping the
    /// refreshing future mid-flight left `CacheState::Refreshing` permanently
    /// set (with a live-looking but orphaned state), so every subsequent call
    /// to `get_or_refresh_token` would hang indefinitely on `notified.await`.
    #[tokio::test]
    async fn test_cancellation_during_refresh_does_not_deadlock() {
        // A 2-party barrier: the test driver + the one task that enters
        // `headers()`.  This lets the driver confirm the refresh is in-flight
        // (i.e. `Refreshing` is set and the mutex has been released) before
        // aborting the task.
        let barrier = Arc::new(Barrier::new(2));
        let (provider, _count, _etag) = MockProvider::with_barrier(Arc::clone(&barrier));
        let mw = GCSMiddleware::with_credentials(Credentials::from(provider));

        let mw_clone = mw.clone();
        let handle = tokio::spawn(async move { mw_clone.get_or_refresh_token().await });

        // Wait until the spawned task is inside `headers()` (i.e. the state is
        // `Refreshing` and the mutex has been released).
        barrier.wait().await;

        // Cancel the refreshing task – this is the scenario that previously
        // deadlocked every subsequent caller.
        handle.abort();
        let _ = handle.await; // consume the JoinError

        // The `RefreshGuard` Drop impl must have called `notify_waiters()`,
        // waking any current waiters, and the dead `Weak` token in
        // `CacheState::Refreshing` lets the next caller reset state to `Empty`.
        // A fresh call must succeed (or at least not hang forever).
        tokio::time::timeout(std::time::Duration::from_secs(5), mw.get_or_refresh_token())
            .await
            .expect("timed out – RefreshGuard did not unblock callers on cancellation")
            .expect("token fetch failed after cancellation recovery");
    }

    // ── Integration test (requires real GCS credentials) ─────────────────────

    #[tokio::test]
    async fn test_gcs_middleware() {
        let credentials = match std::env::var("GOOGLE_CLOUD_TEST_KEY_JSON") {
            Ok(credentials) if !credentials.is_empty() => credentials,
            Err(_) | Ok(_) => {
                eprintln!("Skipping test as GOOGLE_CLOUD_TEST_KEY_JSON is not set");
                return;
            }
        };
        println!("Running GCS Test");

        // We have to set GOOGLE_APPLICATION_CREDENTIALS to the path of the JSON key
        // file
        let key_file = tempfile::NamedTempFile::with_suffix(".json").unwrap();
        std::fs::write(&key_file, credentials).unwrap();

        let prev_value = std::env::var("GOOGLE_APPLICATION_CREDENTIALS").ok();
        std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", key_file.path());

        let client = reqwest_middleware::ClientBuilder::new(Client::new())
            .with(GCSMiddleware::default())
            .build();

        let url = "gcs://test-channel/noarch/repodata.json";
        let response = client.get(url).send().await.unwrap();
        assert!(response.status().is_success());

        let url = "gcs://test-channel-nonexist/noarch/repodata.json";
        let response = client.get(url).send().await.unwrap();
        assert!(response.status().is_client_error());

        if let Some(value) = prev_value {
            std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", value);
        } else {
            std::env::remove_var("GOOGLE_APPLICATION_CREDENTIALS");
        }
    }
}
