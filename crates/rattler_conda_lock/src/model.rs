//! Owned CEP-37 v1 data, independent of the source document.

use std::collections::BTreeMap;

use serde::{Serialize, Serializer, ser::SerializeStruct};

/// A CEP-37 v1 lockfile. The schema version is implicit.
///
/// ```
/// use rattler_conda_lock::LockFile;
/// let lock = LockFile::default();
/// assert!(lock.validate().is_ok());
/// assert!(lock.to_yaml()?.contains("version: 1"));
/// # Ok::<(), rattler_conda_lock::Error>(())
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LockFile {
    /// Provenance and declared target platforms.
    pub metadata: Metadata,
    /// Locked artifacts, including their install categories.
    pub package: Vec<Package>,
}

/// The installer responsible for an artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Manager {
    /// A conda artifact.
    Conda,
    /// A Python distribution.
    Pip,
}

/// One artifact selected for a target platform and install category.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Package {
    /// Original package-name spelling.
    pub name: String,
    /// Original resolved version spelling.
    pub version: String,
    /// Installer for this artifact.
    pub manager: Manager,
    /// Install target, not the artifact subdirectory (which may be `noarch`).
    pub platform: String,
    /// Dependency names and constraint strings.
    pub dependencies: BTreeMap<String, String>,
    /// Direct artifact download URL.
    pub url: String,
    /// Available artifact checksums; neither algorithm is mandatory.
    pub hash: Hashes,
    /// Optional URL provenance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<PackageSource>,
    /// Optional conda build string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// Install group, conventionally `main`.
    pub category: String,
    /// Whether the artifact is optional.
    pub optional: bool,
}

/// Optional hexadecimal checksums. Empty and SHA256-only hashes are valid.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Hashes {
    /// MD5 digest, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    /// SHA256 digest, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// URL provenance; its serialized `type` is always `url`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PackageSource {
    /// Original source URL spelling.
    pub url: String,
}

impl Serialize for PackageSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("PackageSource", 2)?;
        state.serialize_field("type", "url")?;
        state.serialize_field("url", &self.url)?;
        state.end()
    }
}

/// Lockfile provenance. Optional empty maps remain distinct from absent maps.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Metadata {
    /// Per-target SHA256 content hashes.
    pub content_hash: BTreeMap<String, String>,
    /// Channels in priority order.
    pub channels: Vec<Channel>,
    /// Declared install targets, excluding `noarch`.
    pub platforms: Vec<String>,
    /// Source paths relative to the lockfile.
    pub sources: Vec<String>,
    /// Creation timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_metadata: Option<TimeMetadata>,
    /// Git provenance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_metadata: Option<GitMetadata>,
    /// Checksums of the declared source files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs_metadata: Option<BTreeMap<String, Hashes>>,
    /// Application-defined string metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_metadata: Option<BTreeMap<String, String>>,
}

/// A channel URL or conventional channel name, without environment expansion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Channel {
    /// Channel location, preserving its spelling.
    pub url: String,
    /// Environment variable names needed to access the channel.
    pub used_env_vars: Vec<String>,
}

/// UTC creation time with whole-second precision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TimeMetadata {
    /// Timestamp in `YYYY-MM-DDTHH:MM:SSZ` form.
    pub created_at: String,
}

/// Optional git repository and author provenance.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GitMetadata {
    /// Git `user.name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_user_name: Option<String>,
    /// Git `user.email`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_user_email: Option<String>,
    /// Commit identifying the most recent modification of the sources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
}
