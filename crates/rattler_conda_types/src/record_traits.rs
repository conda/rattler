use rattler_digest::{Md5Hash, Sha256Hash};
use url::Url;

use crate::{
    MinimalPrefixRecord, PackageName, PackageRecord, PrefixRecord, RepoDataRecord,
    VersionWithSource,
};

/// A trait for types that allows identifying record uniquely within a subdirectory.
pub trait HasArtifactIdentificationRefs {
    /// Returns the name of the packages.
    fn name(&self) -> &PackageName;

    /// The version of the package
    fn version(&self) -> &VersionWithSource;

    /// Returns the build string of the package.
    fn build(&self) -> &str;
}

/// Provides the hashes and download URL of a package artifact.
pub trait HasArtifactDigestRefs {
    /// Returns the SHA-256 hash of the artifact, if available.
    fn sha256(&self) -> Option<&Sha256Hash>;

    /// Returns the MD5 hash of the artifact, if available.
    fn md5(&self) -> Option<&Md5Hash>;

    /// Returns the artifact's canonical download URL.
    fn url(&self) -> &Url;
}

impl HasArtifactIdentificationRefs for PackageRecord {
    fn name(&self) -> &PackageName {
        &self.name
    }

    fn version(&self) -> &VersionWithSource {
        &self.version
    }

    fn build(&self) -> &str {
        &self.build
    }
}

impl HasArtifactIdentificationRefs for RepoDataRecord {
    fn name(&self) -> &PackageName {
        &self.package_record.name
    }

    fn version(&self) -> &VersionWithSource {
        &self.package_record.version
    }

    fn build(&self) -> &str {
        &self.package_record.build
    }
}

impl HasArtifactIdentificationRefs for PrefixRecord {
    fn name(&self) -> &PackageName {
        &self.repodata_record.package_record.name
    }

    fn version(&self) -> &VersionWithSource {
        &self.repodata_record.package_record.version
    }

    fn build(&self) -> &str {
        &self.repodata_record.package_record.build
    }
}

impl HasArtifactIdentificationRefs for MinimalPrefixRecord {
    fn name(&self) -> &PackageName {
        &self.name
    }

    fn version(&self) -> &VersionWithSource {
        &self.version
    }

    fn build(&self) -> &str {
        &self.build
    }
}

impl HasArtifactDigestRefs for RepoDataRecord {
    fn sha256(&self) -> Option<&Sha256Hash> {
        self.package_record.sha256.as_ref()
    }

    fn md5(&self) -> Option<&Md5Hash> {
        self.package_record.md5.as_ref()
    }

    fn url(&self) -> &Url {
        &self.url
    }
}

impl HasArtifactDigestRefs for PrefixRecord {
    fn sha256(&self) -> Option<&Sha256Hash> {
        self.repodata_record.sha256()
    }

    fn md5(&self) -> Option<&Md5Hash> {
        self.repodata_record.md5()
    }

    fn url(&self) -> &Url {
        self.repodata_record.url()
    }
}
