#![allow(dead_code)]

//! A thread-safe, deduplicating map that ensures expensive computations are
//! executed only once per key, even when multiple concurrent requests are made.
//!
//! This map is designed for scenarios where multiple async tasks might request
//! the same resource simultaneously. Instead of performing duplicate work, the
//! `CoalescedMap` ensures that only the first request for a given key executes
//! the initialization function, while subsequent concurrent requests wait for
//! and receive the same result.
//!
//! The implementation uses `DashMap` for thread-safe storage and
//! `tokio::sync::broadcast` channels for coordinating between concurrent
//! waiters.
//!
//! ## Example
//!
//! ```rust,ignore
//! use crate::utils::coalesced_map::{CoalescedMap, CoalescedGetError};
//! use std::{sync::Arc, time::Duration};
//!
//! #[tokio::main]
//! async fn main() {
//!     let cache: CoalescedMap<String, Arc<String>> = CoalescedMap::new();
//!
//!     // Simulate multiple concurrent requests for the same expensive resource
//!     let key = "expensive_computation".to_string();
//!     
//!     let handle1 = {
//!         let cache = cache.clone();
//!         let key = key.clone();
//!         tokio::spawn(async move {
//!             cache.get_or_try_init(key, || async {
//!                 // Simulate expensive work (e.g., network request, file I/O)
//!                 tokio::time::sleep(Duration::from_millis(100)).await;
//!                 println!("Performing expensive computation...");
//!                 Ok(Arc::new("computed_result".to_string()))
//!             }).await
//!         })
//!     };
//!     
//!     let handle2 = {
//!         let cache = cache.clone();
//!         let key = key.clone();
//!         tokio::spawn(async move {
//!             cache.get_or_try_init(key, || async {
//!                 // This function will NOT be executed due to coalescing
//!                 println!("This should not print!");
//!                 Ok(Arc::new("unused".to_string()))
//!             }).await
//!         })
//!     };
//!
//!     let result1 = handle1.await.unwrap().unwrap();
//!     let result2 = handle2.await.unwrap().unwrap();
//!     
//!     // Both results are identical (same Arc instance)
//!     assert!(Arc::ptr_eq(&result1, &result2));
//!     assert_eq!(*result1, "computed_result");
//! }
//! ```

use std::{
    fmt,
    hash::Hash,
    sync::{Arc, Weak},
};

use dashmap::{mapref::entry::Entry, DashMap};
use tokio::sync::broadcast;

/// Error type returned by [`CoalescedMap::get_or_try_init`].
///
/// When multiple tasks race to initialize the same key, only the winner runs
/// the provided initializer. Other tasks subscribe to a broadcast channel. If
/// the initializer returns an error, the winner returns `Init(err)` and the
/// channel sender is dropped, causing subscribers to receive
/// `CoalescedRequestFailed`.
#[derive(Debug)]
pub enum CoalescedGetError<E> {
    /// The initialization future returned an error.
    Init(E),
    /// The in-flight coalesced request failed before publishing a value.
    CoalescedRequestFailed,
}

impl<E: fmt::Display> fmt::Display for CoalescedGetError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoalescedGetError::Init(e) => write!(f, "initializer failed: {e}"),
            CoalescedGetError::CoalescedRequestFailed => {
                write!(f, "a coalesced request failed")
            }
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for CoalescedGetError<E> {}

/// A thread-safe map that deduplicates concurrent async initialization
/// requests.
///
/// When multiple tasks concurrently request the same key, only the first task
/// executes the initialization function. Other tasks automatically wait for the
/// result via a broadcast channel. This prevents duplicate work and ensures
/// consistent results across all waiters.
///
/// Internally, each entry can be in one of two states:
/// - **Pending**: An initialization is in progress, tracked by a broadcast
///   sender
/// - **Fetched**: The value has been computed and cached for immediate
///   retrieval
#[derive(Clone)]
pub struct CoalescedMap<K, V>
where
    K: Eq + Hash,
    V: Clone,
{
    map: DashMap<K, PendingOrFetched<V>>,
}

impl<K, V> Default for CoalescedMap<K, V>
where
    K: Eq + Hash,
    V: Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> CoalescedMap<K, V>
where
    K: Eq + Hash,
    V: Clone,
{
    /// Creates an empty `CoalescedMap`.
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    /// Returns the number of entries currently stored (including pending).
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Returns true if the map contains no entries.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl<K, V> CoalescedMap<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone + Send + Sync + 'static,
{
    /// Returns the value for `key`, initializing it at most once using the
    /// provided async `init` function. Concurrent calls for the same key are
    /// coalesced: only the first executes `init`; others await the result.
    ///
    /// On successful initialization, the value is inserted and shared with all
    /// waiters. If the initializer returns an error, the error is returned to
    /// the caller that executed it, while other waiters receive
    /// `CoalescedRequestFailed`.
    pub async fn get_or_try_init<E, Fut, F>(
        &self,
        key: K,
        init: F,
    ) -> Result<V, CoalescedGetError<E>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<V, E>>,
    {
        // Attempt to occupy or observe the entry.
        let sender = match self.map.entry(key.clone()) {
            Entry::Vacant(entry) => {
                // First caller: create a broadcast sender and mark as pending.
                let (tx, _) = broadcast::channel(1);
                let tx = Arc::new(tx);
                entry.insert(PendingOrFetched::Pending(Arc::downgrade(&tx)));
                tx
            }
            Entry::Occupied(mut entry) => match entry.get() {
                PendingOrFetched::Fetched(v) => return Ok(v.clone()),
                PendingOrFetched::Pending(weak_tx) => {
                    if let Some(tx) = weak_tx.upgrade() {
                        // Subscribe before dropping the entry to avoid missing the send.
                        let mut rx = tx.subscribe();

                        // We only care about the receiver, drop the sender immediately.
                        drop(tx);

                        // Drop the entry to allow others to query the map
                        drop(entry);

                        return rx
                            .recv()
                            .await
                            .map_err(|_err| CoalescedGetError::CoalescedRequestFailed);
                    }

                    // Previous sender dropped without publishing; become the new initializer.
                    let (tx, _) = broadcast::channel(1);
                    let tx = Arc::new(tx);
                    entry.insert(PendingOrFetched::Pending(Arc::downgrade(&tx)));
                    tx
                }
            },
        };

        // We are the initializer for this key. Run the init future.
        match init().await {
            Ok(value) => {
                // Store the value and notify any waiters.
                self.map
                    .insert(key, PendingOrFetched::Fetched(value.clone()));
                let _ = sender.send(value.clone());
                Ok(value)
            }
            Err(err) => Err(CoalescedGetError::Init(err)),
        }
    }

    /// Attempts to get a previously fetched value without triggering
    /// initialization. Returns `Some` if the entry is present and fetched.
    pub fn get(&self, key: &K) -> Option<V> {
        self.map.get(key).and_then(|g| match g.value() {
            PendingOrFetched::Fetched(v) => Some(v.clone()),
            PendingOrFetched::Pending(_) => None,
        })
    }

    /// Clears all entries matching the predicate, similar to `DashMap::retain`.
    pub fn retain<F>(&self, mut f: F)
    where
        F: FnMut(&K, &PendingOrFetched<V>) -> bool,
    {
        self.map.retain(|k, v| f(k, v));
    }
}

/// Internal state for a coalesced entry.
#[derive(Clone)]
pub enum PendingOrFetched<T> {
    /// There is an in-flight initialization; waiters subscribe to the sender.
    Pending(Weak<broadcast::Sender<T>>),
    /// The value has been initialized.
    Fetched(T),
}

#[cfg(test)]
mod tests {
    use std::{
        future::pending,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    use super::*;

    #[tokio::test]
    async fn test_basic_get_or_try_init() {
        let map: CoalescedMap<String, String> = CoalescedMap::new();

        let result = map
            .get_or_try_init("key1".to_string(), || async {
                Ok::<_, &str>("value1".to_string())
            })
            .await
            .unwrap();

        assert_eq!(result, "value1");

        // Second call should return cached value without calling init function
        let result2 = map
            .get_or_try_init("key1".to_string(), || async {
                Ok::<_, &str>("should_not_be_called".to_string())
            })
            .await
            .unwrap();

        assert_eq!(result2, "value1");
    }

    #[tokio::test]
    async fn test_get_if_fetched() {
        let map: CoalescedMap<String, String> = CoalescedMap::new();

        // Should return None for non-existent key
        assert_eq!(map.get(&"key1".to_string()), None);

        // Initialize a value
        map.get_or_try_init("key1".to_string(), || async {
            Ok::<_, &str>("value1".to_string())
        })
        .await
        .unwrap();

        // Should return Some for initialized key
        assert_eq!(map.get(&"key1".to_string()), Some("value1".to_string()));
    }

    #[tokio::test]
    async fn test_concurrent_initialization() {
        let map: Arc<CoalescedMap<String, Arc<String>>> = Arc::new(CoalescedMap::new());
        let call_count = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(tokio::sync::Barrier::new(10));

        // Start multiple concurrent tasks that try to initialize the same key
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let map = map.clone();
                let call_count = call_count.clone();
                let barrier = barrier.clone();
                tokio::spawn(async move {
                    // Wait for all tasks to be ready before starting
                    barrier.wait().await;

                    map.get_or_try_init("shared_key".to_string(), || {
                        let call_count = call_count.clone();
                        async move {
                            // Track how many times the init function is called
                            call_count.fetch_add(1, Ordering::SeqCst);

                            Ok::<_, &str>(Arc::new(format!("value_from_task_{i}")))
                        }
                    })
                    .await
                })
            })
            .collect();

        // Wait for all tasks to complete
        let results: Vec<_> = futures::future::try_join_all(handles)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        // The initialization function should only be called once
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // All results should be identical (same Arc instance)
        let first_result = &results[0];
        for result in &results {
            assert!(Arc::ptr_eq(first_result, result));
        }
    }

    #[tokio::test]
    async fn test_error_handling() {
        let map: CoalescedMap<String, String> = CoalescedMap::new();

        // Test that errors are properly propagated
        let result = map
            .get_or_try_init("error_key".to_string(), || async {
                Err("initialization failed")
            })
            .await;

        match result {
            Err(CoalescedGetError::Init(err)) => assert_eq!(err, "initialization failed"),
            _ => panic!("Expected Init error"),
        }

        // Key should not be cached after error
        assert_eq!(map.get(&"error_key".to_string()), None);

        // Subsequent call should retry initialization
        let success_result = map
            .get_or_try_init("error_key".to_string(), || async {
                Ok::<_, &str>("success_value".to_string())
            })
            .await
            .unwrap();

        assert_eq!(success_result, "success_value");
    }

    #[tokio::test]
    async fn test_concurrent_error_handling() {
        let map = Arc::new(CoalescedMap::new());
        let init_calls = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(tokio::sync::Barrier::new(2));

        // Start multiple concurrent tasks, one will fail
        let handles: Vec<_> = (0..5)
            .map(|i| {
                let map = map.clone();
                let init_calls = init_calls.clone();
                let barrier = barrier.clone();
                tokio::spawn(async move {
                    map.get_or_try_init("fail_key".to_string(), || {
                        let init_calls = init_calls.clone();
                        async move {
                            init_calls.fetch_add(1, Ordering::SeqCst);

                            if i == 0 {
                                // First task succeeds after a delay
                                barrier.wait().await;
                                Ok(format!("success_{i}"))
                            } else {
                                // Other tasks would fail, but they should be coalesced
                                // and receive the successful result
                                Err(format!("error_{i}"))
                            }
                        }
                    })
                    .await
                })
            })
            .collect();

        // Wait for all tasks to reach the barrier.
        barrier.wait().await;

        // Wait for all tasks to complete.
        let results: Vec<_> = futures::future::join_all(handles).await;

        // Since
        assert_eq!(init_calls.load(Ordering::SeqCst), 1);

        // All tasks should get the same successful result
        for result in results {
            let value = result.unwrap().unwrap();
            assert_eq!(value, "success_0");
        }
    }

    #[tokio::test]
    async fn test_different_keys() {
        let map = Arc::new(CoalescedMap::new());

        // Initialize different keys concurrently
        let handles: Vec<_> = (0..5)
            .map(|i| {
                let map = map.clone();
                tokio::spawn(async move {
                    let key = format!("key_{i}");
                    let value = format!("value_{i}");
                    map.get_or_try_init(key.clone(), || async move { Ok::<_, &str>(value) })
                        .await
                        .map(|v| (key, v))
                })
            })
            .collect();

        let results: Vec<_> = futures::future::try_join_all(handles)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        // Each key should have its own value
        for (i, (key, value)) in results.into_iter().enumerate() {
            assert_eq!(key, format!("key_{i}"));
            assert_eq!(value, format!("value_{i}"));
        }

        // Verify all keys are cached
        for i in 0..5 {
            let key = format!("key_{i}");
            let expected_value = format!("value_{i}");
            assert_eq!(map.get(&key), Some(expected_value));
        }
    }

    #[tokio::test]
    async fn test_retain_functionality() {
        let map: CoalescedMap<String, String> = CoalescedMap::new();

        // Initialize several keys
        for i in 0..5 {
            let key = format!("key_{i}");
            let value = format!("value_{i}");
            map.get_or_try_init(key, || async move { Ok::<_, &str>(value) })
                .await
                .unwrap();
        }

        // Retain only keys with even numbers
        map.retain(|key, _| {
            if let Some(num_str) = key.strip_prefix("key_") {
                if let Ok(num) = num_str.parse::<i32>() {
                    return num % 2 == 0;
                }
            }
            false
        });

        // Check that only even keys remain
        assert_eq!(map.get(&"key_0".to_string()), Some("value_0".to_string()));
        assert_eq!(map.get(&"key_1".to_string()), None);
        assert_eq!(map.get(&"key_2".to_string()), Some("value_2".to_string()));
        assert_eq!(map.get(&"key_3".to_string()), None);
        assert_eq!(map.get(&"key_4".to_string()), Some("value_4".to_string()));
    }

    #[tokio::test]
    async fn test_coalesced_request_failed_error() {
        let map = Arc::new(CoalescedMap::new());
        // Use a 3-party barrier so the test task can coordinate
        // that both spawned tasks reached the rendezvous before aborting.
        let barrier = Arc::new(tokio::sync::Barrier::new(3));

        let map1 = map.clone();
        let barrier1 = barrier.clone();
        let handle1 = tokio::spawn(async move {
            map1.get_or_try_init("test_key".to_string(), || async move {
                barrier1.wait().await;
                // Simulate a task that gets cancelled/dropped
                // Sleep forever to ensure it doesn't complete
                // before we abort it from the test thread.
                let () = pending().await;
                Ok::<_, &str>("value".to_string())
            })
            .await
        });

        let map2 = map.clone();
        let barrier2 = barrier.clone();
        let handle2 = tokio::spawn(async move {
            // Wait a bit to ensure the first task starts first
            barrier2.wait().await;

            // This should subscribe to the first task's broadcast
            map2.get_or_try_init("test_key".to_string(), || async move {
                Ok::<_, &str>("should_not_be_called".to_string())
            })
            .await
        });

        // Wait till both tasks reach the barrier
        barrier.wait().await;

        // Cancel the first task to simulate a coalesced request failure
        handle1.abort();

        // The second task should receive a CoalescedRequestFailed error
        let result = handle2.await.unwrap();
        match result {
            Err(CoalescedGetError::CoalescedRequestFailed) => {
                // This is expected
            }
            _ => panic!("Expected CoalescedRequestFailed error, got {result:?}"),
        }
    }

    #[tokio::test]
    async fn test_empty_map() {
        let map: CoalescedMap<String, String> = CoalescedMap::new();

        assert_eq!(map.len(), 0);
        assert!(map.is_empty());
        assert_eq!(map.get(&"nonexistent".to_string()), None);

        // Add one item
        map.get_or_try_init("key".to_string(), || async {
            Ok::<_, &str>("value".to_string())
        })
        .await
        .unwrap();

        assert_eq!(map.len(), 1);
        assert!(!map.is_empty());
    }
}
