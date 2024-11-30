//! This module contains functions to derive certain fields from other fields in
//! a [`CondaPackageData`].
//!
//! For instance if we know the location of a package, and it contains a valid
//! archive identifier filename, then we can derive the name, version, build,
//! subdir and channel from it (see [`LocationDerivedFields`]).
//!
//! Care must be taken when changing the logic in this module, as it encodes how
//! we resolve empty fields in a lock-file. This is important for
//! reproducibility.

use std::fmt::{Debug, Formatter};
use std::str::FromStr;

use rattler_conda_types::{
    package::ArchiveIdentifier, BuildNumber, ChannelUrl, NoArchType, PackageName, Platform,
    VersionWithSource,
};
use url::Url;

use crate::UrlOrPath;

/// A helper struct that wraps all fields of a [`CondaPackageData`] that can be
/// derived just from the location of the package.
pub(crate) struct LocationDerivedFields {
    pub file_name: Option<String>,
    pub name: Option<PackageName>,
    pub version: Option<VersionWithSource>,
    pub build: Option<String>,
    pub subdir: Option<String>,
    pub channel: Option<ChannelUrl>,
}

impl Debug for LocationDerivedFields {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocationDerivedFields")
            .field("file_name", &self.file_name)
            .field("name", &self.name.as_ref().map(PackageName::as_source))
            .field("version", &self.version.as_ref().map(|s| s.as_str()))
            .field("build", &self.build)
            .field("subdir", &self.subdir)
            .field("channel", &self.channel.as_ref().map(ChannelUrl::as_str))
            .finish()
    }
}

impl LocationDerivedFields {
    /// Constructs a new instance by deriving all fields from the given
    /// location.
    pub fn new(location: &UrlOrPath) -> Self {
        let (file_name, archive_identifier) = location
            .file_name()
            .and_then(|f| ArchiveIdentifier::try_from_filename(f).map(|a| (f.to_string(), a)))
            .map_or((None, None), |(f, a)| (Some(f), Some(a)));
        let (name, version, build) = archive_identifier
            .and_then(|a| {
                Some((
                    PackageName::new_unchecked(a.name),
                    VersionWithSource::from_str(&a.version).ok()?,
                    a.build_string,
                ))
            })
            .map_or((None, None, None), |(name, version, build_string)| {
                (Some(name), Some(version), Some(build_string))
            });
        let subdir = derive_subdir_from_location(location).map(ToString::to_string);
        let channel = derive_channel_from_location(location);
        Self {
            file_name,
            name,
            version,
            build,
            subdir,
            channel,
        }
    }
}

/// Try to derive the build number from the build string. It is common to append
/// the build number to the end of the build string.
pub(crate) fn derive_build_number_from_build(build: &str) -> Option<BuildNumber> {
    let (_, trailing_number_str) = build
        .rsplit_once(|c: char| !c.is_ascii_digit())
        .unwrap_or(("", build));

    trailing_number_str.parse().ok()
}

/// Try to derive the subdir from a common conda URL. This assumes that the URL
/// is formatted as `scheme://../subdir/name-version-build.ext`.
pub(crate) fn derive_subdir_from_location(location: &UrlOrPath) -> Option<&str> {
    match location {
        UrlOrPath::Url(url) => derive_subdir_from_url(url),
        UrlOrPath::Path(_) => None,
    }
}

/// Try to derive the subdir from a common conda URL. This assumes that the URL
/// is formatted as `scheme://../subdir/name-version-build.ext`.
pub fn derive_subdir_from_url(url: &Url) -> Option<&str> {
    if !(url.scheme() == "file" || url.scheme() == "http" || url.scheme() == "https") {
        return None;
    }

    let mut path_iter = url.path_segments()?.rev();
    let archive_str = path_iter.next()?;
    let subdir_str = path_iter.next()?;

    // Try to parse the archive string as an archive identifier. If it fails we
    // can't derive the subdir.
    let _ = ArchiveIdentifier::try_from_filename(archive_str)?;

    // Parse the subdir as a platform, if it fails we can't derive the subdir.
    Platform::from_str(subdir_str).is_ok().then_some(subdir_str)
}

/// Channel from url, this is everything before the filename and the subdir
/// So for example: <https://conda.anaconda.org/conda-forge/> is a channel name
/// that we parse from something like: <https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda>
pub(crate) fn derive_channel_from_url(url: &Url) -> Option<ChannelUrl> {
    let mut result = url.clone();

    // Strip the last two path segments. We assume the first one contains the
    // file_name, and the other the subdirectory.
    result.path_segments_mut().ok()?.pop().pop();

    Some(result.into())
}

/// Returns the channel when deriving it from the location if possible.
pub(crate) fn derive_channel_from_location(url: &UrlOrPath) -> Option<ChannelUrl> {
    match url {
        UrlOrPath::Url(url) => derive_channel_from_url(url),
        UrlOrPath::Path(_) => None,
    }
}

pub(crate) fn derive_arch_and_platform(subdir: &str) -> (Option<String>, Option<String>) {
    let platform = Platform::from_str(subdir).ok();
    platform.map_or((None, None), |p| {
        (
            p.arch().map(|arch| arch.to_string()),
            p.only_platform().map(ToString::to_string),
        )
    })
}

/// Derive the noarch type from the subdir and the build string.
///
/// By default, the noarch type is set to `None`. However, if the channel is
/// `noarch`, then we assume that the package is a noarch package but it does
/// not tell us whether this is a generic or python noarch package. As a
/// heuristic we then look at the build string, if it contains the word `py` we
/// assume that this is a python noarch package.
pub(crate) fn derive_noarch_type(subdir: &str, build_string: &str) -> NoArchType {
    let is_python_build_string = build_string.contains("py");
    match subdir {
        "noarch" => {
            if is_python_build_string {
                NoArchType::python()
            } else {
                NoArchType::generic()
            }
        }
        _ => NoArchType::none(),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::*;

    #[test]
    fn test_derive_build_number_from_build() {
        assert_eq!(derive_build_number_from_build("2"), Some(2));
        assert_eq!(derive_build_number_from_build("1.2.3"), Some(3));
        assert_eq!(derive_build_number_from_build("1.2.3-4"), Some(4));
        assert_eq!(derive_build_number_from_build("py313hb6a6212_1"), Some(1));
        assert_eq!(derive_build_number_from_build("foobar"), None);
        assert_eq!(derive_build_number_from_build("123_"), None);
        assert_eq!(derive_build_number_from_build("123foo"), None);
        assert_eq!(derive_build_number_from_build("pyhd8ed1ab_100"), Some(100));
        assert_eq!(derive_build_number_from_build("h803f02a_3"), Some(3));
    }

    #[test]
    fn test_derive_subdir_from_url() {
        assert_eq!(
            derive_subdir_from_url(
                &"https://conda.anaconda.org/conda-forge/linux-aarch64/qt-5.15.8-h803f02a_0.conda"
                    .parse()
                    .unwrap()
            ),
            Some("linux-aarch64")
        );
        assert_eq!(
            derive_subdir_from_url(
                &"https://conda.anaconda.org/conda-forge/osx-arm64/x264-1!164.3095-h57fd34a_2.tar.bz2"
                    .parse().unwrap()
            ),
            Some("osx-arm64")
        );
        assert_eq!(
            derive_subdir_from_url(
                &"https://conda.anaconda.org/conda-forge/win-64/package-1.0.0-0.tar.bz2"
                    .parse()
                    .unwrap()
            ),
            Some("win-64")
        );
        assert_eq!(
            derive_subdir_from_url(
                &"https://conda.anaconda.org/conda-forge/noarch/package-1.0.0-0.tar.bz2"
                    .parse()
                    .unwrap()
            ),
            Some("noarch")
        );
    }

    #[test]
    fn test_channel_from_url() {
        assert_eq!(derive_channel_from_url(&Url::parse("https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda").unwrap()), Some(Url::parse("https://conda.anaconda.org/conda-forge").unwrap().into()));
        assert_eq!(
            derive_channel_from_url(
                &Url::parse(
                    "file:///C:/Users/someone/AppData/Local/Temp/.tmpsasJ7b/noarch/foo-1-0.conda"
                )
                .unwrap()
            ),
            Some(
                Url::parse("file:///C:/Users/someone/AppData/Local/Temp/.tmpsasJ7b")
                    .unwrap()
                    .into()
            )
        );
        assert_eq!(
            derive_channel_from_url(
                &Url::parse("https://repo.anaconda.com/pkgs/main/linux-64/package-1.0.0-0.tar.bz2")
                    .unwrap()
            ),
            Some(
                Url::parse("https://repo.anaconda.com/pkgs/main")
                    .unwrap()
                    .into()
            )
        );
        assert_eq!(
            derive_channel_from_url(
                &Url::parse("https://repo.anaconda.com/pkgs/free/noarch/package-1.0.0-0.tar.bz2")
                    .unwrap()
            ),
            Some(
                Url::parse("https://repo.anaconda.com/pkgs/free")
                    .unwrap()
                    .into()
            )
        );
    }

    #[test]
    fn test_derive_noarch_type() {
        assert_eq!(
            derive_noarch_type("noarch", "py313hb6a6212_1"),
            NoArchType::python()
        );
        assert_eq!(
            derive_noarch_type("noarch", "313hb6a6212_1"),
            NoArchType::generic()
        );
        assert_eq!(
            derive_noarch_type("linux-aarch64", "313hb6a6212_1"),
            NoArchType::none()
        );
        assert_eq!(
            derive_noarch_type("noarch", "generic_0"),
            NoArchType::generic()
        );
        assert_eq!(derive_noarch_type("win-64", "py_0"), NoArchType::none());
    }

    #[test]
    fn test_derive_platform_and_arch() {
        assert_eq!(
            derive_arch_and_platform("linux-aarch64"),
            (Some("aarch64".to_string()), Some("linux".to_string()))
        );
        assert_eq!(
            derive_arch_and_platform("osx-arm64"),
            (Some("arm64".to_string()), Some("osx".to_string()))
        );
        assert_eq!(
            derive_arch_and_platform("win-64"),
            (Some("x86_64".to_string()), Some("win".to_string()))
        );
        assert_eq!(derive_arch_and_platform("noarch"), (None, None));
        assert_eq!(derive_arch_and_platform("unknown"), (None, None));
        assert_eq!(derive_arch_and_platform("win-64-2"), (None, None));
        assert_eq!(
            derive_arch_and_platform("emscripten-wasm32"),
            (Some("wasm32".to_string()), Some("emscripten".to_string()))
        );
        assert_eq!(
            derive_arch_and_platform("wasi-wasm32"),
            (Some("wasm32".to_string()), Some("wasi".to_string()))
        );
    }

    #[rstest]
    #[case(
        1,
        "https://conda.anaconda.org/conda-forge/linux-aarch64/qt-5.15.8-h803f02a_0.conda"
    )]
    #[case(
        2,
        "https://repo.anaconda.com/pkgs/main/linux-64/package-1.0.0-0.tar.bz2"
    )]
    #[case(
        3,
        "https://conda.anaconda.org/conda-forge/osx-arm64/x264-1!164.3095-h57fd34a_2.tar.bz2"
    )]
    #[case(4, "file:///packages/win-64/x264-1!164.3095-h57fd34a_2.tar.bz2")]
    #[case(5, "../source/win-64/package-1.0.0-0.tar.bz2")]
    #[case(6, "../source/win-64")]
    #[case(7, "../source/win-64/")]
    fn test_location_derived_fields(#[case] idx: usize, #[case] location_str: &str) {
        let location = UrlOrPath::from_str(location_str).unwrap();
        let fields = LocationDerivedFields::new(&location);
        insta::assert_debug_snapshot!(
            format!("location_derived_fields-{idx}"),
            fields,
            location_str
        );
    }
}
