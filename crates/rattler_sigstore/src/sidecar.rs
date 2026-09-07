//! Discovery and retrieval of attestation sidecars.
//!
//! The conda CEP on distribution of Sigstore attestations serves the
//! attestations of a package at `<package_url>.sigs` (mutable) and at
//! `<package_url>.sigs.<sha256>` (immutable, content-addressed). The hash is
//! advertised in the repodata through the `attestations_sha256` field of the
//! package record. Clients discover sidecars exclusively through that field
//! and always fetch the content-addressed URL.

use futures::StreamExt;
use rattler_conda_types::RepoDataRecord;
use rattler_digest::{Sha256, Sha256Hash};
use reqwest_middleware::ClientWithMiddleware;
use sigstore_types::Bundle;
use url::Url;

use crate::error::{SigstoreError, SigstoreResult};

/// The suffix that is appended to a package filename to obtain the attestation
/// sidecar filename.
pub const SIDECAR_SUFFIX: &str = ".sigs";

/// The default maximum size of a sidecar in bytes. Sigstore bundles are in the
/// order of 10 KiB each, so this leaves room for a generous number of
/// attestations per package while bounding what a malicious server can make a
/// client buffer.
pub const DEFAULT_MAX_SIDECAR_SIZE: u64 = 4 * 1024 * 1024;

/// The attestation sidecar of a package: a hash-verified JSON array of Sigstore
/// bundles.
#[derive(Debug, Clone)]
pub struct AttestationSidecar {
    /// The content-addressed URL the sidecar was retrieved from.
    pub url: Url,

    /// The SHA256 of the sidecar bytes. This is guaranteed to match the hash
    /// that was advertised in the repodata.
    pub sha256: Sha256Hash,

    /// The bundles contained in the sidecar, in the order they appear.
    pub bundles: Vec<Bundle>,
}

/// Returns the content-addressed sidecar URL for a package.
///
/// The URL is derived by appending `.sigs.<sha256>` to the last path segment
/// of the package URL. Any query string (e.g. an authentication token) is
/// preserved.
pub fn sidecar_url(package_url: &Url, attestations_sha256: &Sha256Hash) -> SigstoreResult<Url> {
    let mut url = package_url.clone();
    let filename = url
        .path_segments()
        .and_then(|mut segments| segments.next_back().map(str::to_owned))
        .filter(|filename| !filename.is_empty())
        .ok_or_else(|| SigstoreError::InvalidSidecarUrl(package_url.clone()))?;
    let sidecar_filename = format!(
        "{filename}{SIDECAR_SUFFIX}.{}",
        hex::encode(attestations_sha256)
    );
    url.path_segments_mut()
        .map_err(|()| SigstoreError::InvalidSidecarUrl(package_url.clone()))?
        .pop()
        .push(&sidecar_filename);
    Ok(url)
}

/// Returns the content-addressed sidecar URL for a record, or `None` if the
/// record does not advertise attestations.
pub fn sidecar_url_for_record(record: &RepoDataRecord) -> SigstoreResult<Option<Url>> {
    record
        .package_record
        .attestations_sha256
        .as_ref()
        .map(|hash| sidecar_url(&record.url, hash))
        .transpose()
}

/// Parses sidecar bytes after checking them against the advertised hash.
///
/// The bytes must hash to `expected_sha256` and decode as a non-empty JSON
/// array of Sigstore bundles.
pub fn parse_sidecar(
    url: &Url,
    bytes: &[u8],
    expected_sha256: &Sha256Hash,
) -> SigstoreResult<AttestationSidecar> {
    let actual = rattler_digest::compute_bytes_digest::<Sha256>(bytes);
    if &actual != expected_sha256 {
        return Err(SigstoreError::SidecarHashMismatch {
            url: url.clone(),
            expected: hex::encode(expected_sha256).into_boxed_str(),
            actual: hex::encode(actual).into_boxed_str(),
        });
    }

    let raw_bundles: Vec<serde_json::Value> =
        serde_json::from_slice(bytes).map_err(|err| SigstoreError::MalformedSidecar {
            url: url.clone(),
            message: format!("expected a JSON array of Sigstore bundles: {err}"),
        })?;
    if raw_bundles.is_empty() {
        return Err(SigstoreError::MalformedSidecar {
            url: url.clone(),
            message: "the array of Sigstore bundles is empty".to_string(),
        });
    }

    let bundles = raw_bundles
        .into_iter()
        .enumerate()
        .map(|(index, raw)| {
            serde_json::from_value::<Bundle>(raw).map_err(|err| SigstoreError::MalformedSidecar {
                url: url.clone(),
                message: format!("bundle {index} is not a valid Sigstore bundle: {err}"),
            })
        })
        .collect::<SigstoreResult<Vec<_>>>()?;

    Ok(AttestationSidecar {
        url: url.clone(),
        sha256: actual,
        bundles,
    })
}

/// Fetches and validates the attestation sidecar advertised by a record.
///
/// Returns `Ok(None)` if the record does not advertise attestations. The body
/// is streamed and rejected as soon as it grows beyond `max_size` bytes. The
/// received bytes are checked against the advertised hash before parsing.
///
/// Per the CEP, an unavailable, oversized, malformed or hash-mismatched
/// sidecar is a retrieval failure and is reported as an error.
pub async fn fetch_sidecar(
    client: &ClientWithMiddleware,
    record: &RepoDataRecord,
    max_size: u64,
) -> SigstoreResult<Option<AttestationSidecar>> {
    let Some(expected_sha256) = record.package_record.attestations_sha256.as_ref() else {
        return Ok(None);
    };
    let url = sidecar_url(&record.url, expected_sha256)?;
    let bytes = fetch_bounded(client, &url, max_size).await?;
    parse_sidecar(&url, &bytes, expected_sha256).map(Some)
}

/// Downloads `url` into memory, failing once more than `max_size` bytes have
/// been received.
async fn fetch_bounded(
    client: &ClientWithMiddleware,
    url: &Url,
    max_size: u64,
) -> SigstoreResult<Vec<u8>> {
    let response =
        client
            .get(url.clone())
            .send()
            .await
            .map_err(|source| SigstoreError::FetchSidecar {
                url: url.clone(),
                source,
            })?;

    let status = response.status();
    if !status.is_success() {
        return Err(SigstoreError::SidecarHttpStatus {
            url: url.clone(),
            status,
        });
    }

    if response.content_length().is_some_and(|len| len > max_size) {
        return Err(SigstoreError::SidecarTooLarge {
            url: url.clone(),
            max_size,
        });
    }

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|source| SigstoreError::ReadSidecar {
            url: url.clone(),
            source,
        })?;
        if (bytes.len() + chunk.len()) as u64 > max_size {
            return Err(SigstoreError::SidecarTooLarge {
                url: url.clone(),
                max_size,
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash_of(bytes: &[u8]) -> Sha256Hash {
        rattler_digest::compute_bytes_digest::<Sha256>(bytes)
    }

    #[test]
    fn sidecar_url_appends_suffix_and_hash() {
        let hash = hash_of(b"sidecar");
        let package_url =
            Url::parse("https://prefix.dev/channel/linux-64/foo-1.0-h1_0.conda").unwrap();
        let url = sidecar_url(&package_url, &hash).unwrap();
        assert_eq!(
            url.as_str(),
            format!(
                "https://prefix.dev/channel/linux-64/foo-1.0-h1_0.conda.sigs.{}",
                hex::encode(hash)
            )
        );
    }

    #[test]
    fn sidecar_url_preserves_query() {
        let hash = hash_of(b"sidecar");
        let package_url = Url::parse(
            "https://example.com/t/secret/channel/noarch/foo-1.0-h1_0.tar.bz2?token=abc",
        )
        .unwrap();
        let url = sidecar_url(&package_url, &hash).unwrap();
        assert!(
            url.path()
                .ends_with(&format!(".tar.bz2.sigs.{}", hex::encode(hash)))
        );
        assert_eq!(url.query(), Some("token=abc"));
    }

    #[test]
    fn sidecar_url_rejects_urls_without_filename() {
        let hash = hash_of(b"sidecar");
        let package_url = Url::parse("https://prefix.dev/").unwrap();
        assert!(matches!(
            sidecar_url(&package_url, &hash),
            Err(SigstoreError::InvalidSidecarUrl(_))
        ));
    }

    #[test]
    fn parse_sidecar_rejects_hash_mismatch() {
        let url = Url::parse("https://example.com/foo.conda.sigs.abc").unwrap();
        let err = parse_sidecar(&url, b"[]", &hash_of(b"other")).unwrap_err();
        assert!(
            matches!(err, SigstoreError::SidecarHashMismatch { .. }),
            "{err}"
        );
    }

    #[test]
    fn parse_sidecar_rejects_malformed_content() {
        let url = Url::parse("https://example.com/foo.conda.sigs.abc").unwrap();
        for bytes in [&b"not json"[..], b"{}", b"[]", b"[{\"mediaType\": 1}]"] {
            let err = parse_sidecar(&url, bytes, &hash_of(bytes)).unwrap_err();
            assert!(
                matches!(err, SigstoreError::MalformedSidecar { .. }),
                "{bytes:?}: {err}"
            );
        }
    }

    #[test]
    fn parse_sidecar_accepts_real_bundle() {
        let bytes = include_bytes!("../test-data/actionlint-1.7.12-h60d57d3_0.conda.sigs");
        let url = Url::parse("https://example.com/foo.conda.sigs.abc").unwrap();
        let sidecar = parse_sidecar(&url, bytes, &hash_of(bytes)).unwrap();
        assert_eq!(sidecar.bundles.len(), 1);
    }
}
