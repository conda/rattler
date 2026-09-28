//! Boxed future and stream aliases shared by the gateway queries.
//!
//! The gateway queries need to box futures and streams to store them in
//! collections and to return them from public methods. Which box to use
//! depends on the target: on wasm there are no threads to send a task
//! between, so the futures and streams there are not `Send`. These aliases
//! and constructors hide that difference from the query implementations.

use std::future::Future;

use futures::{FutureExt, Stream, StreamExt};

/// A boxed future used by the gateway queries. Not `Send` on wasm, where
/// there are no threads to send it between.
#[cfg(target_arch = "wasm32")]
pub(super) type BoxFuture<T> = futures::future::LocalBoxFuture<'static, T>;

/// Box `future` as a [`BoxFuture`].
#[cfg(target_arch = "wasm32")]
pub(super) fn box_future<T, F: Future<Output = T> + 'static>(future: F) -> BoxFuture<T> {
    future.boxed_local()
}

/// A boxed future used by the gateway queries.
#[cfg(not(target_arch = "wasm32"))]
pub(super) type BoxFuture<T> = futures::future::BoxFuture<'static, T>;

/// Box `future` as a [`BoxFuture`].
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn box_future<T, F: Future<Output = T> + Send + 'static>(future: F) -> BoxFuture<T> {
    future.boxed()
}

/// A boxed stream returned from a gateway query, so callers can poll it
/// without pinning it themselves. Not `Send` on wasm, where there are no
/// threads to send it between.
#[cfg(target_arch = "wasm32")]
pub type BoxStream<T> = futures::stream::LocalBoxStream<'static, T>;

/// Box `stream` as a [`BoxStream`].
#[cfg(target_arch = "wasm32")]
pub(super) fn box_stream<T, S: Stream<Item = T> + 'static>(stream: S) -> BoxStream<T> {
    stream.boxed_local()
}

/// A boxed stream returned from a gateway query, so callers can poll it
/// without pinning it themselves.
#[cfg(not(target_arch = "wasm32"))]
pub type BoxStream<T> = futures::stream::BoxStream<'static, T>;

/// Box `stream` as a [`BoxStream`].
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn box_stream<T, S: Stream<Item = T> + Send + 'static>(stream: S) -> BoxStream<T> {
    stream.boxed()
}
