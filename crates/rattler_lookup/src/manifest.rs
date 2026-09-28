//! `<subdir>/lookup/manifest.json`: the entry point of a subdir's index.
//!
//! The manifest is the only file of an index that changes. It lists the kinds of
//! lookups the index answers, its layers — each a packages file and one lookup
//! table per kind, all content-addressed — and the artifacts that were removed
//! from the channel after their layer was written.

use std::collections::{BTreeMap, BTreeSet};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::format::Kind;

/// The directory of a subdir's index, relative to the subdir.
pub const LOOKUP_DIR: &str = "lookup";
/// The file name of the manifest.
pub const MANIFEST_FILE: &str = "manifest.json";
/// The only manifest version this crate reads and writes.
pub const MANIFEST_VERSION: u32 = 1;

/// The manifest of the lookup index of one subdir.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// The version of the manifest format, always [`MANIFEST_VERSION`].
    pub version: u32,
    /// The base URL of the indexed channel.
    pub channel: String,
    /// The indexed subdir, e.g. `noarch`.
    pub subdir: String,
    /// When the manifest was created.
    pub created_at: Timestamp,
    /// The kinds of lookup tables every layer of this index has, e.g. `paths`.
    ///
    /// Non-empty and without duplicates. A kind that is not listed here is
    /// unavailable — which is not the same as answering no results.
    pub kinds: Vec<String>,
    /// The layers of the index, oldest first. May be empty.
    pub layers: Vec<Layer>,
    /// Filenames of artifacts that are contained in a layer but are no longer
    /// part of the channel.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub removed: BTreeSet<String>,
}

/// One layer of an index: the artifacts it covers and their paths.
///
/// An artifact belongs to at most one layer of an index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Layer {
    /// When the layer was created.
    pub created_at: Timestamp,
    /// The packages file of the layer, which the ids in its tables refer to.
    pub packages: PackagesRef,
    /// The lookup tables of the layer, by kind. Has an entry for every kind of
    /// the manifest; entries of other kinds are ignored.
    pub tables: BTreeMap<String, FileRef>,
}

/// A reference to a layer file, relative to the manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileRef {
    /// The name of the file, e.g. `paths-<sha256>.parquet`.
    pub file: String,
    /// The size of the file in bytes.
    pub size: u64,
}

/// A reference to the packages file of a layer, relative to the manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackagesRef {
    /// The name of the file, e.g. `packages-<sha256>.parquet`.
    pub file: String,
    /// The size of the file in bytes.
    pub size: u64,
    /// The number of artifacts the layer indexes, the number of rows of the
    /// packages file.
    pub count: u64,
}

/// An error that occurred while parsing a manifest.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// The manifest is not valid JSON or does not have the expected shape.
    #[error("the manifest is not valid JSON")]
    Json(#[from] serde_json::Error),

    /// The manifest was written in a version this crate does not understand.
    #[error("unsupported manifest version {0}")]
    UnsupportedVersion(u32),

    /// The manifest lists no kinds, so it cannot answer anything.
    #[error("the manifest lists no kinds")]
    NoKinds,

    /// The manifest lists a kind twice.
    #[error("the manifest lists the kind `{0}` more than once")]
    DuplicateKind(String),

    /// A layer is missing a table of a kind the manifest lists.
    #[error("a layer of the manifest has no `{0}` table")]
    MissingTable(String),

    /// A layer refers to a file name that is not a plain, relative file name.
    #[error("invalid layer file name `{0}` in the manifest")]
    InvalidFileName(String),
}

impl Manifest {
    /// An index of `subdir` with the given kinds and without any layers.
    pub fn empty(
        channel: impl Into<String>,
        subdir: impl Into<String>,
        kinds: impl IntoIterator<Item = Kind>,
    ) -> Self {
        Self {
            version: MANIFEST_VERSION,
            channel: channel.into(),
            subdir: subdir.into(),
            created_at: Timestamp::now(),
            kinds: kinds
                .into_iter()
                .map(|kind| kind.name().to_string())
                .collect(),
            layers: Vec::new(),
            removed: BTreeSet::new(),
        }
    }

    /// Parses and validates a manifest.
    ///
    /// Unknown fields are ignored, an unknown `version` is rejected.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ManifestError> {
        let manifest: Manifest = serde_json::from_slice(bytes)?;
        if manifest.version != MANIFEST_VERSION {
            return Err(ManifestError::UnsupportedVersion(manifest.version));
        }
        if manifest.kinds.is_empty() {
            return Err(ManifestError::NoKinds);
        }
        let mut seen = BTreeSet::new();
        for kind in &manifest.kinds {
            if !seen.insert(kind) {
                return Err(ManifestError::DuplicateKind(kind.clone()));
            }
        }
        for layer in &manifest.layers {
            for kind in &manifest.kinds {
                if !layer.tables.contains_key(kind) {
                    return Err(ManifestError::MissingTable(kind.clone()));
                }
            }
            for file in layer.file_names() {
                if file.contains('/')
                    || file.contains('\\')
                    || !file.ends_with(".parquet")
                    || file.starts_with('.')
                {
                    return Err(ManifestError::InvalidFileName(file.to_string()));
                }
            }
        }
        Ok(manifest)
    }

    /// Serializes the manifest.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec_pretty(self)
    }

    /// Whether this index has a table of `kind` and can answer its lookups.
    pub fn has_kind(&self, kind: Kind) -> bool {
        self.kinds.iter().any(|name| name == kind.name())
    }

    /// The kinds of this index that this crate knows how to read.
    pub fn known_kinds(&self) -> impl Iterator<Item = Kind> + '_ {
        self.kinds.iter().filter_map(|name| Kind::from_name(name))
    }

    /// The names of all files the layers of this manifest refer to.
    pub fn layer_file_names(&self) -> impl Iterator<Item = &str> {
        self.layers.iter().flat_map(Layer::file_names)
    }
}

impl Layer {
    /// The table of `kind` of this layer, if it has one.
    pub fn table(&self, kind: Kind) -> Option<&FileRef> {
        self.tables.get(kind.name())
    }

    /// The names of all files of this layer.
    pub fn file_names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.packages.file.as_str())
            .chain(self.tables.values().map(|table| table.file.as_str()))
    }
}

/// The URL of the manifest of `subdir` below an index base URL.
pub fn manifest_url(base: &Url, subdir: &str) -> Result<Url, url::ParseError> {
    directory_url(base)?.join(&format!("{subdir}/{LOOKUP_DIR}/{MANIFEST_FILE}"))
}

/// Resolves the name of a layer file relative to the manifest that lists it.
pub fn layer_url(manifest_url: &Url, file_name: &str) -> Result<Url, url::ParseError> {
    manifest_url.join(file_name)
}

/// The same URL, guaranteed to end in `/` so that joining appends.
pub fn directory_url(url: &Url) -> Result<Url, url::ParseError> {
    if url.path().ends_with('/') {
        Ok(url.clone())
    } else {
        Url::parse(&format!("{url}/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            version: MANIFEST_VERSION,
            channel: "https://conda.anaconda.org/conda-forge/".into(),
            subdir: "noarch".into(),
            created_at: "2026-09-24T03:00:00Z".parse().unwrap(),
            kinds: vec!["paths".into(), "reversed-paths".into()],
            layers: vec![Layer {
                created_at: "2026-09-23T02:00:00Z".parse().unwrap(),
                packages: PackagesRef {
                    file: "packages-cd.parquet".into(),
                    size: 2,
                    count: 3,
                },
                tables: BTreeMap::from([
                    (
                        "paths".to_string(),
                        FileRef {
                            file: "paths-ab.parquet".into(),
                            size: 1,
                        },
                    ),
                    (
                        "reversed-paths".to_string(),
                        FileRef {
                            file: "reversed-paths-ef.parquet".into(),
                            size: 4,
                        },
                    ),
                ]),
            }],
            removed: BTreeSet::new(),
        }
    }

    #[test]
    fn roundtrip() {
        let manifest = manifest();
        let json = manifest.to_bytes().unwrap();
        assert_eq!(Manifest::from_bytes(&json).unwrap(), manifest);
        // `removed` is omitted while it is empty.
        assert!(!String::from_utf8(json).unwrap().contains("removed"));
        for kind in Kind::ALL {
            assert!(manifest.has_kind(kind));
            assert!(manifest.layers[0].table(kind).is_some());
        }
        assert_eq!(manifest.known_kinds().collect::<Vec<_>>(), Kind::ALL);
    }

    #[test]
    fn ignores_unknown_fields() {
        let json = br#"{"version":1,"channel":"c","subdir":"s","kinds":["paths","x-other"],
            "created_at":"2026-09-24T03:00:00Z","layers":[],"future":42}"#;
        let manifest = Manifest::from_bytes(json).unwrap();
        assert!(manifest.layers.is_empty());
        // A kind this crate does not know is not a reason to reject the index.
        assert_eq!(
            manifest.known_kinds().collect::<Vec<_>>(),
            vec![Kind::Paths]
        );
        assert!(!manifest.has_kind(Kind::ReversedPaths));
    }

    #[test]
    fn rejects_unknown_version() {
        let json = br#"{"version":2,"channel":"c","subdir":"s","kinds":["paths"],
            "created_at":"2026-09-24T03:00:00Z","layers":[]}"#;
        assert!(matches!(
            Manifest::from_bytes(json),
            Err(ManifestError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn rejects_broken_kinds() {
        let json = br#"{"version":1,"channel":"c","subdir":"s","kinds":[],
            "created_at":"2026-09-24T03:00:00Z","layers":[]}"#;
        assert!(matches!(
            Manifest::from_bytes(json),
            Err(ManifestError::NoKinds)
        ));

        let json = br#"{"version":1,"channel":"c","subdir":"s","kinds":["paths","paths"],
            "created_at":"2026-09-24T03:00:00Z","layers":[]}"#;
        assert!(matches!(
            Manifest::from_bytes(json),
            Err(ManifestError::DuplicateKind(_))
        ));
    }

    #[test]
    fn rejects_a_layer_without_a_listed_kind() {
        let mut manifest = manifest();
        manifest.layers[0].tables.remove("reversed-paths");
        let json = manifest.to_bytes().unwrap();
        assert!(matches!(
            Manifest::from_bytes(&json),
            Err(ManifestError::MissingTable(kind)) if kind == "reversed-paths"
        ));
    }

    #[test]
    fn rejects_paths_in_file_names() {
        let in_a_table = |file: &str| {
            let mut manifest = manifest();
            manifest.layers[0].tables.get_mut("paths").unwrap().file = file.to_string();
            manifest
        };
        let in_the_packages_file = |file: &str| {
            let mut manifest = manifest();
            manifest.layers[0].packages.file = file.to_string();
            manifest
        };

        for file in ["../x.parquet", "a/x.parquet", "x.json", ".parquet"] {
            for manifest in [in_a_table(file), in_the_packages_file(file)] {
                let json = manifest.to_bytes().unwrap();
                assert!(
                    matches!(
                        Manifest::from_bytes(&json),
                        Err(ManifestError::InvalidFileName(_))
                    ),
                    "{file} should be rejected"
                );
            }
        }
    }

    #[test]
    fn resolves_urls() {
        let base = Url::parse("https://x.org/channel").unwrap();
        let manifest = manifest_url(&base, "noarch").unwrap();
        assert_eq!(
            manifest.as_str(),
            "https://x.org/channel/noarch/lookup/manifest.json"
        );
        assert_eq!(
            layer_url(&manifest, "a.parquet").unwrap().as_str(),
            "https://x.org/channel/noarch/lookup/a.parquet"
        );
    }
}
