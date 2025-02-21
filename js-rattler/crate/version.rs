use crate::JsResult;
use rattler_conda_types::{Version, VersionBumpType};
use std::{cmp::Ordering, str::FromStr};
use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen]
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

#[wasm_bindgen]
pub struct MajorMinor(pub usize, pub usize);

#[wasm_bindgen]
impl JsVersion {
    #[wasm_bindgen(constructor)]
    pub fn new(version: &str) -> JsResult<Self> {
        let version = Version::from_str(version)?;
        Ok(version.into())
    }

    pub fn as_str(&self) -> String {
        format!("{}", self.as_ref())
    }

    #[wasm_bindgen(getter)]
    pub fn epoch(&self) -> Option<usize> {
        self.as_ref().epoch_opt().map(|v| v as usize)
    }

    #[wasm_bindgen(getter)]
    pub fn has_local(&self) -> bool {
        self.as_ref().has_local()
    }

    #[wasm_bindgen(getter)]
    pub fn is_dev(&self) -> bool {
        self.as_ref().is_dev()
    }

    pub fn as_major_minor(&self) -> Option<MajorMinor> {
        let (major, minor) = self.as_ref().as_major_minor()?;
        Some(MajorMinor(major as _, minor as _))
    }

    pub fn starts_with(&self, other: &Self) -> bool {
        self.as_ref().starts_with(other.as_ref())
    }

    pub fn compatible_with(&self, other: &Self) -> bool {
        self.as_ref().compatible_with(other.as_ref())
    }

    pub fn pop_segments(&self, n: usize) -> Option<Self> {
        Some(self.as_ref().pop_segments(n)?.into())
    }

    pub fn extend_to_length(&self, length: usize) -> JsResult<Self> {
        Ok(self.as_ref().extend_to_length(length)?.into_owned().into())
    }

    pub fn with_segments(&self, start: usize, stop: usize) -> Option<Self> {
        let range = start..stop;
        Some(self.as_ref().with_segments(range)?.into())
    }

    #[wasm_bindgen(getter)]
    pub fn length(&self) -> usize {
        self.as_ref().segment_count()
    }

    /// Create a new version with local segment stripped.
    pub fn strip_local(&self) -> Self {
        self.as_ref().strip_local().into_owned().into()
    }

    /// Returns a new version where the major segment of this version has been bumped.
    pub fn bump_major(&self) -> JsResult<Self> {
        Ok(self.as_ref().bump(VersionBumpType::Major).map(Into::into)?)
    }

    /// Returns a new version where the minor segment of this version has been bumped.
    pub fn bump_minor(&self) -> JsResult<Self> {
        Ok(self.as_ref().bump(VersionBumpType::Minor).map(Into::into)?)
    }

    /// Returns a new version where the patch segment of this version has been bumped.
    pub fn bump_patch(&self) -> JsResult<Self> {
        Ok(self.as_ref().bump(VersionBumpType::Patch).map(Into::into)?)
    }

    /// Returns a new version where the last segment of this version has been bumped.
    pub fn bump_last(&self) -> JsResult<Self> {
        Ok(self.as_ref().bump(VersionBumpType::Last).map(Into::into)?)
    }

    /// Returns a new version where the given segment of this version has been bumped.
    pub fn bump_segment(&self, index: i32) -> JsResult<Self> {
        Ok(self
            .as_ref()
            .bump(VersionBumpType::Segment(index))
            .map(Into::into)?)
    }

    /// Returns a new version where the last segment is an "alpha" segment (ie. `.0a0`)
    pub fn with_alpha(&self) -> Self {
        self.as_ref().with_alpha().into_owned().into()
    }

    pub fn equals(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }

    pub fn compare(&self, other: &Self) -> i8 {
        match self.as_ref().cmp(other.as_ref()) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }
}
