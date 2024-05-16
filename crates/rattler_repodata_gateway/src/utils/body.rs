use bytes::Bytes;
use futures::Stream;
use pin_project_lite::pin_project;
use std::{
    collections::VecDeque,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};

/// A helper trait to convert a stream of bytes coming from a request body into
/// another type.
pub trait BodyStreamExt<E>: Sized {
    /// Reads the contents of a body stream as bytes.
    fn bytes(self) -> BytesCollect<Self, E>;

    /// Read the contents of a body stream as text.
    async fn text(self) -> Result<String, E>;
}

impl<E, S: Stream<Item = Result<Bytes, E>>> BodyStreamExt<E> for S {
    fn bytes(self) -> BytesCollect<Self, E> {
        BytesCollect::new(self)
    }

    async fn text(self) -> Result<String, E> {
        let full = self.bytes().await?;
        let text = String::from_utf8_lossy(&full);
        Ok(text.into_owned())
    }
}

pin_project! {
    #[project = BytesCollectProj]
    pub struct BytesCollect<S, E> {
        #[pin]
        stream: S,
        bytes: VecDeque<Bytes>,
        _err: PhantomData<E>,
    }
}

impl<S, E> BytesCollect<S, E> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            bytes: VecDeque::new(),
            _err: PhantomData,
        }
    }
}

impl<E, S: Stream<Item = Result<Bytes, E>>> Future for BytesCollect<S, E> {
    type Output = Result<Vec<u8>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        loop {
            match this.stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.bytes.push_back(chunk);
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                Poll::Ready(None) => {
                    let mut result = Vec::with_capacity(this.bytes.iter().map(Bytes::len).sum());
                    for chunk in this.bytes.iter() {
                        result.extend_from_slice(chunk);
                    }
                    return Poll::Ready(Ok(result));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
