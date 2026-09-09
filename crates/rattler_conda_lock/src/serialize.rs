//! Canonical, borrowed serialization and package-derived content hashing.

use std::{borrow::Cow, collections::BTreeMap, io, path::Path};

use serde::{
    Serialize, Serializer,
    ser::{SerializeMap, SerializeSeq},
};
use serde_json_python_formatter::PythonFormatter;
use sha2::{Digest, Sha256};

use crate::{
    Channel, Error, GitMetadata, Hashes, LockFile, Manager, Metadata, Package, PackageSource,
    TimeMetadata,
};

impl LockFile {
    /// Serialize validated CEP-37 v1 YAML without modifying this model.
    ///
    /// Packages and target platforms are sorted; maps use lexical key order.
    /// Channel priority and source order are preserved. Checksums are lowercase,
    /// absent checksums are omitted, and channel entries are always objects.
    pub fn to_yaml(&self) -> Result<String, Error> {
        self.validate()?;
        let packages = sorted_packages(self);
        let mut platforms: Vec<_> = self.metadata.platforms.iter().map(String::as_str).collect();
        platforms.sort_unstable();
        let document = CanonicalLock {
            version: 1,
            metadata: CanonicalMetadata::new(&self.metadata, &platforms),
            package: PackageList(&packages),
        };
        serde_saphyr::to_string(&document)
            .map_err(|error| Error::new("", format!("could not serialize lockfile: {error}")))
    }

    /// Validate and serialize before opening the destination, so invalid models
    /// never truncate an existing lockfile. Writing is not an atomic replacement.
    ///
    /// ```no_run
    /// # use rattler_conda_lock::LockFile;
    /// LockFile::default().to_path("conda-lock.yml")?;
    /// # Ok::<(), rattler_conda_lock::Error>(())
    /// ```
    pub fn to_path(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let yaml = self.to_yaml()?;
        let path = path.as_ref();
        std::fs::write(path, yaml)
            .map_err(|error| Error::new("", format!("could not write {}: {error}", path.display())))
    }

    /// Compute package-derived SHA256 hashes for each declared target platform.
    ///
    /// **These are not conda-lock's input-spec hashes.** Existing metadata hashes
    /// are neither validated nor included. This method does not validate the
    /// model.
    ///
    /// The preimage is the UTF-8 JSON that Python's
    /// `json.dumps(packages, sort_keys=True)` produces for the platform's
    /// packages, so an empty platform hashes `[]`. Each package object holds
    /// `build` (omitted if absent), `category`, `dependencies`, `hash`,
    /// `manager`, `name`, `optional`, `platform`, `source` (omitted if absent),
    /// `url` and `version`. Hash fields are `md5` then `sha256`, omitted if
    /// absent and ASCII-lowercased; source fields are `type` (always `url`) then
    /// `url`. Every other string, including names, URLs and dependency
    /// constraints, is hashed unchanged.
    ///
    /// Packages sort by `(platform, manager, name, category)` (`conda` before
    /// `pip`). Ties, possible only for invalid duplicate identities, sort by the
    /// model's field order: name, version, manager, platform, dependencies, URL,
    /// hashes (MD5 then SHA256), source URL, build, category, optional. Strings
    /// and maps compare lexically, absent options precede present ones, and false
    /// precedes true. Thus input package order never affects the result.
    ///
    /// ```
    /// use rattler_conda_lock::LockFile;
    /// let mut lock = LockFile::default();
    /// lock.metadata.platforms.push("linux-64".into());
    /// lock.metadata.content_hash = lock.compute_content_hashes();
    /// lock.validate()?;
    /// assert_eq!(lock.metadata.content_hash["linux-64"],
    ///     "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945");
    /// # Ok::<(), rattler_conda_lock::Error>(())
    /// ```
    pub fn compute_content_hashes(&self) -> BTreeMap<String, String> {
        let packages = sorted_packages(self);
        self.metadata
            .platforms
            .iter()
            .map(|platform| {
                let start = packages
                    .partition_point(|package| package.platform.as_str() < platform.as_str());
                let end = packages
                    .partition_point(|package| package.platform.as_str() <= platform.as_str());
                let mut serializer = serde_json::Serializer::with_formatter(
                    HashWriter(Sha256::new()),
                    PythonFormatter::default(),
                );
                // Only strings, booleans, maps and arrays reach an infallible
                // in-memory writer; no user-provided serializer is called.
                PackageList(&packages[start..end])
                    .serialize(&mut serializer)
                    .expect("canonical package JSON is infallible");
                (
                    platform.clone(),
                    hex::encode(serializer.into_inner().0.finalize()),
                )
            })
            .collect()
    }
}

fn sorted_packages(lock: &LockFile) -> Vec<&Package> {
    let mut packages: Vec<_> = lock.package.iter().collect();
    packages.sort_unstable_by(|a, b| {
        (&a.platform, a.manager, &a.name, &a.category)
            .cmp(&(&b.platform, b.manager, &b.name, &b.category))
            .then_with(|| a.cmp(b))
    });
    packages
}

struct HashWriter(Sha256);

impl io::Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Serialize)]
struct CanonicalLock<'a> {
    version: u8,
    metadata: CanonicalMetadata<'a>,
    package: PackageList<'a>,
}

#[derive(Serialize)]
struct CanonicalMetadata<'a> {
    content_hash: ContentHashes<'a>,
    channels: &'a [Channel],
    platforms: &'a [&'a str],
    sources: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    time_metadata: Option<&'a TimeMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_metadata: Option<&'a GitMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inputs_metadata: Option<InputHashes<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    custom_metadata: Option<&'a BTreeMap<String, String>>,
}

impl<'a> CanonicalMetadata<'a> {
    fn new(metadata: &'a Metadata, platforms: &'a [&'a str]) -> Self {
        Self {
            content_hash: ContentHashes(&metadata.content_hash),
            channels: &metadata.channels,
            platforms,
            sources: &metadata.sources,
            time_metadata: metadata.time_metadata.as_ref(),
            git_metadata: metadata.git_metadata.as_ref(),
            inputs_metadata: metadata.inputs_metadata.as_ref().map(InputHashes),
            custom_metadata: metadata.custom_metadata.as_ref(),
        }
    }
}

struct ContentHashes<'a>(&'a BTreeMap<String, String>);

impl Serialize for ContentHashes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (platform, hash) in self.0 {
            map.serialize_entry(platform, &lowercase(hash))?;
        }
        map.end()
    }
}

struct InputHashes<'a>(&'a BTreeMap<String, Hashes>);

impl Serialize for InputHashes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (path, hashes) in self.0 {
            map.serialize_entry(path, &CanonicalHashes::new(hashes))?;
        }
        map.end()
    }
}

struct PackageList<'a>(&'a [&'a Package]);

impl Serialize for PackageList<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for package in self.0 {
            sequence.serialize_element(&CanonicalPackage::new(package))?;
        }
        sequence.end()
    }
}

#[derive(Serialize)]
struct CanonicalPackage<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    build: Option<&'a str>,
    category: &'a str,
    dependencies: &'a BTreeMap<String, String>,
    hash: CanonicalHashes<'a>,
    manager: Manager,
    name: &'a str,
    optional: bool,
    platform: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a PackageSource>,
    url: &'a str,
    version: &'a str,
}

impl<'a> CanonicalPackage<'a> {
    fn new(package: &'a Package) -> Self {
        Self {
            build: package.build.as_deref(),
            category: &package.category,
            dependencies: &package.dependencies,
            hash: CanonicalHashes::new(&package.hash),
            manager: package.manager,
            name: &package.name,
            optional: package.optional,
            platform: &package.platform,
            source: package.source.as_ref(),
            url: &package.url,
            version: &package.version,
        }
    }
}

#[derive(Serialize)]
struct CanonicalHashes<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    md5: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<Cow<'a, str>>,
}

impl<'a> CanonicalHashes<'a> {
    fn new(hashes: &'a Hashes) -> Self {
        Self {
            md5: hashes.md5.as_deref().map(lowercase),
            sha256: hashes.sha256.as_deref().map(lowercase),
        }
    }
}

fn lowercase(value: &str) -> Cow<'_, str> {
    if value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        Cow::Owned(value.to_ascii_lowercase())
    } else {
        Cow::Borrowed(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock() -> LockFile {
        let mut lock = LockFile::default();
        lock.metadata.platforms = vec!["linux-64".into(), "win-64".into()];
        lock.package.push(Package {
            name: "example".into(),
            version: "1.0".into(),
            manager: Manager::Conda,
            platform: "linux-64".into(),
            dependencies: BTreeMap::new(),
            url: "https://example.org/noarch/example-1.0-0.conda".into(),
            hash: Hashes {
                md5: None,
                sha256: Some("AB".repeat(32)),
            },
            source: None,
            build: Some("0".into()),
            category: "main".into(),
            optional: false,
        });
        lock
    }

    #[test]
    fn hashes_ignore_provenance_but_cover_package_semantics() {
        let mut lock = lock();
        let original = lock.compute_content_hashes();
        lock.metadata.sources.push("environment.yml".into());
        lock.metadata
            .content_hash
            .insert("linux-64".into(), "not yet a digest".into());
        assert_eq!(original, lock.compute_content_hashes());
        lock.package[0].optional = true;
        let changed = lock.compute_content_hashes();
        assert_ne!(original["linux-64"], changed["linux-64"]);
        assert_eq!(original["win-64"], changed["win-64"]);
    }

    #[test]
    fn order_and_checksum_case_do_not_change_output() {
        let mut lock = lock();
        let mut other = lock.package[0].clone();
        other.name = "another".into();
        lock.package.push(other);
        lock.metadata.content_hash = lock.compute_content_hashes();
        let yaml = lock.to_yaml().unwrap();
        let hashes = lock.compute_content_hashes();
        lock.package.reverse();
        lock.metadata.platforms.reverse();
        for package in &mut lock.package {
            package.hash.sha256 = package
                .hash
                .sha256
                .take()
                .map(|value| value.to_ascii_lowercase());
        }
        assert_eq!(yaml, lock.to_yaml().unwrap());
        assert_eq!(hashes, lock.compute_content_hashes());
        assert!(!yaml.contains("md5:"));
        let parsed = yaml.parse::<LockFile>().unwrap();
        assert_eq!(parsed.to_yaml().unwrap(), yaml);
        assert_eq!(
            parsed.package[0].hash.sha256.as_deref(),
            Some("abababababababababababababababababababababababababababababababab")
        );
    }
}
