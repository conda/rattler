use thiserror::Error;

/// `VersionBumpType` is used to specify the type of bump to perform on a version.
#[derive(Clone)]
pub enum VersionBumpType {
    /// Bump the major version number.
    Major,
    /// Bump the minor version number.
    Minor,
    /// Bump the patch version number.
    Patch,
    /// Bump the last version number.
    Last,
    /// Bump a given segment. If negative, count from the end.
    Segment(i32),
}

/// `VersionBumpError` is used to specify the type of error that occurred when bumping a version.
#[derive(Error, Debug, PartialEq)]
pub enum VersionBumpError {
    /// Cannot bump the major segment of a version with less than 1 segment.
    #[error("cannot bump the major segment of a version with less than 1 segment")]
    NoMajorSegment,
    /// Cannot bump the minor segment of a version with less than 2 segments.
    #[error("cannot bump the minor segment of a version with less than 2 segments")]
    NoMinorSegment,
    /// Cannot bump the patch segment of a version with less than 3 segments.
    #[error("cannot bump the patch segment of a version with less than 3 segments")]
    NoPatchSegment,
    /// Cannot bump the last segment of a version with no segments.
    #[error("cannot bump the last segment of a version with no segments")]
    NoLastSegment,
    /// Invalid segment index.
    #[error("cannot bump the segment '{index:?}' of a version if it's not present")]
    InvalidSegment {
        /// The segment index that was attempted to be bumped.
        index: i32,
    },
}
