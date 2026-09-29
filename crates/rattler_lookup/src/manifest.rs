//! `manifest.json`: the entry point of a subdir index.
//!
//! The manifest is the only file of an index that changes. It lists the kinds
//! of the index, its layers (each a content-addressed packages file plus one
//! lookup table per kind) and the artifacts that were removed from the
//! channel after their layer was written.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Kind, Location, LookupError};

/// The directory of a subdir index, relative to the subdir.
pub const LOOKUP_DIR: &str = "lookup";
/// The file name of the manifest.
pub const MANIFEST_FILE: &str = "manifest.json";
/// The manifest version this crate writes and reads.
pub const MANIFEST_VERSION: u64 = 1;
/// The `lookup_url` a channel uses when it publishes the index next to its
/// repodata.
pub const DEFAULT_LOOKUP_URL: &str = "./lookup/manifest.json";

/// The manifest of a subdir index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// The version of the manifest format, always [`MANIFEST_VERSION`].
    pub version: u64,
    /// The base URL of the indexed channel.
    pub channel: String,
    /// The subdir.
    pub subdir: String,
    /// When the manifest was created (RFC 3339).
    pub created_at: String,
    /// The kinds of the index: every layer has a table of each of them.
    pub kinds: Vec<String>,
    /// The layers of the index.
    pub layers: Vec<Layer>,
    /// Filenames of artifacts that are contained in a layer but are no longer
    /// part of the channel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

/// One layer: a packages file and a lookup table per kind.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Layer {
    /// When the layer was created (RFC 3339).
    pub created_at: String,
    /// The packages file.
    pub packages: PackagesRef,
    /// The lookup tables, by kind.
    pub tables: BTreeMap<String, FileRef>,
}

/// A lookup table of a layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileRef {
    /// The file name, relative to the manifest.
    pub file: String,
    /// The size of the file in bytes.
    pub size: u64,
}

/// The packages file of a layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackagesRef {
    /// The file name, relative to the manifest.
    pub file: String,
    /// The size of the file in bytes.
    pub size: u64,
    /// The number of rows (artifacts).
    pub count: u64,
}

/// Only the version, to reject unknown versions before parsing the rest.
#[derive(Deserialize)]
struct Probe {
    version: u64,
}

impl Manifest {
    /// A manifest without layers.
    pub fn empty(channel: impl Into<String>, subdir: impl Into<String>, kinds: &[Kind]) -> Self {
        Self {
            version: MANIFEST_VERSION,
            channel: channel.into(),
            subdir: subdir.into(),
            created_at: now_rfc3339(),
            kinds: kinds.iter().map(|kind| kind.name().to_string()).collect(),
            layers: Vec::new(),
            removed: Vec::new(),
        }
    }

    /// Parses and validates a manifest. `location` is only used in error
    /// messages.
    pub fn parse(bytes: &[u8], location: &Location) -> Result<Self, LookupError> {
        let invalid = |reason: String| LookupError::InvalidManifest {
            location: location.clone(),
            reason,
        };
        let probe: Probe = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
        if probe.version != MANIFEST_VERSION {
            return Err(LookupError::UnsupportedManifestVersion {
                location: location.clone(),
                version: probe.version,
            });
        }
        let manifest: Manifest =
            serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
        manifest.validate().map_err(invalid)?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), String> {
        if self.kinds.is_empty() {
            return Err("the manifest lists no kinds".into());
        }
        for (i, kind) in self.kinds.iter().enumerate() {
            if self.kinds[..i].contains(kind) {
                return Err(format!("the kind `{kind}` is listed twice"));
            }
        }
        for layer in &self.layers {
            for kind in &self.kinds {
                if !layer.tables.contains_key(kind) {
                    return Err(format!("a layer has no `{kind}` table"));
                }
            }
            let files = layer.tables.values().map(|t| t.file.as_str());
            for file in files.chain([layer.packages.file.as_str()]) {
                if file.is_empty() || file.contains('/') || file.contains('\\') {
                    return Err(format!("invalid file name `{file}`"));
                }
            }
        }
        Ok(())
    }

    /// Serializes the manifest as pretty-printed JSON.
    pub fn to_json(&self) -> Result<Vec<u8>, LookupError> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// Whether every layer has a table of this kind.
    pub fn has_kind(&self, kind: Kind) -> bool {
        self.kinds.iter().any(|k| k == kind.name())
    }

    /// The kinds of the index that this crate knows.
    pub fn known_kinds(&self) -> Vec<Kind> {
        self.kinds
            .iter()
            .filter_map(|kind| Kind::from_name(kind))
            .collect()
    }

    /// The number of artifacts listed in all layers.
    pub fn package_count(&self) -> u64 {
        self.layers.iter().map(|layer| layer.packages.count).sum()
    }

    /// The names of all files of all layers.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.layers.iter().flat_map(|layer| {
            std::iter::once(layer.packages.file.as_str())
                .chain(layer.tables.values().map(|table| table.file.as_str()))
        })
    }
}

impl Layer {
    /// The table of a kind, if the layer has one.
    pub fn table(&self, kind: Kind) -> Option<&FileRef> {
        self.tables.get(kind.name())
    }
}

/// The current time as an RFC 3339 timestamp with second precision, as used
/// for `created_at`.
pub fn now_rfc3339() -> String {
    jiff::Timestamp::now()
        .round(jiff::Unit::Second)
        .unwrap_or_else(|_| jiff::Timestamp::now())
        .to_string()
}

/// The location of the manifest of `subdir` below an index base (a channel
/// URL or directory): `<base>/<subdir>/lookup/manifest.json`.
pub fn manifest_location(base: &Location, subdir: &str) -> Location {
    base.join(&format!("{subdir}/{LOOKUP_DIR}/{MANIFEST_FILE}"))
}

/// The relative path of the manifest of `subdir` within a channel.
pub fn manifest_path(subdir: &str) -> String {
    format!("{subdir}/{LOOKUP_DIR}/{MANIFEST_FILE}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> FileRef {
        FileRef {
            file: name.into(),
            size: 1,
        }
    }

    fn location() -> Location {
        Location::parse("idx/noarch/lookup/manifest.json")
    }

    #[test]
    fn roundtrip() {
        let mut manifest = Manifest::empty(
            "https://conda.anaconda.org/conda-forge/",
            "noarch",
            &Kind::ALL,
        );
        manifest.layers.push(Layer {
            created_at: now_rfc3339(),
            packages: PackagesRef {
                file: "packages-cd.parquet".into(),
                size: 2,
                count: 3,
            },
            tables: [
                ("paths".into(), file("paths-ab.parquet")),
                ("reversed-paths".into(), file("reversed-paths-ef.parquet")),
            ]
            .into(),
        });
        let json = manifest.to_json().unwrap();
        assert_eq!(Manifest::parse(&json, &location()).unwrap(), manifest);
        assert!(manifest.has_kind(Kind::ReversedPaths));
        assert_eq!(manifest.package_count(), 3);
        assert_eq!(manifest.files().count(), 3);
        assert!(!String::from_utf8(json).unwrap().contains("removed"));
        assert!(manifest.created_at.ends_with('Z'));
        assert!(!manifest.created_at.contains('.'));
    }

    #[test]
    fn reads_the_cep_example() {
        let json = br#"{
          "version": 1,
          "channel": "https://conda.anaconda.org/conda-forge/",
          "subdir": "linux-64",
          "created_at": "2026-09-24T03:00:00Z",
          "kinds": ["paths", "reversed-paths"],
          "layers": [
            {
              "created_at": "2026-09-01T02:00:00Z",
              "packages": { "file": "packages-3f1c.parquet", "size": 3846270, "count": 783515 },
              "tables": {
                "paths": { "file": "paths-9c1e.parquet", "size": 162110748 },
                "reversed-paths": { "file": "reversed-paths-e2a0.parquet", "size": 100558813 }
              }
            }
          ],
          "removed": ["foo-1.0-h1234_0.conda"]
        }"#;
        let manifest = Manifest::parse(json, &location()).unwrap();
        assert_eq!(manifest.removed, vec!["foo-1.0-h1234_0.conda".to_string()]);
        assert_eq!(
            manifest.layers[0].table(Kind::Paths),
            Some(&FileRef {
                file: "paths-9c1e.parquet".into(),
                size: 162110748
            })
        );
        assert_eq!(
            manifest.known_kinds(),
            vec![Kind::Paths, Kind::ReversedPaths]
        );
    }

    #[test]
    fn rejects_other_versions() {
        for version in [0, 2] {
            let json = format!(
                r#"{{"version":{version},"channel":"c","subdir":"s","created_at":"t","kinds":["paths"],"layers":[]}}"#
            );
            assert!(matches!(
                Manifest::parse(json.as_bytes(), &location()),
                Err(LookupError::UnsupportedManifestVersion { version: v, .. }) if v == version
            ));
        }
    }

    #[test]
    fn rejects_invalid_manifests() {
        let cases: [&[u8]; 4] = [
            // A file name with a path.
            br#"{"version":1,"channel":"c","subdir":"s","created_at":"t","kinds":["paths"],
            "layers":[{"created_at":"t","tables":{"paths":{"file":"../x.parquet","size":1}},
             "packages":{"file":"p.parquet","size":1,"count":1}}]}"#,
            // A layer without a listed kind.
            br#"{"version":1,"channel":"c","subdir":"s","created_at":"t",
            "kinds":["paths","reversed-paths"],
            "layers":[{"created_at":"t","tables":{"paths":{"file":"a.parquet","size":1}},
             "packages":{"file":"p.parquet","size":1,"count":1}}]}"#,
            // No kinds.
            br#"{"version":1,"channel":"c","subdir":"s","created_at":"t","kinds":[],"layers":[]}"#,
            // Duplicate kinds.
            br#"{"version":1,"channel":"c","subdir":"s","created_at":"t","kinds":["paths","paths"],"layers":[]}"#,
        ];
        for json in cases {
            assert!(matches!(
                Manifest::parse(json, &location()),
                Err(LookupError::InvalidManifest { .. })
            ));
        }
    }

    #[test]
    fn ignores_unknown_kinds_and_fields() {
        let json = br#"{"version":1,"channel":"c","subdir":"s","created_at":"t",
            "kinds":["paths","file-sha256"],"future":1,
            "layers":[{"created_at":"t","tables":{"paths":{"file":"a.parquet","size":1},
             "file-sha256":{"file":"b.parquet","size":1}, "x-other":{"file":"c.parquet","size":1}},
             "packages":{"file":"p.parquet","size":1,"count":1}}]}"#;
        let manifest = Manifest::parse(json, &location()).unwrap();
        assert!(manifest.has_kind(Kind::Paths));
        assert!(!manifest.has_kind(Kind::ReversedPaths));
        assert_eq!(manifest.known_kinds(), vec![Kind::Paths]);
    }

    #[test]
    fn locates_manifests() {
        assert_eq!(
            manifest_location(&Location::parse("https://x.org/c/"), "noarch").to_string(),
            "https://x.org/c/noarch/lookup/manifest.json"
        );
        assert_eq!(manifest_path("noarch"), "noarch/lookup/manifest.json");
    }
}
