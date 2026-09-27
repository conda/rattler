use std::borrow::Cow;

use thiserror::Error;

use super::{Component, ComponentVec, SegmentVec, Version, VersionExtendError, segment::Segment};

/// Selects which release segment [`Version::bump`] changes.
#[derive(Clone)]
pub enum VersionBumpType {
    /// Increments the first release segment and resets minor and patch segments.
    Major,
    /// Increments the second release segment and resets the patch segment.
    Minor,
    /// Increments the third release segment.
    Patch,
    /// Increments the final release segment.
    Last,
    /// Increments a selected release segment; negative indexes count from the end.
    Segment(i32),
}

/// Explains why [`Version::bump`] could not produce a new [`Version`].
#[derive(Error, Debug, PartialEq)]
pub enum VersionBumpError {
    /// The requested release-segment index does not exist.
    #[error("cannot bump the segment '{index:?}' of a version if it's not present")]
    InvalidSegment {
        /// Index supplied to [`VersionBumpType::Segment`].
        index: i32,
    },

    /// Extending a short version to the requested segment failed.
    #[error("could not extend the version: {0}")]
    VersionExtendError(#[from] VersionExtendError),
}

impl Version {
    /// Adds an alpha suffix to a release [`Version`] that does not already end in text.
    ///
    /// `1.0.0` becomes `1.0.0.0a0`, while `1.0.0a` is borrowed unchanged.
    ///
    /// ```
    /// # use rattler_conda_version::Version;
    /// # use std::str::FromStr;
    /// let version = Version::from_str("1.0.0").unwrap();
    /// let alpha = version.with_alpha().into_owned();
    ///
    /// assert_eq!(alpha, Version::from_str("1.0.0.0a0").unwrap());
    /// ```
    pub fn with_alpha(&self) -> Cow<'_, Self> {
        let last_segment = self.segments().last().expect("at least one segment");
        // check if there is an iden component in the last segment
        let has_iden = last_segment.components().any(|c| c.as_iden().is_some());
        if has_iden {
            return Cow::Borrowed(self);
        }
        let local_segment_index = self.local_segment_index().unwrap_or(self.segments.len());
        let mut segments = self.segments[0..local_segment_index].to_vec();
        let components_offset = segments.iter().map(|s| s.len() as usize).sum::<usize>()
            + usize::from(self.has_epoch());

        segments.push(Segment::new(3).unwrap().with_separator(Some('.')).unwrap());
        segments.extend(self.segments[local_segment_index..].iter());

        let mut components = self.components.clone();
        components.insert(components_offset, Component::Numeral(0));
        components.insert(components_offset + 1, Component::Iden("a".into()));
        components.insert(components_offset + 2, Component::Numeral(0));

        let flags = if let Some(local_segment_index) = self.local_segment_index() {
            self.flags
                .with_local_segment_index((local_segment_index + 1) as u8)
                .unwrap()
        } else {
            self.flags
        };

        Cow::Owned(Version {
            components,
            segments: segments.into(),
            flags,
        })
    }

    /// Removes the local part from this [`Version`], borrowing when none is present.
    ///
    /// ```
    /// # use rattler_conda_version::Version;
    /// # use std::str::FromStr;
    /// let version = Version::from_str("1.0.0+3.4").unwrap();
    /// let without_local = version.remove_local().into_owned();
    ///
    /// assert_eq!(without_local, Version::from_str("1.0.0").unwrap());
    /// ```
    pub fn remove_local(&self) -> Cow<'_, Self> {
        if let Some(local_segment_index) = self.local_segment_index() {
            let segments = self.segments[0..local_segment_index].to_vec();
            let components_offset = segments.iter().map(|s| s.len() as usize).sum::<usize>()
                + usize::from(self.has_epoch());
            let mut components = self.components.clone();
            components.drain(components_offset..);

            Cow::Owned(Version {
                components,
                segments: segments.into(),
                flags: self.flags.with_local_segment_index(0).unwrap(),
            })
        } else {
            Cow::Borrowed(self)
        }
    }

    /// Produces the next [`Version`] by changing the release segment selected by `bump_type`.
    ///
    /// Major and minor bumps reset the following SemVer-style segments. A
    /// bumped textual suffix becomes `a`; for example, `1.1l` becomes `1.2a`.
    ///
    /// ```
    /// # use rattler_conda_version::{Version, version::VersionBumpType};
    /// # use std::str::FromStr;
    /// let version = Version::from_str("1.2.3").unwrap();
    /// let bumped = version.bump(VersionBumpType::Minor).unwrap();
    /// assert_eq!(bumped.to_string(), "1.3.0");
    /// ```
    pub fn bump(&self, bump_type: VersionBumpType) -> Result<Self, VersionBumpError> {
        // Sanity check whether the version has enough segments for this bump type.
        let segment_count = self.segment_count();
        let segment_to_bump = match bump_type {
            VersionBumpType::Major => 0,
            VersionBumpType::Minor => 1,
            VersionBumpType::Patch => 2,
            VersionBumpType::Last => {
                if segment_count > 0 {
                    segment_count - 1
                } else {
                    0
                }
            }
            VersionBumpType::Segment(index_to_bump) => {
                let computed_index = if index_to_bump < 0 {
                    index_to_bump + segment_count as i32
                } else {
                    index_to_bump
                };
                if computed_index < 0 {
                    return Err(VersionBumpError::InvalidSegment {
                        index: index_to_bump,
                    });
                }
                computed_index as usize
            }
        };

        // Add the necessary segments to the version if it's too short.
        let version = self.extend_to_length(segment_to_bump + 1)?;

        let mut components = ComponentVec::new();
        let mut segments = SegmentVec::new();
        let mut flags = version.flags;

        // Copy the optional epoch.
        if let Some(epoch) = version.epoch_opt() {
            components.push(Component::Numeral(epoch));
            flags = flags.with_has_epoch(true);
        }

        // Copy over all the segments and bump the last segment.
        for (idx, segment_iter) in version.segments().enumerate() {
            let segment = segment_iter.segment;
            let mut segment_components =
                segment_iter.components().cloned().collect::<ComponentVec>();

            // Handle semantic versioning resets
            match bump_type {
                VersionBumpType::Major => {
                    if idx == 0 {
                        // Increment major version
                        if let Some(num) = segment_components
                            .iter_mut()
                            .find_map(Component::as_number_mut)
                        {
                            *num += 1;
                        }
                        // Change identifier to 'a' if present
                        if let Some(iden) = segment_components
                            .iter_mut()
                            .filter_map(Component::as_iden_mut)
                            .next_back()
                        {
                            *iden = "a".into();
                        }
                    } else if idx <= 2 {
                        // Reset minor and patch to 0
                        if let Some(num) = segment_components
                            .iter_mut()
                            .find_map(Component::as_number_mut)
                        {
                            *num = 0;
                        }
                    }
                }
                VersionBumpType::Minor => {
                    if idx == 1 {
                        // Increment minor version
                        if let Some(num) = segment_components
                            .iter_mut()
                            .find_map(Component::as_number_mut)
                        {
                            *num += 1;
                        }
                    } else if idx == 2 {
                        // Reset patch to 0
                        if let Some(num) = segment_components
                            .iter_mut()
                            .find_map(Component::as_number_mut)
                        {
                            *num = 0;
                        }
                    }
                }
                VersionBumpType::Patch => {
                    if idx == 2 {
                        // Increment patch version
                        if let Some(num) = segment_components
                            .iter_mut()
                            .find_map(Component::as_number_mut)
                        {
                            *num += 1;
                        }
                        // Change identifier to 'a' if present
                        if let Some(iden) = segment_components
                            .iter_mut()
                            .filter_map(Component::as_iden_mut)
                            .next_back()
                        {
                            *iden = "a".into();
                        }
                    }
                }
                _ => {
                    // For Last and Segment types, keep original behavior
                    if segment_to_bump == idx {
                        let last_numeral_component = segment_components
                            .iter_mut()
                            .filter_map(Component::as_number_mut)
                            .next_back()
                            .expect(
                                "every segment must at least contain a single numeric component",
                            );
                        *last_numeral_component += 1;

                        // If the segment ends with an ascii character, make it `a` instead of whatever it says
                        let last_iden_component = segment_components
                            .iter_mut()
                            .filter_map(Component::as_iden_mut)
                            .next_back();

                        if let Some(last_iden_component) = last_iden_component {
                            *last_iden_component = "a".into();
                        }
                    }
                }
            }

            let has_implicit_default =
                segment.has_implicit_default() && segment_components[0] == Component::default();
            let start_idx = usize::from(has_implicit_default);

            let component_count = segment_components.len();
            for component in segment_components.into_iter().skip(start_idx) {
                components.push(component);
            }

            let segment = Segment::new((component_count - start_idx) as _)
                .expect("there will be no more components than in the previous segment")
                .with_implicit_default(has_implicit_default)
                .with_separator(segment.separator())
                .expect("copying the segment should just work");

            segments.push(segment);
        }

        if version.has_local() {
            let segment_idx = segments.len() as u8;
            for segment_iter in self.local_segments() {
                for component in segment_iter.components().cloned() {
                    components.push(component);
                }
                segments.push(segment_iter.segment);
            }
            flags = flags
                .with_local_segment_index(segment_idx)
                .expect("this should never fail because no new segments are added");
        }

        Ok(Self {
            components,
            segments,
            flags,
        })
    }
}

#[cfg(test)]
mod test {
    use crate::version::{Version, VersionBumpType};
    use rstest::rstest;
    use std::str::FromStr;

    // Original conda-style tests
    #[rstest]
    #[case("1.1", "1.2")]
    #[case("1.1l", "1.2a")]
    #[case("5!1.alpha+3.4", "5!1.1a+3.4")]
    fn bump_last(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(
            Version::from_str(input)
                .unwrap()
                .bump(VersionBumpType::Last)
                .unwrap(),
            Version::from_str(expected).unwrap()
        );
    }

    #[rstest]
    #[case("1.1", "2.0")]
    #[case("2.1l", "3.0l")]
    #[case("5!1.alpha+3.4", "5!2.alpha+3.4")]
    #[case("1.2.3", "2.0.0")]
    #[case("0.1.0", "1.0.0")]
    #[case("1.2.3-alpha", "2.0.0-alpha")]
    #[case("9d", "10a")]
    fn bump_major(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(
            Version::from_str(input)
                .unwrap()
                .bump(VersionBumpType::Major)
                .unwrap(),
            Version::from_str(expected).unwrap()
        );
    }

    #[rstest]
    #[case("1.1", "1.2")]
    #[case("2.1l", "2.2l")]
    #[case("5!1.alpha+3.4", "5!1.1alpha+3.4")]
    #[case("1.2.3", "1.3.0")]
    #[case("1.9.0", "1.10.0")]
    #[case("0.1.0", "0.2.0")]
    fn bump_minor(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(
            Version::from_str(input)
                .unwrap()
                .bump(VersionBumpType::Minor)
                .unwrap(),
            Version::from_str(expected).unwrap()
        );
    }

    #[rstest]
    #[case("1.1.9", "1.1.10")]
    #[case("2.1l.5alpha", "2.1l.6a")]
    #[case("5!1.8.alpha+3.4", "5!1.8.1a+3.4")]
    #[case("1.2.3", "1.2.4")]
    #[case("1.2.9", "1.2.10")]
    #[case("0.1.0", "0.1.1")]
    #[case("1.1.1e", "1.1.2a")]
    fn bump_patch(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(
            Version::from_str(input)
                .unwrap()
                .bump(VersionBumpType::Patch)
                .unwrap(),
            Version::from_str(expected).unwrap()
        );
    }

    #[rstest]
    #[case(0, "1.1.9", "2.1.9")]
    #[case(1, "1.1.9", "1.2.9")]
    #[case(2, "1.1.9", "1.1.10")]
    #[case(-1, "1.1.9", "1.1.10")]
    #[case(-2, "1.1.9", "1.2.9")]
    #[case(-3, "1.1.9", "2.1.9")]
    #[case(0, "9d", "10a")]
    #[case(5, "1.2.3", "1.2.3.0.0.1")]
    #[case(2, "9d", "9d.0.1")]
    fn bump_segment(#[case] idx: i32, #[case] input: &str, #[case] expected: &str) {
        assert_eq!(
            Version::from_str(input)
                .unwrap()
                .bump(VersionBumpType::Segment(idx))
                .unwrap(),
            Version::from_str(expected).unwrap()
        );
    }

    #[rstest]
    #[case(0, "1.1.9", "2.1.9.0a0")]
    #[case(2, "1.0.0", "1.0.1.0a0")]
    #[case(2, "1.0.0a", "1.0.1a")]
    #[case(2, "1.0.0f", "1.0.1a")]
    #[case(2, "5!1.0.0", "5!1.0.1.0a0")]
    #[case(2, "5!1.0.0+3.4", "5!1.0.1.0a0+3.4")]
    fn with_alpha(#[case] idx: i32, #[case] input: &str, #[case] expected: &str) {
        assert_eq!(
            Version::from_str(input)
                .unwrap()
                .bump(VersionBumpType::Segment(idx))
                .unwrap()
                .with_alpha()
                .into_owned(),
            Version::from_str(expected).unwrap()
        );
    }

    #[rstest]
    #[case("1.1.9", "1.1.9")]
    #[case("1.0.0+3", "1.0.0")]
    #[case("1.0.0+3.4", "1.0.0")]
    #[case("1.0.0+3.4alpha.2.4", "1.0.0")]
    #[case("5!1.0.0+3.4alpha.2.4", "5!1.0.0")]
    fn remove_local(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(
            Version::from_str(input)
                .unwrap()
                .remove_local()
                .into_owned(),
            Version::from_str(expected).unwrap()
        );
    }
}
