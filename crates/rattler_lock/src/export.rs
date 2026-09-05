//! Opt-in checks for URLs in reusable lockfiles.

use pep508_rs::VersionOrUrl;
use rattler_redaction::strip_url_for_serialization;
use url::Url;

use crate::{
    CondaPackageData, FindLinksUrlOrPath, LockFile, LockedPackage, PackageBuildSource,
    PypiPackageData, UrlOrPath, source::SourceLocation,
};

/// A URL field that cannot be exported under the durable-output URL policy.
///
/// Neither the original URL nor user-supplied names are retained in this error,
/// so its `Display` and `Debug` implementations do not disclose those values.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("URL at {location} is unsafe or invalid for export")]
pub struct UrlExportError {
    /// The field location, using numeric indices rather than user-supplied names.
    /// Package indices refer to [`LockFile::packages`].
    pub location: String,
}

impl LockFile {
    /// Check URL fields before publishing or committing this lockfile.
    ///
    /// Rejects URLs that [`rattler_redaction::strip_url_for_serialization`] would
    /// change: userinfo, Conda token prefixes, query strings, and non-digest
    /// fragments. This is conservative: even a public username or a non-secret
    /// query is rejected. Callers must supply durable resource URLs and arrange
    /// authentication separately rather than silently changing a resource URL.
    ///
    /// Checks Conda and `PyPI` package locations, channels, indexes, source URLs,
    /// direct `PyPI` requirements, and preserved verbatim URL spellings. Local
    /// paths are accepted. This is not a general secret scanner: arbitrary
    /// metadata, path contents, and unrecognized credential conventions are not
    /// checked.
    ///
    /// The first failing field is returned without including the original URL.
    /// This method does not modify the lockfile. Ordinary serialization,
    /// [`Self::render_to_string`], and [`Self::to_path`] remain lossless and do not
    /// invoke this check automatically.
    ///
    /// ```
    /// # use rattler_lock::LockFile;
    /// # fn export(lock: &LockFile) -> Result<String, Box<dyn std::error::Error>> {
    /// lock.validate_urls_for_export()?;
    /// let contents = lock.render_to_string()?;
    /// # Ok(contents)
    /// # }
    /// ```
    pub fn validate_urls_for_export(&self) -> Result<(), UrlExportError> {
        for (environment_index, environment) in self.inner.environments.iter().enumerate() {
            let field = format!("environments[{environment_index}]");
            for (index, channel) in environment.channels.iter().enumerate() {
                check_text(&channel.url, &format!("{field}.channels[{index}].url"))?;
            }
            if let Some(indexes) = &environment.indexes {
                for (index, url) in indexes.indexes.iter().enumerate() {
                    check_url(url, &format!("{field}.indexes[{index}]"))?;
                }
                for (index, link) in indexes.find_links.iter().enumerate() {
                    if let FindLinksUrlOrPath::Url(url) = link {
                        check_url(url, &format!("{field}.find_links[{index}]"))?;
                    }
                }
            }
        }
        // Also covers packages referenced only by source build/host environments.
        for (index, package) in self.packages().iter().enumerate() {
            let field = format!("packages[{index}]");
            match package {
                LockedPackage::Conda(CondaPackageData::Binary(package)) => {
                    check_location(&package.location, &format!("{field}.location"))?;
                    if let Some(channel) = &package.channel {
                        check_url(channel.as_ref(), &format!("{field}.channel"))?;
                    }
                }
                LockedPackage::Conda(CondaPackageData::Source(package)) => {
                    check_location(&package.location, &format!("{field}.location"))?;
                    match &package.package_build_source {
                        Some(
                            PackageBuildSource::Git { url, .. }
                            | PackageBuildSource::Url { url, .. },
                        ) => {
                            check_url(url, &format!("{field}.package_build_source"))?;
                        }
                        Some(PackageBuildSource::Path { .. }) | None => {}
                    }
                    for (index, source) in package.sources.values().enumerate() {
                        let url = match source {
                            SourceLocation::Url(source) => &source.url,
                            SourceLocation::Git(source) => &source.git,
                            SourceLocation::Path(_) => continue,
                        };
                        check_url(url, &format!("{field}.sources[{index}]"))?;
                    }
                }
                LockedPackage::Pypi(package) => {
                    let location = package.location();
                    check_location(location.inner(), &format!("{field}.location"))?;
                    if let Some(given) = location.given() {
                        check_text(given, &format!("{field}.location.given"))?;
                    }
                    if let PypiPackageData::Distribution(package) = package
                        && let Some(index) = &package.index_url
                    {
                        check_url(index, &format!("{field}.index_url"))?;
                    }
                    for (index, requirement) in package.requires_dist().iter().enumerate() {
                        if let Some(VersionOrUrl::Url(url)) = &requirement.version_or_url {
                            let field = format!("{field}.requires_dist[{index}]");
                            check_url(url, &field)?;
                            if let Some(given) = url.given() {
                                check_text(given, &field)?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn check_url(url: &Url, location: &str) -> Result<(), UrlExportError> {
    if strip_url_for_serialization(url) != *url {
        return Err(UrlExportError {
            location: location.to_owned(),
        });
    }
    Ok(())
}

fn check_location(value: &UrlOrPath, location: &str) -> Result<(), UrlExportError> {
    if let UrlOrPath::Url(url) = value {
        check_url(url, location)?;
    }
    Ok(())
}

fn check_text(value: &str, location: &str) -> Result<(), UrlExportError> {
    // Keep file URLs intact while checking: normalizing them to paths first can
    // discard URL components. Windows drive paths are not URL userinfo/schemes.
    match Url::parse(value) {
        Ok(url) if url.scheme().len() == 1 && url.scheme().as_bytes()[0].is_ascii_alphabetic() => {
            Ok(())
        }
        Ok(url) => check_url(&url, location),
        Err(url::ParseError::RelativeUrlWithoutBase) => Ok(()),
        Err(_) => Err(UrlExportError {
            location: location.to_owned(),
        }),
    }
}
