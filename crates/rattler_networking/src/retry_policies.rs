//! Reexports the trait [`RetryPolicy`] from the `retry_policies` crate as well as all
//! implementations.
//!
//! This module also provides the [`DoNotRetryPolicy`] which is useful if you do not want to retry
//! anything.

pub use retry_policies::{policies::*, Jitter, RetryDecision, RetryPolicy};
use std::time::SystemTime;

/// A simple [`RetryPolicy`] that just never retries.
#[derive(Clone, Copy)]
pub struct DoNotRetryPolicy;
impl RetryPolicy for DoNotRetryPolicy {
    fn should_retry(&self, _: SystemTime, _: u32) -> RetryDecision {
        RetryDecision::DoNotRetry
    }
}

/// Returns the default retry policy that can be used .
///
/// This is useful if you just do not care about a retry policy and you just want something
/// sensible. Note that the behavior of what is "sensible" might change over time.
pub fn default_retry_policy() -> ExponentialBackoff {
    ExponentialBackoff::builder().build_with_max_retries(3)
}
