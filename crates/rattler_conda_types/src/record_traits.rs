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
