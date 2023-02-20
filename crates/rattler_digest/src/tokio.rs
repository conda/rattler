use super::HashingWriter;
use digest::Digest;
use std::{
    io::Error,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::AsyncWrite;

impl<W: AsyncWrite + Unpin, D: Digest> AsyncWrite for HashingWriter<W, D> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        // pin-project the writer
        let (writer, hasher) = unsafe {
            let this = self.get_unchecked_mut();
            (Pin::new_unchecked(&mut this.writer), &mut this.hasher)
        };

        match writer.poll_write(cx, buf) {
            Poll::Ready(Ok(bytes)) => {
                hasher.update(&buf[..bytes]);
                Poll::Ready(Ok(bytes))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        // This is okay because `writer` is pinned when `self` is.
        let writer = unsafe { self.map_unchecked_mut(|s| &mut s.writer) };
        writer.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        // This is okay because `writer` is pinned when `self` is.
        let writer = unsafe { self.map_unchecked_mut(|s| &mut s.writer) };
        writer.poll_flush(cx)
    }
}
