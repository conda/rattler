use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use rattler_conda_types::{
    package::{ArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier, DistArchiveType},
    NoArchType, PackageRecord, RepoDataRecord, Version,
};
use url::Url;

use super::super::{dummy_md5_hash, dummy_sha256_hash};

#[derive(Clone)]
pub struct PackageBuilder {
    record: RepoDataRecord,
    archive_type: CondaArchiveType,
}

impl PackageBuilder {
    pub fn new(name: &str) -> Self {
        let archive_type = CondaArchiveType::Conda;
        Self {
            record: RepoDataRecord {
                url: Url::from_str("http://example.com").unwrap(),
                channel: None,
                identifier: DistArchiveIdentifier {
                    identifier: ArchiveIdentifier {
                        name: name.to_string(),
                        version: "0.0.0".to_string(),
                        build_string: "h123456_0".to_string(),
                    },
                    archive_type: DistArchiveType::Conda(archive_type),
                },
                package_record: PackageRecord {
                    name: name.parse().unwrap(),
                    version: Version::from_str("0.0.0").unwrap().into(),
                    build: "h123456_0".to_string(),
                    build_number: 0,
                    subdir: "linux-64".to_string(),
                    md5: Some(dummy_md5_hash()),
                    sha256: Some(dummy_sha256_hash()),
                    size: None,
                    arch: None,
                    experimental_extra_depends: BTreeMap::new(),
                    platform: None,
                    depends: Vec::new(),
                    constrains: Vec::new(),
                    track_features: Vec::new(),
                    features: None,
                    filename: None,
                    noarch: NoArchType::default(),
                    license: None,
                    license_family: None,
                    timestamp: None,
                    legacy_bz2_size: None,
                    legacy_bz2_md5: None,
                    purls: None,
                    python_site_packages_path: None,
                    run_exports: None,
                },
            },
            archive_type,
        }
    }

    /// Updates the filename based on current package metadata and archive type.
    fn update_filename(&mut self) {
        self.record.identifier = DistArchiveIdentifier {
            identifier: ArchiveIdentifier {
                name: self.record.package_record.name.as_normalized().to_string(),
                version: self.record.package_record.version.as_str().to_string(),
                build_string: self.record.package_record.build.clone(),
            },
            archive_type: DistArchiveType::Conda(self.archive_type),
        };
    }

    pub fn depends(mut self, deps: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.record.package_record.depends = deps.into_iter().map(Into::into).collect();
        self
    }

    pub fn channel(mut self, channel: &str) -> Self {
        self.record.channel = Some(channel.to_string());
        self
    }

    pub fn subdir(mut self, subdir: &str) -> Self {
        self.record.package_record.subdir = subdir.to_string();
        self
    }

    pub fn version(mut self, version: &str) -> Self {
        self.record.package_record.version = Version::from_str(version).unwrap().into();
        self.update_filename();
        self
    }

    pub fn build_string(mut self, build: &str) -> Self {
        self.record.package_record.build = build.to_string();
        self.update_filename();
        self
    }

    pub fn build_number(mut self, build_number: u64) -> Self {
        self.record.package_record.build_number = build_number;
        self
    }

    /// Sets the archive type (defaults to `.conda`).
    pub fn archive_type(mut self, archive_type: CondaArchiveType) -> Self {
        self.archive_type = archive_type;
        self.update_filename();
        self
    }

    pub fn extra_depends(
        mut self,
        extra: &str,
        deps: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.record
            .package_record
            .experimental_extra_depends
            .insert(
                extra.to_string(),
                deps.into_iter().map(Into::into).collect(),
            );
        self
    }

    pub fn timestamp(mut self, timestamp: &str) -> Self {
        let dt: DateTime<Utc> = timestamp.parse().expect("invalid timestamp format");
        self.record.package_record.timestamp = Some(dt.into());
        self
    }

    pub fn build(self) -> RepoDataRecord {
        self.record
    }
}

impl From<PackageBuilder> for RepoDataRecord {
    fn from(builder: PackageBuilder) -> Self {
        builder.build()
    }
}
