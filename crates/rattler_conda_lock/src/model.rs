//! Owned CEP-37 v1 data, independent of the source document.

use std::{collections::BTreeMap, fmt};

use serde::{Serialize, Serializer, ser::SerializeStruct};

/// A CEP-37 v1 lockfile. The schema version is implicit.
///
/// ```
/// use rattler_conda_lock::{Hashes, LockFile, Manager, Package};
///
/// let mut lock = LockFile::default();
/// lock.metadata.platforms.push("linux-64".into());
/// lock.package.push(Package {
///     name: "ca-certificates".into(),
///     version: "2025.10.5".into(),
///     manager: Manager::Conda,
///     platform: "linux-64".into(),
///     dependencies: Default::default(),
///     url: "https://conda.anaconda.org/conda-forge/noarch/ca-certificates-2025.10.5-hbd8a1cb_0.conda".into(),
///     hash: Hashes { md5: None, sha256: Some("3b5ad78b8bb61b6cdc0978a6a99f8dfb2cc789a451378d054698441005ecbdb6".into()) },
///     source: None,
///     build: Some("hbd8a1cb_0".into()),
///     category: "main".into(),
///     optional: false,
/// });
/// lock.metadata.content_hash = lock.compute_content_hashes();
///
/// // Writing is canonical, so reading the result back yields the same model.
/// let yaml = lock.to_yaml()?;
/// assert_eq!(yaml.parse::<LockFile>()?, lock);
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

impl fmt::Display for Manager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Conda => "conda",
            Self::Pip => "pip",
        })
    }
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

impl Package {
    /// The fields that decide what this artifact installs where: two packages
    /// with one identity make the lock file ambiguous, which
    /// [`LockFile::validate`] rejects.
    pub fn identity(&self) -> PackageIdentity<'_> {
        PackageIdentity {
            platform: &self.platform,
            manager: self.manager,
            name: &self.name,
            category: &self.category,
        }
    }
}

/// The install identity of a [`Package`], as returned by [`Package::identity`].
///
/// The field order is the canonical package order used by
/// [`LockFile::to_yaml`] and [`LockFile::compute_content_hashes`]: target
/// platform first, so that all packages for one platform are contiguous.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageIdentity<'a> {
    /// Install target platform.
    pub platform: &'a str,
    /// Installer responsible for the artifact.
    pub manager: Manager,
    /// Package name, as written.
    pub name: &'a str,
    /// Install category.
    pub category: &'a str,
}

impl fmt::Display for PackageIdentity<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            name,
            manager,
            platform,
            category,
        } = self;
        write!(f, "{manager}:{platform}:{category}:{name}")
    }
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
