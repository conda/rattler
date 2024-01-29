use pin_project_lite::pin_project;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncBufRead, AsyncRead, ReadBuf};

/// Describes the encoding of a stream
#[derive(Debug, Copy, Clone)]
pub enum Encoding {
    Passthrough,
    GZip,
    Bz2,
    Zst,
}

impl<'a> From<&'a reqwest::Response> for Encoding {
    fn from(res: &'a reqwest::Response) -> Self {
        if is_response_encoded_with(res, "gzip") {
            Encoding::GZip
        } else {
            Encoding::Passthrough
        }
    }
}

pin_project! {
    #[project = DecoderProj]
    pub enum Decoder<T: AsyncBufRead> {
        Passthrough { #[pin] inner: T },
        GZip { #[pin] inner: async_compression::tokio::bufread::GzipDecoder<T> },
        Bz2 { #[pin] inner: async_compression::tokio::bufread::BzDecoder<T> },
        Zst { #[pin] inner: async_compression::tokio::bufread::ZstdDecoder<T> },
    }
}

impl<T: AsyncBufRead> AsyncRead for Decoder<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.project() {
            DecoderProj::Passthrough { inner } => inner.poll_read(cx, buf),
            DecoderProj::GZip { inner } => inner.poll_read(cx, buf),
            DecoderProj::Bz2 { inner } => inner.poll_read(cx, buf),
            DecoderProj::Zst { inner } => inner.poll_read(cx, buf),
        }
    }
}

pub trait AsyncEncoding: AsyncBufRead + Sized {
    /// Creates a new object that decompresses the incoming bytes on the fly.
    fn decode(self, encoding: Encoding) -> Decoder<Self>;
}

impl<T: AsyncBufRead> AsyncEncoding for T {
    fn decode(self, encoding: Encoding) -> Decoder<Self> {
        match encoding {
            Encoding::Passthrough => Decoder::Passthrough { inner: self },
            Encoding::GZip => Decoder::GZip {
                inner: async_compression::tokio::bufread::GzipDecoder::new(self),
            },
            Encoding::Bz2 => Decoder::Bz2 {
                inner: async_compression::tokio::bufread::BzDecoder::new(self),
            },
            Encoding::Zst => Decoder::Zst {
                inner: async_compression::tokio::bufread::ZstdDecoder::new(self),
            },
        }
    }
}

/// Returns true if the response is encoded as the specified encoding.
fn is_response_encoded_with(response: &reqwest::Response, encoding_str: &str) -> bool {
    let headers = response.headers();
    headers
        .get_all(reqwest::header::CONTENT_ENCODING)
        .iter()
        .any(|enc| enc == encoding_str)
        || headers
            .get_all(reqwest::header::TRANSFER_ENCODING)
            .iter()
            .any(|enc| enc == encoding_str)
}
