use thiserror::Error;

use crate::{Component, Version};

use super::{flags::Flags, segment::Segment, ComponentVec, SegmentVec};

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

impl Version {
    /// Returns a new version after bumping it according to the specified bump type.
    /// Note: if a version ends with a character, the next bigger version will use `a` as the character.
    /// For example: `1.1l` -> `1.2a`, but also `1.1.0alpha` -> `1.1.1a`.
    pub fn bump(&self, bump_type: VersionBumpType) -> Result<Self, VersionBumpError> {
        let mut components = ComponentVec::new();
        let mut segments = SegmentVec::new();
        let mut flags = Flags::default();

        // Copy the optional epoch.
        if let Some(epoch) = self.epoch_opt() {
            components.push(Component::Numeral(epoch));
            flags = flags.with_has_epoch(true);
        }

        // Sanity check whether the version has enough segments for this bump type.
        let segment_count = self.segment_count();
        match bump_type {
            VersionBumpType::Major => {
                if segment_count < 1 {
                    return Err(VersionBumpError::NoMajorSegment);
                }
            }
            VersionBumpType::Minor => {
                if segment_count < 2 {
                    return Err(VersionBumpError::NoMinorSegment);
                }
            }
            VersionBumpType::Patch => {
                if segment_count < 3 {
                    return Err(VersionBumpError::NoPatchSegment);
                }
            }
            VersionBumpType::Last => {
                if segment_count == 0 {
                    return Err(VersionBumpError::NoLastSegment);
                }
            }
            VersionBumpType::Segment(index) => {
                let uindex = if index < 0 {
                    segment_count as i32 + index
                } else {
                    index
                };

                if uindex < 0 || uindex >= segment_count as i32 {
                    return Err(VersionBumpError::InvalidSegment { index });
                }
            }
        }

        // Copy over all the segments and bump the last segment.
        let segment_count = self.segment_count();
        for (idx, segment_iter) in self.segments().enumerate() {
            let segment = segment_iter.segment;

            let mut segment_components =
                segment_iter.components().cloned().collect::<ComponentVec>();

            // Determine whether this is the segment that needs to be bumped.
            let is_segment_to_bump = match bump_type {
                VersionBumpType::Major => idx == 0,
                VersionBumpType::Minor => idx == 1,
                VersionBumpType::Patch => idx == 2,
                VersionBumpType::Last => idx == (segment_count - 1),
                VersionBumpType::Segment(mut index_to_bump) => {
                    if index_to_bump < 0 {
                        index_to_bump += segment_count as i32;
                    }

                    idx == index_to_bump as usize
                }
            };

            // Bump the segment if we need to. Each segment must at least start with a number so this should always work.
            if is_segment_to_bump {
                let last_numeral_component = segment_components
                    .iter_mut()
                    .filter_map(Component::as_number_mut)
                    .next_back()
                    .expect("every segment must at least contain a single numeric component");
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

        if self.has_local() {
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
    use std::str::FromStr;

    use rstest::rstest;

    use crate::{Version, VersionBumpError, VersionBumpType};

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
    #[case("1.1", "2.1")]
    #[case("2.1l", "3.1l")]
    #[case("5!1.alpha+3.4", "5!2.alpha+3.4")]
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
    #[case("2.1l", "2.2a")]
    #[case("5!1.alpha+3.4", "5!1.1a+3.4")]
    fn bump_minor(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(
            Version::from_str(input)
                .unwrap()
                .bump(VersionBumpType::Minor)
                .unwrap(),
            Version::from_str(expected).unwrap()
        );
    }

    #[test]
    fn bump_minor_fail() {
        let err = Version::from_str("1")
            .unwrap()
            .bump(VersionBumpType::Minor)
            .unwrap_err();

        assert_eq!(err, VersionBumpError::NoMinorSegment);
    }

    #[rstest]
    #[case("1.1.9", "1.1.10")]
    #[case("2.1l.5alpha", "2.1l.6a")]
    #[case("5!1.8.alpha+3.4", "5!1.8.1a+3.4")]
    fn bump_patch(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(
            Version::from_str(input)
                .unwrap()
                .bump(VersionBumpType::Patch)
                .unwrap(),
            Version::from_str(expected).unwrap()
        );
    }

    #[test]
    fn bump_patch_fail() {
        let err = Version::from_str("1.3")
            .unwrap()
            .bump(VersionBumpType::Patch)
            .unwrap_err();

        assert_eq!(err, VersionBumpError::NoPatchSegment);
    }

    #[rstest]
    #[case(0, "1.1.9", "2.1.9")]
    #[case(1, "1.1.9", "1.2.9")]
    #[case(2, "1.1.9", "1.1.10")]
    #[case(-1, "1.1.9", "1.1.10")]
    #[case(-2, "1.1.9", "1.2.9")]
    #[case(-3, "1.1.9", "2.1.9")]
    #[case(0, "9d", "10a")]
    fn bump_segment(#[case] idx: i32, #[case] input: &str, #[case] expected: &str) {
        assert_eq!(
            Version::from_str(input)
                .unwrap()
                .bump(VersionBumpType::Segment(idx))
                .unwrap(),
            Version::from_str(expected).unwrap()
        );
    }

    #[test]
    fn bump_segment_fail() {
        let err = Version::from_str("1.3")
            .unwrap()
            .bump(VersionBumpType::Segment(3))
            .unwrap_err();

        assert_eq!(err, VersionBumpError::InvalidSegment { index: 3 });

        let err = Version::from_str("1.3")
            .unwrap()
            .bump(VersionBumpType::Segment(-3))
            .unwrap_err();

        assert_eq!(err, VersionBumpError::InvalidSegment { index: -3 });
    }
}
