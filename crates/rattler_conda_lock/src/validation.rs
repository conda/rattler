//! Semantic validation shared by programmatic models and parsed documents.

use std::{collections::BTreeMap, str::FromStr};

use rattler_conda_types::{MatchSpec, PackageName, ParseStrictness, Version};

use crate::error::{ErrorKind, NodePath};
use crate::{Error, Hashes, LockFile, Manager};

/// The SHA256 digest length CEP-37 requires for `content_hash` and `sha256`.
const SHA256_DIGITS: usize = 64;
/// The MD5 digest length CEP-37 requires for `md5`.
const MD5_DIGITS: usize = 32;

impl LockFile {
    /// Validate CEP-37 fields, unique identities, and metadata references.
    ///
    /// This does not fetch artifacts, solve dependencies, or compare provenance
    /// hashes with [`Self::compute_content_hashes`]. Historical ecosystem name
    /// spellings and lenient conda `MatchSpecs` are accepted without normalization.
    pub fn validate(&self) -> Result<(), Error> {
        let metadata = NodePath::root().field("metadata");
        let platforms_path = metadata.field("platforms");
        let content_hash_path = metadata.field("content_hash");
        let mut platforms = BTreeMap::new();
        for (index, platform) in self.metadata.platforms.iter().enumerate() {
            let path = platforms_path.index(index);
            validate_platform(platform, &path)?;
            if let Some(previous) = platforms.insert(platform.as_str(), index) {
                return Err(Error::new(path, ErrorKind::DuplicatePlatform)
                    .with_related_path(platforms_path.index(previous), "first declared here"));
            }
            if !self.metadata.content_hash.contains_key(platform) {
                return Err(Error::new(
                    content_hash_path.clone(),
                    ErrorKind::MissingContentHash {
                        platform: platform.clone(),
                    },
                )
                .with_related_path(path, "target declared here"));
            }
        }
        for (platform, hash) in &self.metadata.content_hash {
            let path = content_hash_path.field(platform);
            if !platforms.contains_key(platform.as_str()) {
                return Err(Error::new(path, ErrorKind::UndeclaredContentHashPlatform)
                    .with_related_path(platforms_path.clone(), "declared targets"));
            }
            validate_digest(hash, SHA256_DIGITS, &path)?;
        }
        let channels_path = metadata.field("channels");
        for (index, channel) in self.metadata.channels.iter().enumerate() {
            if channel.url.trim().is_empty() {
                return Err(Error::new(
                    channels_path.index(index).field("url"),
                    ErrorKind::EmptyChannel,
                ));
            }
        }
        let sources_path = metadata.field("sources");
        let mut sources = BTreeMap::new();
        for (index, source) in self.metadata.sources.iter().enumerate() {
            let path = sources_path.index(index);
            validate_source_path(source, &path)?;
            if let Some(previous) = sources.insert(source.as_str(), index) {
                return Err(Error::new(path, ErrorKind::DuplicateSource)
                    .with_related_path(sources_path.index(previous), "first declared here"));
            }
        }
        if let Some(time) = &self.metadata.time_metadata {
            let value = time.created_at.as_bytes();
            let shape = value.len() == 20
                && value.iter().enumerate().all(|(i, c)| match i {
                    4 | 7 => *c == b'-',
                    10 => *c == b'T',
                    13 | 16 => *c == b':',
                    19 => *c == b'Z',
                    _ => c.is_ascii_digit(),
                });
            if !shape || time.created_at.parse::<jiff::Timestamp>().is_err() {
                return Err(Error::new(
                    metadata.field("time_metadata").field("created_at"),
                    ErrorKind::InvalidTimestamp,
                ));
            }
        }
        if let Some(inputs) = &self.metadata.inputs_metadata {
            let inputs_path = metadata.field("inputs_metadata");
            for (source, hashes) in inputs {
                let path = inputs_path.field(source);
                if !sources.contains_key(source.as_str()) {
                    return Err(Error::new(path, ErrorKind::UndeclaredInputSource)
                        .with_related_path(sources_path.clone(), "declared sources"));
                }
                validate_hashes(hashes, &path)?;
            }
            for (source, index) in &sources {
                if !inputs.contains_key(*source) {
                    return Err(Error::new(
                        inputs_path.clone(),
                        ErrorKind::MissingInputHashes {
                            source_path: (*source).to_owned(),
                        },
                    )
                    .with_related_path(sources_path.index(*index), "source declared here"));
                }
            }
        }
        let packages_path = NodePath::root().field("package");
        let mut identities = BTreeMap::new();
        for (index, package) in self.package.iter().enumerate() {
            let path = packages_path.index(index);
            validate_name(&package.name, package.manager, &path.field("name"))?;
            let valid_version = match package.manager {
                Manager::Conda => {
                    package.version.len() <= 64 && Version::from_str(&package.version).is_ok()
                }
                Manager::Pip => pep440_rs::Version::from_str(&package.version).is_ok(),
            };
            if !valid_version {
                return Err(Error::new(
                    path.field("version"),
                    ErrorKind::InvalidVersion {
                        manager: package.manager,
                    },
                ));
            }
            if !platforms.contains_key(package.platform.as_str()) {
                return Err(Error::new(
                    path.field("platform"),
                    ErrorKind::UndeclaredPackagePlatform,
                )
                .with_related_path(platforms_path.clone(), "declared targets"));
            }
            if package.category.is_empty() {
                return Err(Error::new(path.field("category"), ErrorKind::EmptyCategory));
            }
            if let Some(previous) = identities.insert(package.identity(), index) {
                return Err(
                    Error::new(path.clone(), ErrorKind::DuplicatePackageIdentity)
                        .with_related_path(packages_path.index(previous), "first declared here"),
                );
            }
            validate_url(&package.url, &path.field("url"))?;
            if let Some(source) = &package.source {
                validate_url(&source.url, &path.field("source").field("url"))?;
            }
            // conda-lock records the revision of a direct source in `hash.sha256`
            // instead of an artifact digest, so only artifacts require exact
            // digest lengths. Both remain hexadecimal.
            let hash_path = path.field("hash");
            if package.source.is_some() {
                validate_revision(&package.hash, &hash_path)?;
            } else {
                validate_hashes(&package.hash, &hash_path)?;
            }
            if let Some(build) = &package.build
                && package.manager == Manager::Conda
                && (build.is_empty()
                    || build.len() > 64
                    || !build
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'+')))
            {
                return Err(Error::new(
                    path.field("build"),
                    ErrorKind::InvalidBuildString,
                ));
            }
            let dependencies_path = path.field("dependencies");
            for (name, constraint) in &package.dependencies {
                let dependency_path = dependencies_path.field(name);
                validate_name(name, package.manager, &dependency_path)?;
                let valid = match package.manager {
                    Manager::Conda => MatchSpec::from_str(
                        &format!("{name} {constraint}"),
                        ParseStrictness::Lenient,
                    )
                    .is_ok(),
                    Manager::Pip => validate_python_constraint(constraint),
                };
                if !valid {
                    return Err(Error::new(
                        dependency_path,
                        ErrorKind::InvalidDependencyConstraint {
                            manager: package.manager,
                        },
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Accepts PEP 440 specifiers plus the forms conda-lock emits: `*` for an
/// unconstrained dependency, a bare version literal for an exact pin, and
/// `||`-separated alternatives derived from Poetry constraints.
fn validate_python_constraint(value: &str) -> bool {
    value.trim() == "*"
        || value.split("||").all(|alternative| {
            let alternative = alternative.trim();
            pep440_rs::VersionSpecifiers::from_str(alternative).is_ok()
                || pep440_rs::Version::from_str(alternative).is_ok()
        })
}

fn validate_platform(value: &str, path: &NodePath) -> Result<(), Error> {
    let valid = value.len() <= 32
        && value.split_once('-').is_some_and(|(os, arch)| {
            !os.is_empty()
                && !arch.is_empty()
                && os
                    .bytes()
                    .chain(arch.bytes())
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        });
    if valid {
        Ok(())
    } else {
        Err(Error::new(path.clone(), ErrorKind::InvalidPlatform))
    }
}

fn validate_name(value: &str, manager: Manager, path: &NodePath) -> Result<(), Error> {
    let valid = match manager {
        Manager::Conda => value.len() <= 64 && PackageName::from_str(value).is_ok(),
        Manager::Pip => pep508_rs::PackageName::from_str(value).is_ok(),
    };
    if valid {
        Ok(())
    } else {
        Err(Error::new(
            path.clone(),
            ErrorKind::InvalidPackageName { manager },
        ))
    }
}

fn validate_url(value: &str, path: &NodePath) -> Result<(), Error> {
    if value.is_empty() || value.chars().any(char::is_whitespace) || url::Url::parse(value).is_err()
    {
        Err(Error::new(path.clone(), ErrorKind::InvalidUrl))
    } else {
        Ok(())
    }
}

fn validate_source_path(value: &str, path: &NodePath) -> Result<(), Error> {
    // Check both path syntaxes, regardless of the host OS reading the lockfile.
    if value.is_empty()
        || value.contains('\0')
        || value.starts_with(['/', '\\'])
        || value.as_bytes().get(1) == Some(&b':')
        || url::Url::parse(value).is_ok()
    {
        Err(Error::new(path.clone(), ErrorKind::InvalidSourcePath))
    } else {
        Ok(())
    }
}

fn validate_hashes(value: &Hashes, path: &NodePath) -> Result<(), Error> {
    if let Some(md5) = &value.md5 {
        validate_digest(md5, MD5_DIGITS, &path.field("md5"))?;
    }
    if let Some(sha256) = &value.sha256 {
        validate_digest(sha256, SHA256_DIGITS, &path.field("sha256"))?;
    }
    Ok(())
}

/// Direct-source packages carry a revision rather than an artifact digest.
fn validate_revision(value: &Hashes, path: &NodePath) -> Result<(), Error> {
    for (field, digest) in [("md5", &value.md5), ("sha256", &value.sha256)] {
        if let Some(digest) = digest
            && (digest.is_empty()
                || digest.len() > SHA256_DIGITS
                || !digest.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(Error::new(path.field(field), ErrorKind::InvalidRevision));
        }
    }
    Ok(())
}

fn validate_digest(value: &str, length: usize, path: &NodePath) -> Result<(), Error> {
    if value.len() == length && value.bytes().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(Error::new(
            path.clone(),
            ErrorKind::InvalidDigest { length },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn future_subdirs_but_not_artifact_noarch_are_targets() {
        let path = NodePath::root().field("target");
        assert!(validate_platform("future-cpu123", &path).is_ok());
        for invalid in ["noarch", "linux-", "-64", "linux-x86-64", "Linux-64"] {
            assert!(validate_platform(invalid, &path).is_err());
        }
    }

    #[test]
    fn relative_sources_are_host_independent() {
        let path = NodePath::root().field("source");
        for invalid in [
            "/etc/env.yml",
            "C:\\env.yml",
            "C:env.yml",
            "\\\\server\\env.yml",
            "https://example.org/env.yml",
        ] {
            assert!(validate_source_path(invalid, &path).is_err());
        }
        assert!(validate_source_path("../environments/base.yml", &path).is_ok());
    }
}
