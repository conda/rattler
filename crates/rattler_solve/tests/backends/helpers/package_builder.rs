use std::collections::BTreeMap;
use std::str::FromStr;

use rattler_conda_types::{NoArchType, PackageRecord, RepoDataRecord, Version};
use url::Url;

use super::super::{dummy_md5_hash, dummy_sha256_hash};

#[derive(Clone)]
pub struct PackageBuilder {
    record: RepoDataRecord,
}

impl PackageBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            record: RepoDataRecord {
                url: Url::from_str("http://example.com").unwrap(),
                channel: None,
                file_name: format!("{}-0.0.0-h123456_0.tar.bz2", name),
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
        }
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
        // Update filename to include the new version
        let name = self.record.package_record.name.as_normalized();
        let build = &self.record.package_record.build;
        self.record.file_name = format!("{}-{}-{}.tar.bz2", name, version, build);
        self
    }

    pub fn build_string(mut self, build: &str) -> Self {
        self.record.package_record.build = build.to_string();
        self
    }

    pub fn build_number(mut self, build_number: u64) -> Self {
        self.record.package_record.build_number = build_number;
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
            .insert(extra.to_string(), deps.into_iter().map(Into::into).collect());
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
