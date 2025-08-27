//! Minimal prefix record reading for fast environment change detection.
//!
//! This module provides functionality to read only the minimal metadata needed
//! from conda-meta JSON files to determine if packages have changed, avoiding
//! the expensive parsing of file lists and other large data structures.

use std::str::FromStr;
use std::{io, path::Path};

use crate::{NoArchType, PackageName, PackageRecord, PrefixRecord, Version, VersionWithSource};
use hex;
use itertools::Itertools;
use rattler_digest::{Md5Hash, Sha256Hash};

/// A minimal version of `PrefixRecord` that only contains fields needed for transaction computation.
/// This is much faster to parse than the full `PrefixRecord`.
#[derive(Debug, Clone)]
#[allow(deprecated)]
pub struct MinimalPrefixRecord {
    /// The package name
    pub name: PackageName,
    /// The package version as a string
    pub version: String,
    /// The build string
    pub build: String,

    /// SHA256 hash of the package
    pub sha256: Option<Sha256Hash>,
    /// MD5 hash of the package, only if there is no SHA256 hash.
    pub md5: Option<Md5Hash>,
    /// Size of the package in bytes, only if there is no MD5 hash.
    pub size: Option<u64>,
    /// Optionally a path within the environment of the site-packages directory.
    /// This field is only present for python interpreter packages.
    /// This field was introduced with <https://github.com/conda/ceps/blob/main/cep-17.md>.
    pub python_site_packages_path: Option<String>,

    /// Deprecated: Old field for requested spec.
    /// Only used for migration to `requested_specs`.
    #[deprecated = "Use requested_specs instead"]
    pub requested_spec: Option<String>,

    /// The list of requested specs that were used to install this package.
    /// This is used to track which specs requested this package.
    pub requested_specs: Vec<String>,
}

impl MinimalPrefixRecord {
    // Could use `ajson` as it can parse multiple values at once,
    // which could make it faster, although on synthetic benchmarks it
    // loses to `gjson` and seems to work incorrectly when parsing multiple values.
    //
    // Ideal approach would be to create `SparsePrefixRecord` akin `SpareRepodata`.
    /// Parse a minimal prefix record from a JSON file sparsely.
    pub fn from_path(path: &Path) -> Result<Self, io::Error> {
        let filename_without_ext = path.file_stem().and_then(|stem| stem.to_str()).unwrap(); // It is highly unlikely that path doesn't have filename.
        let (build, version, name) = filename_without_ext.rsplitn(3, '-').next_tuple().unwrap();
        let content = fs_err::read_to_string(path)?;
        let json = content.as_str();

        let mut sha256 = None;
        let mut md5 = None;
        let mut size = None;
        let mut python_site_packages_path = None;
        let mut requested_specs = Vec::new();
        let mut requested_spec = None;

        let sha256_val = gjson::get(json, "sha256");
        if sha256_val.exists() && sha256_val.kind() == gjson::Kind::String {
            if let Ok(bytes) = hex::decode(sha256_val.str()) {
                if bytes.len() == 32 {
                    sha256 = Some(Sha256Hash::from(
                        <[u8; 32]>::try_from(bytes.as_slice()).unwrap(),
                    ));
                }
            }
        };
        if sha256.is_none() {
            let md5_val = gjson::get(json, "md5");
            if md5_val.exists() && md5_val.kind() == gjson::Kind::String {
                if let Ok(bytes) = hex::decode(md5_val.str()) {
                    if bytes.len() == 16 {
                        md5 = Some(Md5Hash::from(
                            <[u8; 16]>::try_from(bytes.as_slice()).unwrap(),
                        ));
                    }
                }
            }
        }

        if sha256.is_none() && md5.is_none() {
            let size_val = gjson::get(json, "size");
            if size_val.exists() && size_val.kind() == gjson::Kind::Number {
                size = Some(size_val.u64());
            }
        }

        if name.trim() == "python" {
            let python_site_packages_path_val = gjson::get(json, "python_site_packages_path");
            if python_site_packages_path_val.exists()
                && python_site_packages_path_val.kind() == gjson::Kind::String
            {
                python_site_packages_path = Some(python_site_packages_path_val.str().into());
            } else {
                return Err(io::Error::other(format!(
                    "Could not obtain python site packages path of prefix record at {}",
                    path.display()
                )));
            }
        }

        // Parse requested_specs array
        let requested_specs_val = gjson::get(json, "requested_specs");
        if requested_specs_val.exists() && requested_specs_val.kind() == gjson::Kind::Array {
            requested_specs_val.each(|_, val| {
                if val.kind() == gjson::Kind::String {
                    requested_specs.push(val.str().to_string());
                }
                true
            });
        }

        // Parse deprecated requested_spec field
        let requested_spec_val = gjson::get(json, "requested_spec");
        if requested_spec_val.exists() && requested_spec_val.kind() == gjson::Kind::String {
            requested_spec = Some(requested_spec_val.str().to_string());
        }

        #[allow(deprecated)]
        Ok(Self {
            name: name
                .parse::<PackageName>()
                .map_err(|e| format!("Could not parse package name: {e:#?}"))
                .map_err(io::Error::other)?,
            version: version.into(),
            build: build.into(),
            sha256,
            md5,
            size,
            python_site_packages_path,
            requested_specs,
            requested_spec,
        })
    }

    /// Convert to a partial `PackageRecord` for use in transaction computation.
    /// This creates a `PackageRecord` with only the essential fields filled in.
    pub fn to_package_record(&self) -> PackageRecord {
        let version = self
            .version
            .parse::<Version>()
            .unwrap_or_else(|_| Version::from_str(&self.version).unwrap());
        let version_with_source = VersionWithSource::from(version);

        PackageRecord {
            name: self.name.clone(),
            version: version_with_source,
            build: self.build.clone(),
            build_number: 0,
            subdir: "noarch".to_string(),
            md5: self.md5,
            sha256: self.sha256,
            size: self.size,
            noarch: NoArchType::none(),
            arch: None,
            platform: None,
            depends: Vec::new(),
            constrains: Vec::new(),
            features: None,
            legacy_bz2_size: None,
            license: None,
            license_family: None,
            purls: None,
            run_exports: None,
            timestamp: None,
            track_features: Vec::new(),
            python_site_packages_path: None,
            experimental_extra_depends: std::collections::BTreeMap::new(),
            legacy_bz2_md5: None,
        }
    }
}

/// Collect minimal prefix records from a prefix directory.
/// This is much faster than collecting full `PrefixRecord`s when you only need
/// to check if packages have changed.
pub fn collect_minimal_prefix_records(
    prefix: &Path,
) -> Result<Vec<MinimalPrefixRecord>, io::Error> {
    let conda_meta_path = prefix.join("conda-meta");

    if !conda_meta_path.exists() {
        return Ok(Vec::new());
    }

    // Collect paths first
    let json_paths: Vec<_> = fs_err::read_dir(&conda_meta_path)?
        .filter_map(|entry| {
            entry.ok().and_then(|e| {
                if e.file_type().ok()?.is_file()
                    && e.file_name().to_string_lossy().ends_with(".json")
                {
                    Some(e.path())
                } else {
                    None
                }
            })
        })
        .collect();

    // Parse minimal records in parallel if rayon is available
    #[cfg(feature = "rayon")]
    {
        use rayon::prelude::*;
        json_paths
            .par_iter()
            .map(|path| MinimalPrefixRecord::from_path(path))
            .collect()
    }

    #[cfg(not(feature = "rayon"))]
    {
        json_paths
            .iter()
            .map(|path| MinimalPrefixRecord::from_path(path))
            .collect()
    }
}

/// Extension trait for `PrefixRecord` to support sparse collection
pub trait MinimalPrefixCollection {
    /// Collect only the minimal fields needed for transaction computation.
    /// Fall back to full parsing if more fields needed!
    fn collect_minimal_from_prefix(prefix: &Path) -> Result<Vec<PrefixRecord>, io::Error>;
}

impl MinimalPrefixCollection for PrefixRecord {
    fn collect_minimal_from_prefix(prefix: &Path) -> Result<Vec<PrefixRecord>, io::Error> {
        let minimal_records = collect_minimal_prefix_records(prefix)?;

        // For now, we'll convert minimal records to full PrefixRecords with just the essential fields.
        // In the future, we could make Transaction work directly with SparsePrefixRecord.
        Ok(minimal_records
            .into_iter()
            .map(|minimal| {
                let package_record = minimal.to_package_record();
                let file_name = format!("{}-{}-{}.tar.bz2",
                    minimal.name.as_normalized(),
                    minimal.version,
                    minimal.build);
                #[allow(deprecated)]
                PrefixRecord {
                    repodata_record: crate::RepoDataRecord {
                        package_record,
                        file_name,
                        url: url::Url::parse("https://conda.anaconda.org/conda-forge/noarch/placeholder-1.0.0-0.tar.bz2").unwrap(),
                        channel: Some(String::new()),
                    },
                    package_tarball_full_path: None,
                    extracted_package_dir: None,
                    files: Vec::new(),
                    paths_data: crate::prefix_record::PrefixPaths::default(),
                    requested_spec: minimal.requested_spec.clone(),
                    requested_specs: minimal.requested_specs.clone(),
                    link: None,
                    installed_system_menus: Vec::new(),
                }
            })
            .collect())
    }
}
