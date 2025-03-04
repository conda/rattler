use rattler_conda_types::{Version, VersionBumpType};
use std::{cmp::Ordering, str::FromStr};
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::JsValue;

use crate::JsResult;

/// This class implements an order relation between version strings. Version
/// strings can contain the usual alphanumeric characters (A-Za-z0-9), separated
/// into segments by dots and underscores. Empty segments (i.e. two consecutive
/// dots, a leading/trailing underscore) are not permitted. An optional epoch
/// number - an integer followed by '!' - can precede the actual version string
/// (this is useful to indicate a change in the versioning scheme itself).
/// Version comparison is case-insensitive.
///
/// @public
#[wasm_bindgen(js_name = "Version")]
#[repr(transparent)]
#[derive(Eq, PartialEq)]
pub struct JsVersion {
    inner: Version,
}

impl From<Version> for JsVersion {
    fn from(value: Version) -> Self {
        JsVersion { inner: value }
    }
}

impl From<JsVersion> for Version {
    fn from(value: JsVersion) -> Self {
        value.inner
    }
}

impl AsRef<Version> for JsVersion {
    fn as_ref(&self) -> &Version {
        &self.inner
    }
}

#[wasm_bindgen(js_class = "Version")]
impl JsVersion {
    /// Constructs a new Version object from a string representation.
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(param_description = "The string representation of the version.")]
        version: &str,
    ) -> JsResult<Self> {
        let version = Version::from_str(version)?;
        Ok(version.into())
    }

    /// Returns the string representation of the version.
    ///
    /// An attempt is made to return the version in the same format as the input
    /// string but this is not guaranteed.
    #[wasm_bindgen(js_name = "toString")]
    pub fn as_str(&self) -> String {
        format!("{}", self.as_ref())
    }

    /// The epoch part of the version. E.g. `1` in `1!2.3`.
    #[wasm_bindgen(getter)]
    pub fn epoch(&self) -> Option<usize> {
        self.as_ref().epoch_opt().map(|v| v as usize)
    }

    /// `true` if the version has a local part. E.g. `2.3` in `1+2.3`.
    #[wasm_bindgen(getter, js_name = "hasLocal")]
    pub fn has_local(&self) -> bool {
        self.as_ref().has_local()
    }

    /// `true` if the version is considered a development version.
    ///
    /// A development version is a version that contains the string `dev` in the
    /// version string.
    #[wasm_bindgen(getter, js_name = "isDev")]
    pub fn is_dev(&self) -> bool {
        self.as_ref().is_dev()
    }

    /// Returns the major and minor part of the version if the version does not
    /// represent a typical major minor version. If any of the parts are not a
    /// single number, undefined is returned.
    // TODO: Simplify when https://github.com/rustwasm/wasm-bindgen/issues/122 is fixed
    #[wasm_bindgen(
        js_name = "asMajorMinor",
        unchecked_return_type = "[number, number] | undefined"
    )]
    pub fn as_major_minor(&self) -> Option<Vec<JsValue>> {
        let (major, minor) = self.as_ref().as_major_minor()?;
        Some(vec![
            JsValue::from(major as usize),
            JsValue::from(minor as usize),
        ])
    }

    /// Returns true if this version starts with the other version. This is
    /// defined as the other version being a prefix of this version.
    #[wasm_bindgen(js_name = "startsWith")]
    pub fn starts_with(
        &self,
        #[wasm_bindgen(param_description = "The version to use for the comparison")] other: &Self,
    ) -> bool {
        self.as_ref().starts_with(other.as_ref())
    }

    /// Returns true if this version is compatible with the other version.
    #[wasm_bindgen(js_name = "compatibleWith")]
    pub fn compatible_with(
        &self,
        #[wasm_bindgen(param_description = "The version to use for the comparison")] other: &Self,
    ) -> bool {
        self.as_ref().compatible_with(other.as_ref())
    }

    /// Pop the last `n` segments from the version.
    #[wasm_bindgen(js_name = "popSegments")]
    pub fn pop_segments(
        &self,
        #[wasm_bindgen(param_description = "The number of segments to pop")] n: usize,
    ) -> Option<Self> {
        Some(self.as_ref().pop_segments(n)?.into())
    }

    /// Extend the version to the given length by adding zeros. If the version
    /// is already at the specified length or longer the original version
    /// will be returned.
    #[wasm_bindgen(js_name = "extendToLength")]
    pub fn extend_to_length(
        &self,
        #[wasm_bindgen(param_description = "The length to extend to")] length: usize,
    ) -> JsResult<Self> {
        Ok(self.as_ref().extend_to_length(length)?.into_owned().into())
    }

    /// Returns a new version with the segments from start to end (exclusive).
    ///
    /// Returns undefined if the start or end index is out of bounds.
    #[wasm_bindgen(js_name = "withSegments")]
    pub fn with_segments(
        &self,
        #[wasm_bindgen(param_description = "The start index")] start: usize,
        #[wasm_bindgen(param_description = "The end index")] end: usize,
    ) -> Option<Self> {
        let range = start..end;
        Some(self.as_ref().with_segments(range)?.into())
    }

    /// The number of segments in the version.
    #[wasm_bindgen(getter)]
    pub fn length(&self) -> usize {
        self.as_ref().segment_count()
    }

    /// Returns the version without the local part. E.g. `1+2.3` becomes `1`.
    #[wasm_bindgen(js_name = "stripLocal")]
    pub fn strip_local(&self) -> Self {
        self.as_ref().strip_local().into_owned().into()
    }

    /// Returns a new version where the major segment of this version has been
    /// bumped.
    #[wasm_bindgen(js_name = "bumpMajor")]
    pub fn bump_major(&self) -> JsResult<Self> {
        Ok(self.as_ref().bump(VersionBumpType::Major).map(Into::into)?)
    }

    /// Returns a new version where the minor segment of this version has been
    /// bumped.
    #[wasm_bindgen(js_name = "bumpMinor")]
    pub fn bump_minor(&self) -> JsResult<Self> {
        Ok(self.as_ref().bump(VersionBumpType::Minor).map(Into::into)?)
    }

    /// Returns a new version where the patch segment of this version has been
    /// bumped.
    #[wasm_bindgen(js_name = "bumpPatch")]
    pub fn bump_patch(&self) -> JsResult<Self> {
        Ok(self.as_ref().bump(VersionBumpType::Patch).map(Into::into)?)
    }

    /// Returns a new version where the last segment of this version has been
    /// bumped.
    #[wasm_bindgen(js_name = "bumpLast")]
    pub fn bump_last(&self) -> JsResult<Self> {
        Ok(self.as_ref().bump(VersionBumpType::Last).map(Into::into)?)
    }

    /// Returns a new version where the given segment of this version has been
    /// bumped.
    #[wasm_bindgen(js_name = "bumpSegment")]
    pub fn bump_segment(
        &self,
        #[wasm_bindgen(param_description = "The index of the segment to bump")] index: i32,
    ) -> JsResult<Self> {
        Ok(self
            .as_ref()
            .bump(VersionBumpType::Segment(index))
            .map(Into::into)?)
    }

    /// Returns a new version where the last segment is an "alpha" segment (ie.
    /// `.0a0`)
    #[wasm_bindgen(js_name = "withAlpha")]
    pub fn with_alpha(&self) -> Self {
        self.as_ref().with_alpha().into_owned().into()
    }

    /// Compares this version with another version. Returns `true` if the
    /// versions are considered equal.
    ///
    /// Note that two version strings can be considered equal even if they are
    /// not exactly the same. For example, `1.0` and `1` are considered equal.
    #[wasm_bindgen(js_name = "equals")]
    pub fn equals(
        &self,
        #[wasm_bindgen(param_description = "The version to compare with")] other: &Self,
    ) -> bool {
        self.as_ref() == other.as_ref()
    }

    /// Compare two versions.
    ///
    /// Returns `-1` if this instance should be ordered before `other`, `0` if
    /// this version and `other` are considered equal, `1` if this version
    /// should be ordered after `other`.
    pub fn compare(
        &self,
        #[wasm_bindgen(param_description = "The version to compare with")] other: &Self,
    ) -> i8 {
        match self.as_ref().cmp(other.as_ref()) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }
}
