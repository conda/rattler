//! Stable fingerprints of resolved Conda environments.
//!
//! The format follows the virtual package detector protocol CEP: one UTF-8 line per
//! record containing the normalized name, verbatim version, verbatim build string
//! and artifact identity, separated by tabs. Lines are sorted by their bytes and
//! joined with line feeds, without a trailing line feed, then hashed with SHA-256.

use std::borrow::Cow;

use rattler_conda_types::{HasArtifactDigestRefs, HasArtifactIdentificationRefs};
use rattler_digest::{Md5Hash, Sha256, Sha256Hash, digest::Digest};

/// Computes the environment digest of `records`.
///
/// The artifact identity is its SHA-256 hash if present, otherwise its MD5 hash,
/// otherwise its canonical URL. Hashes use lowercase hexadecimal. Input ordering
/// does not affect the digest, and duplicate records each contribute a line.
///
/// Both repodata records and installed prefix records implement the required
/// traits. Other record types can provide the same identity through these traits.
pub fn environment_digest<T>(records: &[T]) -> Sha256Hash
where
    T: HasArtifactIdentificationRefs + HasArtifactDigestRefs,
{
    let mut lines: Vec<_> = records.iter().map(DigestLine::new).collect();
    lines.sort_unstable_by(|a, b| {
        a.parts()
            .into_iter()
            .flatten()
            .cmp(b.parts().into_iter().flatten())
    });
    let mut hasher = Sha256::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            hasher.update(b"\n");
        }
        for part in line.parts() {
            hasher.update(part);
        }
    }
    hasher.finalize()
}

struct DigestLine<'a> {
    name: &'a str,
    version: Cow<'a, str>,
    build: &'a str,
    artifact: Artifact<'a>,
}

impl<'a> DigestLine<'a> {
    fn new<T>(record: &'a T) -> Self
    where
        T: HasArtifactIdentificationRefs + HasArtifactDigestRefs,
    {
        let hash = record
            .sha256()
            .map(Sha256Hash::as_slice)
            .or_else(|| record.md5().map(Md5Hash::as_slice));
        let artifact = match hash {
            Some(hash) => {
                let mut bytes = [0; 64];
                let len = hash.len() * 2;
                hex::encode_to_slice(hash, &mut bytes[..len])
                    .expect("hash buffer has the exact hexadecimal length");
                Artifact::Hash { bytes, len }
            }
            None => Artifact::Url(record.url().as_str()),
        };
        Self {
            name: record.name().as_normalized(),
            version: record.version().as_str(),
            build: record.build(),
            artifact,
        }
    }

    fn parts(&self) -> [&[u8]; 7] {
        [
            self.name.as_bytes(),
            b"\t",
            self.version.as_bytes(),
            b"\t",
            self.build.as_bytes(),
            b"\t",
            self.artifact.as_bytes(),
        ]
    }
}

enum Artifact<'a> {
    Hash { bytes: [u8; 64], len: usize },
    Url(&'a str),
}

impl Artifact<'_> {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Hash { bytes, len } => &bytes[..*len],
            Self::Url(url) => url.as_bytes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_conda_types::{
        PackageName, PackageRecord, RepoDataRecord, Version, package::DistArchiveIdentifier,
    };
    use rattler_digest::{Md5, compute_bytes_digest, parse_digest_from_hex};
    use url::Url;

    use super::*;

    fn record(
        name: &str,
        version: &str,
        build: &str,
        sha256: Option<&str>,
        url: &str,
    ) -> RepoDataRecord {
        let mut package_record = PackageRecord::new(
            PackageName::try_from(name).unwrap(),
            Version::from_str(version).unwrap(),
            build.to_string(),
        );
        package_record.sha256 = sha256.map(|hex| parse_digest_from_hex::<Sha256>(hex).unwrap());
        RepoDataRecord {
            package_record,
            identifier: DistArchiveIdentifier::try_from_filename(url.rsplit('/').next().unwrap())
                .unwrap(),
            url: Url::parse(url).unwrap(),
            channel: None,
        }
    }

    #[test]
    fn matches_the_cep_example() {
        let records = [
            record(
                "python",
                "3.13.7",
                "h456_0",
                None,
                "https://conda.example/linux-64/python-3.13.7-h456_0.conda",
            ),
            record(
                "mpi-detect",
                "1.0.0",
                "h123_0",
                Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                "https://conda.example/linux-64/mpi-detect-1.0.0-h123_0.conda",
            ),
        ];
        assert_eq!(
            hex::encode(environment_digest(&records)),
            "6050e025841a00c866b37c4b843b1f6bea2be6209fff9deb40303bab8861962a"
        );
    }

    #[test]
    fn order_of_records_does_not_matter() {
        let a = record(
            "a",
            "1",
            "0",
            None,
            "https://conda.example/noarch/a-1-0.conda",
        );
        let b = record(
            "b",
            "1",
            "0",
            None,
            "https://conda.example/noarch/b-1-0.conda",
        );
        assert_eq!(
            environment_digest(&[a.clone(), b.clone()]),
            environment_digest(&[b, a])
        );
    }

    #[test]
    fn artifact_identity_prefers_sha256_then_md5_then_url() {
        let mut package = record(
            "a",
            "1",
            "0",
            None,
            "https://conda.example/noarch/a-1-0.conda",
        );
        assert_eq!(
            environment_digest(std::slice::from_ref(&package)),
            compute_bytes_digest::<Sha256>(b"a\t1\t0\thttps://conda.example/noarch/a-1-0.conda")
        );
        package.package_record.md5 =
            Some(parse_digest_from_hex::<Md5>("00112233445566778899aabbccddeeff").unwrap());
        assert_eq!(
            environment_digest(std::slice::from_ref(&package)),
            compute_bytes_digest::<Sha256>(b"a\t1\t0\t00112233445566778899aabbccddeeff")
        );
        package.package_record.sha256 = Some(
            parse_digest_from_hex::<Sha256>(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
        );
        assert_eq!(
            environment_digest(std::slice::from_ref(&package)),
            compute_bytes_digest::<Sha256>(
                b"a\t1\t0\taaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            )
        );
    }

    #[test]
    fn empty_environment_hashes_the_empty_string() {
        assert_eq!(
            hex::encode(environment_digest::<RepoDataRecord>(&[])),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn retains_source_version_build_and_utf8_line_ordering() {
        let mut a = record(
            "A",
            "1",
            "z\té",
            None,
            "https://conda.example/noarch/a-1-0.conda",
        );
        a.package_record.version = "1.00".parse().unwrap();
        let b = record(
            "a",
            "1",
            "z\tê",
            None,
            "https://conda.example/noarch/a-1-1.conda",
        );
        assert_eq!(
            environment_digest(&[a, b]),
            compute_bytes_digest::<Sha256>(
                "a\t1\tz\tê\thttps://conda.example/noarch/a-1-1.conda\na\t1.00\tz\té\thttps://conda.example/noarch/a-1-0.conda".as_bytes()
            )
        );
    }
}
