//! Conversions between a conda [`Version`] and a [`semver::Version`].

use std::fmt::Write;

use semver::{BuildMetadata, Prerelease, Version as SemverVersion};
use thiserror::Error;

use super::{Component, ComponentVec, SegmentVec, Version, flags::Flags, segment::Segment};

/// The index of the first local segment is stored in 7 bits, so at most this
/// many segments fit in front of it.
const MAX_LEADING_SEGMENTS: usize = 127;

/// Maximum number of components a single [`Segment`] can hold.
const MAX_COMPONENTS_PER_SEGMENT: usize = (1 << 13) - 1;

/// An error that can occur when converting a conda [`Version`] into a
/// [`semver::Version`].
#[derive(Debug, Error)]
pub enum VersionToSemverError {
    /// The version has an epoch. semver does not know about those.
    #[error("a semver version cannot express the epoch of `{0}!`")]
    EpochNotSupported(u64),

    /// The version has more than three release numbers.
    #[error("a semver version can only hold three release numbers (`major.minor.patch`)")]
    TooManyReleaseSegments,

    /// The version contains a `post` component. In conda `post` sorts after the
    /// release it belongs to, in semver everything behind the `-` sorts before
    /// it. There is no way to keep the ordering.
    #[error("a semver version cannot express a `post` release")]
    PostReleaseNotSupported,

    /// The version contains an underscore. semver does not allow those.
    #[error("a semver version cannot contain an underscore")]
    UnderscoreNotSupported,

    /// The components behind the release numbers do not form a valid semver
    /// pre-release.
    #[error("the version cannot be expressed as a semver pre-release")]
    InvalidPrerelease(#[source] semver::Error),

    /// The local version part does not form valid semver build metadata.
    #[error("the local version cannot be expressed as semver build metadata")]
    InvalidBuildMetadata(#[source] semver::Error),
}

impl From<&SemverVersion> for Version {
    /// Converts a [`semver::Version`] into a conda [`Version`].
    ///
    /// `major`, `minor` and `patch` become the first three segments, every dot
    /// separated pre-release identifier becomes another segment and the build
    /// metadata becomes the local version part. So `1.2.3-rc.1+build.5` turns
    /// into `1.2.3.rc.1+build.5`.
    ///
    /// The pre-release is separated with a `.` instead of the `-` that semver
    /// uses, because conda package filenames use `-` to split the name, version
    /// and build string. Both compare the same in conda, separators do not
    /// affect ordering.
    ///
    /// Ordering is preserved except where conda and semver disagree:
    ///
    /// * semver sorts numeric identifiers below alphanumeric ones
    ///   (`1.0.0-1 < 1.0.0-alpha`), conda does the opposite.
    /// * conda treats `dev` and `post` as special, so `1.0.0-post` ends up
    ///   above `1.0.0` instead of below it.
    ///
    /// Two inputs have no exact representation. A number that does not fit in a
    /// `u64` is kept as text, and the build metadata is dropped if the
    /// pre-release has so many identifiers that there is no room left to encode
    /// the local part. Neither changes the ordering, semver ignores build
    /// metadata when comparing anyway.
    ///
    /// ```
    /// # use std::str::FromStr;
    /// use rattler_conda_types::Version;
    ///
    /// let semver = semver::Version::parse("1.2.3-rc.1+build.5").unwrap();
    /// assert_eq!(Version::from(&semver).to_string(), "1.2.3.rc.1+build.5");
    /// ```
    fn from(value: &SemverVersion) -> Self {
        let mut components = ComponentVec::new();
        let mut segments = SegmentVec::new();

        push_number(&mut components, &mut segments, value.major, None);
        push_number(&mut components, &mut segments, value.minor, Some('.'));
        push_number(&mut components, &mut segments, value.patch, Some('.'));
        push_identifiers(
            &mut components,
            &mut segments,
            value.pre.as_str(),
            Some('.'),
        );

        // Build metadata goes behind the `+`. Its first segment has no
        // separator, the `+` already is one.
        let mut flags = Flags::default();
        let local_segment_index = segments.len();
        if local_segment_index <= MAX_LEADING_SEGMENTS {
            push_identifiers(&mut components, &mut segments, value.build.as_str(), None);

            // Only mark a local part if the build metadata produced segments.
            if segments.len() > local_segment_index {
                flags = u8::try_from(local_segment_index)
                    .ok()
                    .and_then(|index| flags.with_local_segment_index(index))
                    .expect("the index was bounded above");
            }
        }

        Version {
            components,
            segments,
            flags,
        }
    }
}

impl From<SemverVersion> for Version {
    fn from(value: SemverVersion) -> Self {
        Version::from(&value)
    }
}

/// Appends a single numeric segment.
fn push_number(
    components: &mut ComponentVec,
    segments: &mut SegmentVec,
    number: u64,
    separator: Option<char>,
) {
    components.push(Component::Numeral(number));
    segments.push(
        Segment::new(1)
            .expect("one component always fits")
            .with_separator(separator)
            .expect("`.` is a valid separator"),
    );
}

/// Appends the dot separated `identifiers` of a pre-release or of build
/// metadata as segments. `first_separator` goes in front of the first one.
fn push_identifiers(
    components: &mut ComponentVec,
    segments: &mut SegmentVec,
    identifiers: &str,
    first_separator: Option<char>,
) {
    let start = segments.len();
    let mut separator = first_separator;
    for identifier in identifiers.split('.') {
        // Conda splits on `-` too, so `alpha-1` becomes two segments, the same
        // as what the parser does. Empty parts like in `--` have no conda
        // equivalent, skip those.
        for part in identifier.split('-').filter(|part| !part.is_empty()) {
            push_identifier(components, segments, part, separator);
            separator = Some('-');
        }
        if segments.len() > start {
            separator = Some('.');
        }
    }
}

/// Appends a single non-empty semver identifier as one segment.
fn push_identifier(
    components: &mut ComponentVec,
    segments: &mut SegmentVec,
    identifier: &str,
    separator: Option<char>,
) {
    debug_assert!(!identifier.is_empty());

    // Segments must start with a number, so one that starts with a letter gets
    // an implicit `0` in front of it.
    let has_implicit_default = !identifier.starts_with(|c: char| c.is_ascii_digit());

    // An identifier with more runs than fit in one segment is stored as a
    // single component. The text survives, only the ordering is off.
    if identifier.len() > MAX_COMPONENTS_PER_SEGMENT {
        components.push(Component::Iden(
            identifier.to_ascii_lowercase().into_boxed_str(),
        ));
        push_segment(segments, 1, true, separator);
        return;
    }

    // Split into alternating runs of digits and letters, like the parser does.
    let mut component_count = 0u16;
    let mut rest = identifier;
    while let Some(first) = rest.chars().next() {
        let is_digit = first.is_ascii_digit();
        let end = rest
            .find(|c: char| c.is_ascii_digit() != is_digit)
            .unwrap_or(rest.len());
        let (run, remainder) = rest.split_at(end);
        components.push(component_from_run(run, is_digit));
        component_count += 1;
        rest = remainder;
    }

    push_segment(segments, component_count, has_implicit_default, separator);
}

/// Appends a segment covering the last `component_count` components.
fn push_segment(
    segments: &mut SegmentVec,
    component_count: u16,
    has_implicit_default: bool,
    separator: Option<char>,
) {
    segments.push(
        Segment::new(component_count)
            .expect("the component count was bounded above")
            .with_implicit_default(has_implicit_default)
            .with_separator(separator)
            .expect("`.` and `-` are valid separators"),
    );
}

/// Turns a run of digits or letters into a [`Component`], the same way the
/// version parser does.
fn component_from_run(run: &str, is_digit: bool) -> Component {
    if is_digit {
        // Numbers larger than a `u64` are kept as text.
        run.parse()
            .map_or_else(|_| Component::Iden(Box::from(run)), Component::Numeral)
    } else if run.eq_ignore_ascii_case("post") {
        Component::Post
    } else if run.eq_ignore_ascii_case("dev") {
        Component::Dev
    } else if run.bytes().all(|b| b.is_ascii_lowercase()) {
        Component::Iden(Box::from(run))
    } else {
        Component::Iden(run.to_ascii_lowercase().into_boxed_str())
    }
}

impl TryFrom<&Version> for SemverVersion {
    type Error = VersionToSemverError;

    /// Converts a conda [`Version`] into a [`semver::Version`].
    ///
    /// The inverse of the conversion above: the first three segments become
    /// `major`, `minor` and `patch`, everything behind them becomes the
    /// pre-release and the local version part becomes the build metadata.
    /// Missing release numbers default to `0`, so `1.2` becomes `1.2.0`.
    ///
    /// Conda versions can express a lot more than semver versions, so this
    /// fails for anything without a semver equivalent. See
    /// [`VersionToSemverError`] for what gets rejected.
    ///
    /// ```
    /// # use std::str::FromStr;
    /// use rattler_conda_types::Version;
    ///
    /// let version = Version::from_str("1.2.3rc1").unwrap();
    /// let semver = semver::Version::try_from(&version).unwrap();
    /// assert_eq!(semver.to_string(), "1.2.3-rc.1");
    /// ```
    fn try_from(value: &Version) -> Result<Self, Self::Error> {
        if let Some(epoch) = value.epoch_opt().filter(|epoch| *epoch != 0) {
            return Err(VersionToSemverError::EpochNotSupported(epoch));
        }

        let mut release = [0u64; 3];
        let mut release_count = 0;
        let mut pre = String::new();
        let mut in_pre = false;

        for segment in value.segments() {
            let mut components = strip_implicit_default(&segment).peekable();

            if !in_pre {
                match components
                    .peek()
                    .and_then(|component| component.as_number())
                {
                    // The next release number, e.g. the `3` of `1.2.3rc1`.
                    Some(number) if release_count < release.len() => {
                        release[release_count] = number;
                        release_count += 1;
                        components.next();
                    }
                    // A zero behind the patch is padding, e.g. `1.2.3.0rc1`.
                    Some(0) => {
                        components.next();
                    }
                    // Anything else would be a fourth release number.
                    Some(_) => return Err(VersionToSemverError::TooManyReleaseSegments),
                    // The rest of the version is the pre-release.
                    None => {}
                }
                in_pre = components.peek().is_some();
            }

            for component in components {
                push_semver_identifier(&mut pre, component)?;
            }
        }

        let mut build = String::new();
        for segment in value.local_segments() {
            for component in strip_implicit_default(&segment) {
                push_semver_identifier(&mut build, component)?;
            }
        }

        Ok(SemverVersion {
            major: release[0],
            minor: release[1],
            patch: release[2],
            pre: Prerelease::new(&pre).map_err(VersionToSemverError::InvalidPrerelease)?,
            build: BuildMetadata::new(&build)
                .map_err(VersionToSemverError::InvalidBuildMetadata)?,
        })
    }
}

impl TryFrom<Version> for SemverVersion {
    type Error = VersionToSemverError;

    fn try_from(value: Version) -> Result<Self, Self::Error> {
        SemverVersion::try_from(&value)
    }
}

/// Iterates the components of a segment and skips the `0` that conda inserts in
/// front of segments that start with a letter. That zero belongs to the
/// internal representation, not to the version itself.
fn strip_implicit_default<'v>(
    segment: &super::SegmentIter<'v>,
) -> impl Iterator<Item = &'v Component> {
    let mut components = segment.components();
    if segment.has_implicit_default() {
        components.next();
    }
    components
}

/// Appends `component` to `out` as another dot separated identifier.
fn push_semver_identifier(
    out: &mut String,
    component: &Component,
) -> Result<(), VersionToSemverError> {
    if !out.is_empty() {
        out.push('.');
    }
    match component {
        Component::Numeral(number) => {
            write!(out, "{number}").expect("writing to a string never fails");
        }
        Component::Iden(iden) => out.push_str(iden),
        Component::Dev => out.push_str("dev"),
        Component::Post => return Err(VersionToSemverError::PostReleaseNotSupported),
        Component::UnderscoreOrDash { is_dash: true } => out.push('-'),
        Component::UnderscoreOrDash { is_dash: false } => {
            return Err(VersionToSemverError::UnderscoreNotSupported);
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use assert_matches::assert_matches;
    use rstest::rstest;

    use super::{SemverVersion, Version, VersionToSemverError};

    #[rstest]
    #[case("1.2.3", "1.2.3")]
    #[case("0.0.0", "0.0.0")]
    #[case("1.0.0", "1.0.0")]
    #[case("18446744073709551615.0.1", "18446744073709551615.0.1")]
    #[case("1.2.3-rc.1", "1.2.3.rc.1")]
    #[case("1.2.3-alpha1", "1.2.3.alpha1")]
    #[case("1.2.3-0.3.7", "1.2.3.0.3.7")]
    #[case("1.2.3-x.7.z.92", "1.2.3.x.7.z.92")]
    #[case("1.2.3-RC.1", "1.2.3.rc.1")]
    #[case("1.2.3-dev.1", "1.2.3.dev.1")]
    #[case("1.2.3-alpha-1", "1.2.3.alpha-1")]
    #[case("1.2.3+build.5", "1.2.3+build.5")]
    #[case("1.2.3-rc.1+build.5", "1.2.3.rc.1+build.5")]
    #[case("1.2.3+21AF26D3----117B344092BD", "1.2.3+21af26d3-117b344092bd")]
    // Numbers that do not fit in a `u64` are kept as text.
    #[case("1.2.3-99999999999999999999", "1.2.3.99999999999999999999")]
    fn test_from_semver(#[case] input: &str, #[case] expected: &str) {
        let semver = SemverVersion::parse(input).unwrap();
        let version = Version::from(&semver);
        assert_eq!(version.to_string(), expected);

        // Building the version directly should give the same result as parsing
        // the semver string, at least when that string is a valid conda version.
        if let Ok(parsed) = Version::from_str(input) {
            assert_eq!(version, parsed);
        }
    }

    #[rstest]
    #[case("1.2.3", "1.2.3")]
    #[case("1.2", "1.2.0")]
    #[case("1", "1.0.0")]
    #[case("0!1.2.3", "1.2.3")]
    #[case("1.2.3.0", "1.2.3")]
    #[case("1.2.3rc1", "1.2.3-rc.1")]
    #[case("1.2.3.rc1", "1.2.3-rc.1")]
    #[case("1.2.3.0rc1", "1.2.3-rc.1")]
    #[case("1.2.3.rc.1", "1.2.3-rc.1")]
    #[case("1.rc1", "1.0.0-rc.1")]
    #[case("1.2.3.dev1", "1.2.3-dev.1")]
    #[case("1.2.3+build.5", "1.2.3+build.5")]
    #[case("1.2.3.rc1+build.5", "1.2.3-rc.1+build.5")]
    #[case("1.0.1-", "1.0.1--")]
    fn test_to_semver(#[case] input: &str, #[case] expected: &str) {
        let version = Version::from_str(input).unwrap();
        let semver = SemverVersion::try_from(&version).unwrap();
        assert_eq!(semver.to_string(), expected);
    }

    #[rstest]
    #[case("1!2.3.4", VersionToSemverError::EpochNotSupported(1))]
    #[case("1.2.3.4", VersionToSemverError::TooManyReleaseSegments)]
    #[case("1.2.3.4rc1", VersionToSemverError::TooManyReleaseSegments)]
    #[case("1.2.3.post1", VersionToSemverError::PostReleaseNotSupported)]
    #[case("1.2.3_", VersionToSemverError::UnderscoreNotSupported)]
    fn test_to_semver_error(#[case] input: &str, #[case] expected: VersionToSemverError) {
        let version = Version::from_str(input).unwrap();
        let error = SemverVersion::try_from(&version).unwrap_err();
        assert_eq!(
            std::mem::discriminant(&error),
            std::mem::discriminant(&expected),
            "expected `{expected}` but got `{error}`"
        );
    }

    /// Versions that survive a round trip through a conda version unchanged.
    #[rstest]
    #[case("1.2.3")]
    #[case("1.2.3-rc.1")]
    #[case("1.2.3-alpha.1.beta")]
    #[case("1.2.3+build.5")]
    #[case("1.2.3-rc.1+build.5")]
    fn test_round_trip(#[case] input: &str) {
        let semver = SemverVersion::parse(input).unwrap();
        let version = Version::from(&semver);
        assert_eq!(SemverVersion::try_from(&version).unwrap(), semver);
    }

    /// The conversion keeps the ordering of semver versions.
    #[test]
    fn test_ordering_is_preserved() {
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "2.0.0",
        ];

        for pair in ordered.windows(2) {
            let left = Version::from(SemverVersion::parse(pair[0]).unwrap());
            let right = Version::from(SemverVersion::parse(pair[1]).unwrap());
            assert!(left < right, "expected {left} < {right}");
        }
    }

    /// Where conda and semver ordering disagree. There is no way around these.
    #[test]
    fn test_ordering_divergence() {
        // semver orders numeric identifiers below alphanumeric ones, conda
        // orders numbers above strings.
        let numeric = Version::from(SemverVersion::parse("1.0.0-alpha.1").unwrap());
        let alphanumeric = Version::from(SemverVersion::parse("1.0.0-alpha.beta").unwrap());
        assert!(numeric > alphanumeric);

        // `post` is special in conda, so it sorts above the release instead of
        // below it.
        let post = Version::from(SemverVersion::parse("1.0.0-post").unwrap());
        let release = Version::from(SemverVersion::parse("1.0.0").unwrap());
        assert!(post > release);
    }

    /// A version with more identifiers than can be encoded must not panic.
    #[test]
    fn test_pathological_prerelease() {
        let pre = ["a"; 200].join(".");
        let semver = SemverVersion::parse(&format!("1.2.3-{pre}+build")).unwrap();
        let version = Version::from(&semver);

        // No room left for the build metadata, so it is dropped.
        assert!(!version.has_local());
        assert_eq!(version.to_string(), format!("1.2.3.{pre}"));
    }

    #[test]
    fn test_owned_conversions() {
        let semver = SemverVersion::parse("1.2.3-rc.1").unwrap();
        assert_eq!(Version::from(semver.clone()), Version::from(&semver));

        let version = Version::from_str("1.2.3.rc1").unwrap();
        assert_matches!(SemverVersion::try_from(version), Ok(_));
    }
}
