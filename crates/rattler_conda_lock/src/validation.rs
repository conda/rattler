//! Semantic validation shared by programmatic models and parsed documents.

use std::{collections::BTreeMap, str::FromStr};

use rattler_conda_types::{MatchSpec, PackageName, ParseStrictness, Version};

use crate::{Error, Hashes, LockFile, Manager};

impl LockFile {
    /// Validate CEP-37 fields, unique identities, and metadata references.
    ///
    /// This does not fetch artifacts, solve dependencies, or compare provenance
    /// hashes with [`Self::compute_content_hashes`]. Historical ecosystem name
    /// spellings and lenient conda `MatchSpecs` are accepted without normalization.
    pub fn validate(&self) -> Result<(), Error> {
        let mut platforms = BTreeMap::new();
        for (index, platform) in self.metadata.platforms.iter().enumerate() {
            let path = format!("metadata.platforms[{index}]");
            validate_platform(platform, &path)?;
            if let Some(previous) = platforms.insert(platform.as_str(), index) {
                return Err(
                    Error::new(path, "duplicate target platform").with_related_path(
                        format!("metadata.platforms[{previous}]"),
                        "first declared here",
                    ),
                );
            }
            if !self.metadata.content_hash.contains_key(platform) {
                return Err(Error::new(
                    "metadata.content_hash",
                    format!("missing content hash for {platform}"),
                )
                .with_related_path(path, "target declared here"));
            }
        }
        for (platform, hash) in &self.metadata.content_hash {
            let path = format!("metadata.content_hash.{platform}");
            if !platforms.contains_key(platform.as_str()) {
                return Err(Error::new(
                    path,
                    "content hash refers to an undeclared target platform",
                )
                .with_related_path("metadata.platforms", "declared targets"));
            }
            validate_digest(hash, 64, &path)?;
        }
        for (index, channel) in self.metadata.channels.iter().enumerate() {
            if channel.url.trim().is_empty() {
                return Err(Error::new(
                    format!("metadata.channels[{index}].url"),
                    "channel URL or name must not be empty",
                ));
            }
        }
        let mut sources = BTreeMap::new();
        for (index, source) in self.metadata.sources.iter().enumerate() {
            let path = format!("metadata.sources[{index}]");
            validate_source_path(source, &path)?;
            if let Some(previous) = sources.insert(source.as_str(), index) {
                return Err(Error::new(path, "duplicate source path").with_related_path(
                    format!("metadata.sources[{previous}]"),
                    "first declared here",
                ));
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
                    "metadata.time_metadata.created_at",
                    "expected a valid UTC timestamp in YYYY-MM-DDTHH:MM:SSZ form",
                ));
            }
        }
        if let Some(inputs) = &self.metadata.inputs_metadata {
            for (source, hashes) in inputs {
                let path = format!("metadata.inputs_metadata.{source}");
                if !sources.contains_key(source.as_str()) {
                    return Err(
                        Error::new(path, "input hash refers to an undeclared source")
                            .with_related_path("metadata.sources", "declared sources"),
                    );
                }
                validate_hashes(hashes, &path)?;
            }
            for (source, index) in &sources {
                if !inputs.contains_key(*source) {
                    return Err(Error::new(
                        "metadata.inputs_metadata",
                        format!("missing input hashes for {source}"),
                    )
                    .with_related_path(
                        format!("metadata.sources[{index}]"),
                        "source declared here",
                    ));
                }
            }
        }
        let mut identities = BTreeMap::new();
        for (index, package) in self.package.iter().enumerate() {
            let path = format!("package[{index}]");
            validate_name(&package.name, package.manager, &format!("{path}.name"))?;
            let valid_version = match package.manager {
                Manager::Conda => {
                    package.version.len() <= 64 && Version::from_str(&package.version).is_ok()
                }
                Manager::Pip => pep440_rs::Version::from_str(&package.version).is_ok(),
            };
            if !valid_version {
                return Err(Error::new(
                    format!("{path}.version"),
                    "invalid resolved package version",
                ));
            }
            if !platforms.contains_key(package.platform.as_str()) {
                return Err(Error::new(
                    format!("{path}.platform"),
                    "package target is not declared in metadata.platforms",
                )
                .with_related_path("metadata.platforms", "declared targets"));
            }
            if package.category.is_empty() {
                return Err(Error::new(
                    format!("{path}.category"),
                    "package category must not be empty",
                ));
            }
            let identity = (
                &package.name,
                package.manager,
                &package.platform,
                &package.category,
            );
            if let Some(previous) = identities.insert(identity, index) {
                return Err(Error::new(
                    &path,
                    "duplicate (name, manager, platform, category) package identity",
                )
                .with_related_path(format!("package[{previous}]"), "first declared here"));
            }
            validate_url(&package.url, &format!("{path}.url"))?;
            if let Some(source) = &package.source {
                validate_url(&source.url, &format!("{path}.source.url"))?;
            }
            // conda-lock records the revision of a direct source in `hash.sha256`
            // instead of an artifact digest, so only artifacts require exact
            // digest lengths. Both remain hexadecimal.
            if package.source.is_some() {
                validate_revision(&package.hash, &format!("{path}.hash"))?;
            } else {
                validate_hashes(&package.hash, &format!("{path}.hash"))?;
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
                    format!("{path}.build"),
                    "invalid conda build string",
                ));
            }
            for (name, constraint) in &package.dependencies {
                let dependency_path = format!("{path}.dependencies.{name}");
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
                        "invalid dependency constraint for this package manager",
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

fn validate_platform(value: &str, path: &str) -> Result<(), Error> {
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
        Err(Error::new(
            path,
            "expected a CEP-26 target subdir (os-arch), excluding noarch",
        ))
    }
}

fn validate_name(value: &str, manager: Manager, path: &str) -> Result<(), Error> {
    let valid = match manager {
        Manager::Conda => value.len() <= 64 && PackageName::from_str(value).is_ok(),
        Manager::Pip => pep508_rs::PackageName::from_str(value).is_ok(),
    };
    if valid {
        Ok(())
    } else {
        Err(Error::new(path, "invalid package name"))
    }
}

fn validate_url(value: &str, path: &str) -> Result<(), Error> {
    if value.is_empty() || value.chars().any(char::is_whitespace) || url::Url::parse(value).is_err()
    {
        Err(Error::new(
            path,
            "expected an absolute URL without whitespace",
        ))
    } else {
        Ok(())
    }
}

fn validate_source_path(value: &str, path: &str) -> Result<(), Error> {
    // Check both path syntaxes, regardless of the host OS reading the lockfile.
    if value.is_empty()
        || value.contains('\0')
        || value.starts_with(['/', '\\'])
        || value.as_bytes().get(1) == Some(&b':')
        || url::Url::parse(value).is_ok()
    {
        Err(Error::new(
            path,
            "source paths must be non-empty and relative to the lockfile",
        ))
    } else {
        Ok(())
    }
}

fn validate_hashes(value: &Hashes, path: &str) -> Result<(), Error> {
    if let Some(md5) = &value.md5 {
        validate_digest(md5, 32, &format!("{path}.md5"))?;
    }
    if let Some(sha256) = &value.sha256 {
        validate_digest(sha256, 64, &format!("{path}.sha256"))?;
    }
    Ok(())
}

/// Direct-source packages carry a revision rather than an artifact digest.
fn validate_revision(value: &Hashes, path: &str) -> Result<(), Error> {
    for (field, digest) in [("md5", &value.md5), ("sha256", &value.sha256)] {
        if let Some(digest) = digest
            && (digest.is_empty()
                || digest.len() > 64
                || !digest.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err(Error::new(
                format!("{path}.{field}"),
                "expected up to 64 hexadecimal digits",
            ));
        }
    }
    Ok(())
}

fn validate_digest(value: &str, length: usize, path: &str) -> Result<(), Error> {
    if value.len() == length && value.bytes().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(Error::new(
            path,
            format!("expected exactly {length} hexadecimal digits"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn future_subdirs_but_not_artifact_noarch_are_targets() {
        assert!(validate_platform("future-cpu123", "target").is_ok());
        for invalid in ["noarch", "linux-", "-64", "linux-x86-64", "Linux-64"] {
            assert!(validate_platform(invalid, "target").is_err());
        }
    }

    #[test]
    fn relative_sources_are_host_independent() {
        for invalid in [
            "/etc/env.yml",
            "C:\\env.yml",
            "C:env.yml",
            "\\\\server\\env.yml",
            "https://example.org/env.yml",
        ] {
            assert!(validate_source_path(invalid, "source").is_err());
        }
        assert!(validate_source_path("../environments/base.yml", "source").is_ok());
    }
}
