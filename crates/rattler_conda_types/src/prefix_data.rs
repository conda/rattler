//! Lazily populated view of the 'conda-meta' directory
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

use crate::package::ArchiveIdentifier;
use crate::package_name::PackageName;
use crate::PrefixRecord;

/// An error that can occur when loading a prefix record
#[derive(Debug, Clone, thiserror::Error)]
#[error("io error: {0}")]
pub struct PrefixDataError(pub Arc<std::io::Error>);

impl From<std::io::Error> for PrefixDataError {
    fn from(err: std::io::Error) -> Self {
        PrefixDataError(Arc::new(err))
    }
}

/// Internal state for a lazily loaded package record
struct LazyRecordEntry {
    path: PathBuf,
    record: OnceLock<Result<PrefixRecord, PrefixDataError>>,
}

/// A lazily populated view of the `conda-meta` directory in a prefix.
pub struct PrefixData {
    /// The path to the environment prefix
    prefix_path: PathBuf,
    /// A map of package names to their file path and a lazy lock for the parsed record
    records: HashMap<PackageName, LazyRecordEntry>,
}

impl PrefixData {
    /// Returns the path to the environment prefix
    pub fn prefix_path(&self) -> &Path {
        &self.prefix_path
    }

    /// Discovers all packages in the `conda-meta` directory but does not parse the JSON yet.
    pub fn new(prefix_path: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let prefix_path = prefix_path.into();
        let meta_dir = prefix_path.join("conda-meta");
        let mut records = HashMap::new();

        if !meta_dir.exists() {
            return Ok(Self {
                prefix_path,
                records,
            });
        }

        for entry in fs::read_dir(meta_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            if let Some(filename) = path.file_name().and_then(|s| s.to_str()) {
                if let Some(base_name) = filename.strip_suffix(".json") {
                    if let Ok(archive_id) = base_name.parse::<ArchiveIdentifier>() {
                        if let Ok(package_name) = PackageName::try_from(archive_id.name) {
                            records.insert(
                                package_name,
                                LazyRecordEntry {
                                    path,
                                    record: OnceLock::new(),
                                },
                            );
                        }
                    }
                }
            }
        }
        Ok(Self {
            prefix_path,
            records,
        })
    }

    /// Retrieves a record lazily. Parses the JSON only on the first call.
    pub fn get(
        &self,
        package_name: &PackageName,
    ) -> Option<Result<&PrefixRecord, &PrefixDataError>> {
        // 1. Check if the file path exists in our initial scan
        let entry = self.records.get(package_name)?;

        // 2. Parse the file if we haven't already.
        let record_result = entry
            .record
            .get_or_init(|| PrefixRecord::from_path(&entry.path).map_err(PrefixDataError::from));

        // 3. .as_ref() elegantly converts &Result<T, E> into Result<&T, &E>
        Some(record_result.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_prefix_data_lazy_loading() {
        let dir = tempdir().unwrap();
        let meta_dir = dir.path().join("conda-meta");
        fs::create_dir_all(&meta_dir).unwrap();

        let fake_json_path = meta_dir.join("numpy-1.24.3-py311h_0.json");
        fs::write(&fake_json_path, "{}").unwrap();

        let prefix_data = PrefixData::new(dir.path()).unwrap();
        let numpy_name = PackageName::try_from("numpy").unwrap();
        assert!(
            prefix_data.records.contains_key(&numpy_name),
            "Did not extract package name correctly!"
        );
    }

    #[test]
    fn test_prefix_data_strict() {
        let dir = tempdir().unwrap();
        let meta_dir = dir.path().join("conda-meta");
        fs::create_dir_all(&meta_dir).unwrap();

        fs::write(meta_dir.join("numpy-1.24.3-py311h_0.json"), "{}").unwrap();
        fs::write(meta_dir.join("scikit-learn-1.2.2-py311_1.json"), "{}").unwrap();

        fs::write(meta_dir.join("ignore_me.txt"), "some text").unwrap();
        fs::write(meta_dir.join("not_a_json.yaml"), "{}").unwrap();

        let prefix_data = PrefixData::new(dir.path()).unwrap();

        let numpy_name = PackageName::try_from("numpy").unwrap();
        let scikit_name = PackageName::try_from("scikit-learn").unwrap();
        let does_not_exist_name = PackageName::try_from("does-not-exist").unwrap();

        assert_eq!(
            prefix_data.records.len(),
            2,
            "Should only load the 2 valid .json files"
        );
        assert!(
            prefix_data.records.contains_key(&numpy_name),
            "Failed to extract simple package name"
        );
        assert!(
            prefix_data.records.contains_key(&scikit_name),
            "Failed to extract package name with hyphens"
        );

        let numpy_record = prefix_data.get(&numpy_name);
        assert!(
            numpy_record.unwrap().is_err(),
            "Expected Some(Err) because the JSON is malformed/empty"
        );

        let entry = prefix_data.records.get(&numpy_name).unwrap();
        assert!(
            entry.record.get().is_some(),
            "The OnceLock should be populated (with an Err value) after the first get() call"
        );

        assert!(
            prefix_data.get(&does_not_exist_name).is_none(),
            "Expected None because the package is not in the directory at all"
        );
    }
}
