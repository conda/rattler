//! An owned cursor over the members of a tar stream.
//!
//! `tar_codec` lends each member payload from the archive it is read from, so
//! a member cannot be kept while its archive is owned elsewhere. A
//! [`SectionEntry`](super::SectionEntry) needs exactly that: it is handed out
//! by value and read later. The archive is therefore driven inside a single
//! future that keeps the current member alive between requests. That future
//! only runs while a method of [`TarCursor`] polls it; nothing is spawned.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::Poll;

use tar_codec::{Archive as _, Member, TarArchive};
use tokio::sync::{mpsc, oneshot};

use super::{ArchiveEntryKind, DynReader};
use crate::ExtractError;
use crate::tokio::shared::{open_tar, read_member};

/// The metadata of a tar member, detached from its payload.
pub(super) struct MemberInfo {
    /// The raw archive path of the member.
    pub(super) path: String,
    pub(super) kind: ArchiveEntryKind,
    /// The declared body size; zero for members without a body.
    pub(super) size: u64,
    /// The raw link target of a symbolic or hard link.
    pub(super) link_target: Option<String>,
}

impl MemberInfo {
    fn of<P>(member: &Member<P>) -> Self {
        let path = member.metadata().path.clone();
        let (kind, size, link_target) = match member {
            Member::File { size, .. } => (ArchiveEntryKind::File, *size, None),
            Member::Directory { .. } => (ArchiveEntryKind::Directory, 0, None),
            Member::SymbolicLink { target, .. } => {
                (ArchiveEntryKind::Symlink, 0, Some(target.clone()))
            }
            Member::HardLink { target, size, .. } => {
                (ArchiveEntryKind::Hardlink, *size, Some(target.clone()))
            }
            Member::Special { .. } => (ArchiveEntryKind::Other, 0, None),
        };
        Self {
            path,
            kind,
            size,
            link_target,
        }
    }
}

type Reply<T> = oneshot::Sender<Result<T, ExtractError>>;

enum Request {
    /// Advance to the next member and report its metadata.
    Next(Reply<Option<MemberInfo>>),
    /// Read the body of the current member.
    Read(Reply<Vec<u8>>),
}

/// An owned handle on a tar stream that yields one member at a time.
pub(super) struct TarCursor {
    requests: mpsc::UnboundedSender<Request>,
    /// The future driving the archive; `None` once it has finished.
    driver: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
    generation: u64,
}

impl TarCursor {
    pub(super) fn new(reader: DynReader) -> Self {
        let (requests, receiver) = mpsc::unbounded_channel();
        Self {
            requests,
            driver: Some(Box::pin(drive(open_tar(reader), receiver))),
            generation: 0,
        }
    }

    /// The number of members yielded so far, identifying the current one.
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// Advances to the next member and returns its metadata.
    pub(super) async fn next(&mut self) -> Result<Option<MemberInfo>, ExtractError> {
        let (reply, response) = oneshot::channel();
        let info = self.call(Request::Next(reply), response).await?;
        if info.is_some() {
            self.generation += 1;
        }
        Ok(info)
    }

    /// Reads the body of the current member.
    pub(super) async fn read(&mut self) -> Result<Vec<u8>, ExtractError> {
        let (reply, response) = oneshot::channel();
        self.call(Request::Read(reply), response).await
    }

    /// Sends a request and polls the driver until it has replied.
    async fn call<T>(
        &mut self,
        request: Request,
        mut response: oneshot::Receiver<Result<T, ExtractError>>,
    ) -> Result<T, ExtractError> {
        if self.requests.send(request).is_err() {
            return Err(stream_finished());
        }
        std::future::poll_fn(|cx| {
            // Let the driver work on the request before checking for a reply.
            let finished = self
                .driver
                .as_mut()
                .is_some_and(|driver| driver.as_mut().poll(cx).is_ready());
            if finished {
                self.driver = None;
            }
            match Pin::new(&mut response).poll(cx) {
                Poll::Ready(Ok(result)) => Poll::Ready(result),
                Poll::Ready(Err(_)) => Poll::Ready(Err(stream_finished())),
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    }
}

/// Drives a tar archive on behalf of a [`TarCursor`].
///
/// A reply that cannot be delivered because the requesting future was dropped
/// is kept and served to the next request of the same kind, so a cancelled
/// call never loses a member or its body.
async fn drive(mut archive: TarArchive<DynReader>, mut requests: mpsc::UnboundedReceiver<Request>) {
    // A `Next` request that arrived while serving a member. The member has
    // been dropped so the archive can advance.
    let mut carried: Option<Reply<Option<MemberInfo>>> = None;
    let mut failed = false;
    loop {
        let reply = match carried.take() {
            Some(reply) => reply,
            None => match requests.recv().await {
                Some(Request::Next(reply)) => reply,
                Some(Request::Read(reply)) => {
                    let _ = reply.send(Err(no_current_member()));
                    continue;
                }
                None => return,
            },
        };
        if failed {
            let _ = reply.send(Err(stream_failed()));
            continue;
        }
        // `next_member` is called directly instead of going through
        // `Archive::members`: the payload of a `TarArchive` borrows the
        // archive either way, and the generic wrapper keeps the compiler from
        // proving this future `Send`.
        let mut member = match archive.next_member().await {
            Ok(Some(member)) => member,
            Ok(None) => {
                let _ = reply.send(Ok(None));
                continue;
            }
            Err(err) => {
                failed = true;
                let _ = reply.send(Err(err.into()));
                continue;
            }
        };
        let path = PathBuf::from(&member.metadata().path);
        let mut unreported = match reply.send(Ok(Some(MemberInfo::of(&member)))) {
            Ok(()) => None,
            Err(unsent) => unsent.ok().flatten(),
        };
        let mut unread = None;
        // Serve requests for this member until the stream is advanced.
        loop {
            match requests.recv().await {
                None => return,
                Some(Request::Next(reply)) => {
                    let Some(info) = unreported.take() else {
                        carried = Some(reply);
                        break;
                    };
                    if let Err(unsent) = reply.send(Ok(Some(info))) {
                        unreported = unsent.ok().flatten();
                    }
                }
                Some(Request::Read(reply)) => {
                    let result = if unreported.is_some() {
                        // The caller never learned about this member, so the
                        // entry it holds is an earlier one.
                        Err(stream_advanced())
                    } else if let Some(result) = unread.take() {
                        result
                    } else {
                        let result = read_member(&mut member, &path).await;
                        // A stream error must not let the next request look
                        // like a clean end of the archive.
                        if let Err(err) = &result
                            && !matches!(err, ExtractError::LinksNotFollowed(_))
                        {
                            failed = true;
                        }
                        result
                    };
                    if let Err(unsent) = reply.send(result) {
                        unread = Some(unsent);
                    }
                }
            }
        }
    }
}

fn stream_finished() -> ExtractError {
    ExtractError::IoError(std::io::Error::other(
        "the tar stream is no longer available",
    ))
}

fn stream_failed() -> ExtractError {
    ExtractError::IoError(std::io::Error::other(
        "the tar stream cannot be read past an earlier error",
    ))
}

fn stream_advanced() -> ExtractError {
    ExtractError::IoError(std::io::Error::other(
        "the entry cannot be read because the stream has advanced past it",
    ))
}

fn no_current_member() -> ExtractError {
    ExtractError::IoError(std::io::Error::other("no tar entry is current"))
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::Path;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    use tokio::io::{AsyncRead, ReadBuf};

    use super::super::{ArchiveEntryKind, SectionStream};
    use crate::ExtractError;

    /// Returns `Pending` before every read so futures can be dropped midway.
    struct Stuttering {
        data: Vec<u8>,
        position: usize,
        stall: bool,
    }

    impl AsyncRead for Stuttering {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.stall {
                self.stall = false;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            self.stall = true;
            // One block per read, so every member needs reads of its own.
            let len = buf
                .remaining()
                .min(512)
                .min(self.data.len() - self.position);
            buf.put_slice(&self.data[self.position..self.position + len]);
            self.position += len;
            Poll::Ready(Ok(()))
        }
    }

    fn tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *data).unwrap();
        }
        builder.into_inner().unwrap()
    }

    fn stream(data: Vec<u8>) -> SectionStream {
        SectionStream::new(
            Box::new(Stuttering {
                data,
                position: 0,
                stall: true,
            }),
            None,
        )
    }

    /// Polls `future` once with a no-op waker and asserts it is not done.
    fn poll_once<F: Future>(future: Pin<&mut F>) {
        let mut cx = Context::from_waker(Waker::noop());
        assert!(future.poll(&mut cx).is_pending());
    }

    #[tokio::test]
    async fn entries_are_read_in_order_and_only_while_current() {
        let mut stream = stream(tar(&[("a.txt", b"first"), ("b.txt", b"second")]));

        let mut first = stream.next_entry().await.unwrap().unwrap();
        assert_eq!(first.path(), Path::new("a.txt"));
        assert_eq!(first.kind(), ArchiveEntryKind::File);
        assert_eq!(first.size().unwrap(), 5);
        assert_eq!(first.read().await.unwrap(), b"first");
        // The body is consumed; reading again yields nothing but no error.
        assert_eq!(first.read().await.unwrap(), b"");

        let mut second = stream.next_entry().await.unwrap().unwrap();
        assert_eq!(second.path(), Path::new("b.txt"));
        let err = first.read().await.unwrap_err();
        assert!(
            matches!(&err, ExtractError::IoError(_)) && err.to_string().contains("advanced"),
            "{err}"
        );
        assert_eq!(second.read().await.unwrap(), b"second");

        assert!(stream.next_entry().await.unwrap().is_none());
        assert!(stream.next_entry().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn unread_entries_are_skipped() {
        let mut stream = stream(tar(&[("a.txt", b"first"), ("b.txt", b"second")]));
        let _first = stream.next_entry().await.unwrap().unwrap();
        let mut second = stream.next_entry().await.unwrap().unwrap();
        assert_eq!(second.read().await.unwrap(), b"second");
    }

    #[tokio::test]
    async fn cancelled_calls_lose_neither_entries_nor_bodies() {
        let mut stream = stream(tar(&[("a.txt", b"first"), ("b.txt", b"second")]));

        // The driver takes the request and stalls on the reader; the caller
        // gives up before the reply arrives.
        poll_once(std::pin::pin!(stream.next_entry()));
        let mut first = stream.next_entry().await.unwrap().unwrap();
        assert_eq!(first.path(), Path::new("a.txt"));

        poll_once(std::pin::pin!(first.read()));
        assert_eq!(first.read().await.unwrap(), b"first");

        poll_once(std::pin::pin!(stream.next_entry()));
        let mut second = stream.next_entry().await.unwrap().unwrap();
        assert_eq!(second.path(), Path::new("b.txt"));
        assert_eq!(second.read().await.unwrap(), b"second");
        assert!(stream.next_entry().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn truncated_streams_report_an_eof_error() {
        let mut data = tar(&[("a.txt", &[7u8; 2000]), ("b.txt", b"second")]);
        data.truncate(512 + 1000);
        let mut stream = stream(data);

        let mut first = stream.next_entry().await.unwrap().unwrap();
        match first.read().await.unwrap_err() {
            ExtractError::IoError(err) => assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof),
            err => panic!("expected an I/O error, got {err:?}"),
        }
        assert!(stream.next_entry().await.is_err());
    }
}
