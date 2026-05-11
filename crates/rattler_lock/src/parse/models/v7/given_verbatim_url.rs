use std::{
    fmt::{self, Debug, Display, Formatter},
    path::Path,
};

use pep508_rs::{Pep508Url, Requirement, VerbatimUrl, VerbatimUrlError, VersionOrUrl};

/// A [`VerbatimUrl`] newtype whose [`Display`] preserves the original (relative)
/// string for file URLs, falling back to the absolute URL otherwise.
///
/// This is used so we can reuse pep508_rs's own [`Display`] impl for
/// [`Requirement`] when serializing the lockfile, while still keeping
/// relative path dependencies intact.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(super) struct GivenVerbatimUrl(VerbatimUrl);

impl Display for GivenVerbatimUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match (self.0.scheme(), self.0.given()) {
            ("file", Some(given)) => f.write_str(given),
            _ => Display::fmt(&self.0, f),
        }
    }
}

impl Pep508Url for GivenVerbatimUrl {
    type Err = VerbatimUrlError;

    fn parse_url(url: &str, working_dir: Option<&Path>) -> Result<Self, Self::Err> {
        <VerbatimUrl as Pep508Url>::parse_url(url, working_dir).map(Self)
    }
}

impl GivenVerbatimUrl {
    /// Rewrap a [`Requirement<VerbatimUrl>`] as a [`Requirement<GivenVerbatimUrl>`]
    /// so it formats through pep508_rs's [`Display`] impl while preserving
    /// relative paths.
    ///
    /// Destructured intentionally: if pep508_rs adds a new field to
    /// [`Requirement`], this stops compiling so we can decide how to handle it.
    pub(super) fn wrap_requirement(req: &Requirement<VerbatimUrl>) -> Requirement<Self> {
        let Requirement {
            name,
            extras,
            version_or_url,
            marker,
            origin,
        } = req;
        Requirement {
            name: name.clone(),
            extras: extras.clone(),
            version_or_url: version_or_url.as_ref().map(|v| match v {
                VersionOrUrl::VersionSpecifier(s) => VersionOrUrl::VersionSpecifier(s.clone()),
                VersionOrUrl::Url(u) => VersionOrUrl::Url(Self(u.clone())),
            }),
            marker: marker.clone(),
            origin: origin.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pep508_rs::{Requirement, VerbatimUrl};

    use super::GivenVerbatimUrl;

    fn base_dir() -> PathBuf {
        // Use an absolute base dir; on Windows the parser still needs an
        // absolute working directory for relative-path resolution.
        if cfg!(windows) {
            PathBuf::from("C:\\base")
        } else {
            PathBuf::from("/base")
        }
    }

    fn fmt(input: &str) -> String {
        let req = Requirement::<VerbatimUrl>::parse(input, base_dir()).unwrap();
        GivenVerbatimUrl::wrap_requirement(&req).to_string()
    }

    #[test]
    fn relative_file_path_is_preserved() {
        // The absolute form would contain "/base/my-pkg"; the relative form
        // ("./my-pkg") must survive serialization.
        let out = fmt("foo @ ./my-pkg");
        assert!(out.contains("./my-pkg"), "got: {out}");
        assert!(!out.contains("/base/"), "got: {out}");
        assert_eq!(out, "foo @ ./my-pkg");
    }

    #[test]
    fn parent_relative_file_path_is_preserved() {
        let out = fmt("foo @ ../sibling");
        assert_eq!(out, "foo @ ../sibling");
    }

    #[test]
    fn explicit_file_scheme_url_passes_through() {
        // file:// URLs are already absolute; we just want the same string back.
        let input = if cfg!(windows) {
            "foo @ file:///C:/abs/pkg"
        } else {
            "foo @ file:///abs/pkg"
        };
        let out = fmt(input);
        assert_eq!(out, input);
    }

    #[test]
    fn https_url_uses_normalized_url() {
        // For non-file schemes we defer to the URL's own Display impl.
        let out = fmt("foo @ https://example.com/pkg-1.0.whl");
        assert_eq!(out, "foo @ https://example.com/pkg-1.0.whl");
    }

    #[test]
    fn version_specifier_unchanged() {
        // No URL involved; should match upstream pep508_rs Display verbatim.
        let out = fmt("foo>=1.0,<2.0");
        assert_eq!(out, "foo>=1.0,<2.0");
    }

    #[test]
    fn bare_name_unchanged() {
        let out = fmt("foo");
        assert_eq!(out, "foo");
    }

    #[test]
    fn extras_and_marker_are_preserved_with_relative_path() {
        // pep508_rs normalizes `python_version > "3.8"` to
        // `python_full_version >= '3.9'`; what matters here is that the
        // relative path survives alongside extras and a marker.
        let out = fmt("foo[bar,baz] @ ./pkg ; python_version > \"3.8\"");
        assert!(out.starts_with("foo[bar,baz] @ ./pkg ; "), "got: {out}");
        assert!(out.contains("python_full_version"), "got: {out}");
    }

    #[test]
    fn marker_only_unchanged() {
        let out = fmt("foo ; python_version > \"3.8\"");
        assert!(out.starts_with("foo ; "), "got: {out}");
        assert!(out.contains("python_full_version"), "got: {out}");
    }

    #[test]
    fn absolute_file_url_without_given_uses_url_display() {
        // Construct a VerbatimUrl that has no `given` string and verify we
        // fall through to the standard URL Display.
        let path = if cfg!(windows) {
            PathBuf::from("C:\\abs\\pkg")
        } else {
            PathBuf::from("/abs/pkg")
        };
        let url = VerbatimUrl::from_absolute_path(&path).unwrap();
        assert!(url.given().is_none());

        let req = Requirement::<VerbatimUrl> {
            name: "foo".parse().unwrap(),
            extras: vec![],
            version_or_url: Some(pep508_rs::VersionOrUrl::Url(url.clone())),
            marker: pep508_rs::MarkerTree::TRUE,
            origin: None,
        };
        let out = GivenVerbatimUrl::wrap_requirement(&req).to_string();
        // Should be the file:// URL (whatever VerbatimUrl::Display produces),
        // *not* an empty string.
        assert!(out.starts_with("foo @ file://"), "got: {out}");
    }
}
