//! Shared functionality for reading tar streams asynchronously.

use std::path::Path;

use tar_codec::{
    DecodePolicy, Member, MemberPayload, PaxDecodePolicy, PaxVendorExtensionPolicy, TarArchive,
};
use tokio::io::AsyncRead;

use crate::ExtractError;

/// Buffer size for async I/O operations (128KB).
pub(crate) const DEFAULT_BUF_SIZE: usize = 128 * 1024;

/// Cap for upfront buffer allocations based on (untrusted) tar header sizes.
const MAX_PREALLOC: u64 = 4 * 1024 * 1024;

/// Bytes requested per payload chunk when reading a member into memory.
const PAYLOAD_CHUNK_SIZE: usize = 64 * 1024;

/// Opens the tar stream of a conda package for reading.
///
/// Packages are written by a range of tools: conda-build uses Python's
/// `tarfile`, which fills the complete `uname`/`gname` header fields for
/// 32-byte names, rattler-build uses the `tar` crate, and older packages were
/// written by libarchive, whose pax headers carry vendor records. Vendor
/// records are accepted and ignored, as previous readers did.
pub(crate) fn open_tar<R: AsyncRead + Unpin>(reader: R) -> TarArchive<R> {
    let policy = DecodePolicy::default()
        .allow_header_uname_and_gname_without_terminator(true)
        .pax_policy(
            PaxDecodePolicy::default()
                .vendor_extension_policy(PaxVendorExtensionPolicy::AllowUnknown),
        );
    TarArchive::new(reader).with_policy(policy)
}

/// Describes a link entry for [`ExtractError::LinksNotFollowed`].
pub(crate) fn describe_link(path: &Path, target: &str) -> String {
    format!("'{}' (links to '{target}')", path.display())
}

/// Reads the complete body of a tar member.
///
/// `path` names the member in errors. Links are never followed: reading one is
/// an error. Directories and special files have an empty body.
pub(crate) async fn read_member<P>(
    member: &mut Member<P>,
    path: &Path,
) -> Result<Vec<u8>, ExtractError>
where
    P: MemberPayload<Error = tar_codec::DecodeError>,
{
    match member {
        Member::File { size, payload, .. } => read_payload(payload, *size).await,
        Member::SymbolicLink { target, .. } | Member::HardLink { target, .. } => Err(
            ExtractError::LinksNotFollowed(vec![describe_link(path, target)]),
        ),
        Member::Directory { .. } | Member::Special { .. } => Ok(Vec::new()),
    }
}

/// Reads a payload into memory, capping the upfront allocation derived from
/// the untrusted declared size.
async fn read_payload<P>(payload: &mut P, size: u64) -> Result<Vec<u8>, ExtractError>
where
    P: MemberPayload<Error = tar_codec::DecodeError>,
{
    let mut contents = Vec::with_capacity(size.min(MAX_PREALLOC) as usize);
    let mut chunk = Vec::new();
    while payload.next_chunk(&mut chunk, PAYLOAD_CHUNK_SIZE).await? {
        contents.extend_from_slice(&chunk);
    }
    Ok(contents)
}
