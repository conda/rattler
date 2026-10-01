//! The environment digest: a fingerprint of every package record in a
//! resolved detector environment.
//!
//! Two clients that resolve the same records compute the same digest, so it
//! keys both the installed environment and the cached results. The format is
//! fixed by the protocol CEP: one line per record with the normalized name,
//! the verbatim version, the verbatim build string and the package artifact,
//! separated by tabs; lines sorted by their bytes and joined by line feeds; the
//! SHA-256 of the result.

use rattler_conda_types::RepoDataRecord;
use rattler_digest::{Sha256, Sha256Hash, digest::Digest};

/// Computes the environment digest of `records`.
///
/// The artifact of a record is its SHA-256 hash if present, otherwise its MD5
/// hash, otherwise its URL. Hashes are written as lowercase hexadecimal.
pub fn environment_digest(records: &[RepoDataRecord]) -> Sha256Hash {
    let mut lines: Vec<String> = records.iter().map(digest_line).collect();
    lines.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    let mut hasher = Sha256::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            hasher.update(b"\n");
        }
        hasher.update(line.as_bytes());
    }
    hasher.finalize()
}

fn digest_line(record: &RepoDataRecord) -> String {
    let package = &record.package_record;
    let artifact = match (&package.sha256, &package.md5) {
        (Some(sha256), _) => hex::encode(sha256),
        (None, Some(md5)) => hex::encode(md5),
        (None, None) => record.url.to_string(),
    };
    format!(
        "{}\t{}\t{}\t{}",
        package.name.as_normalized(),
        package.version.as_str(),
        package.build,
        artifact
    )
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_conda_types::{
        PackageName, PackageRecord, Version, package::DistArchiveIdentifier,
    };
    use rattler_digest::parse_digest_from_hex;
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

    /// The worked example of the protocol CEP.
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
    fn md5_is_used_before_the_url() {
        let mut with_md5 = record(
            "a",
            "1",
            "0",
            None,
            "https://conda.example/noarch/a-1-0.conda",
        );
        with_md5.package_record.md5 = Some(
            parse_digest_from_hex::<rattler_digest::Md5>("00112233445566778899aabbccddeeff")
                .unwrap(),
        );
        assert_eq!(
            digest_line(&with_md5),
            "a\t1\t0\t00112233445566778899aabbccddeeff"
        );
        let without = record(
            "a",
            "1",
            "0",
            None,
            "https://conda.example/noarch/a-1-0.conda",
        );
        assert_eq!(
            digest_line(&without),
            "a\t1\t0\thttps://conda.example/noarch/a-1-0.conda"
        );
    }

    #[test]
    fn empty_environment_hashes_the_empty_string() {
        assert_eq!(
            hex::encode(environment_digest(&[])),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
