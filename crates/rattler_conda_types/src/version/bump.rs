use thiserror::Error;

use crate::{Component, Version};

use super::{segment::Segment, ComponentVec, SegmentVec};

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
    /// Invalid segment index.
    #[error("cannot bump the segment '{index:?}' of a version if it's not present")]
    InvalidSegment {
        /// The segment index that was attempted to be bumped.
        index: i32,
    },

    /// Could not extend the version
    #[error("could not extend the version: {0}")]
    VersionExtendError(#[from] crate::VersionExtendError),
}

impl Version {
    /// Bumps the version according to the specified bump type and returns the new version.
    /// This function will add an alpha specifier to the version (e.g. `1.0.0` will bump to `1.0.1.0a0`) to exclude
    /// any alpha releases.
    pub fn bump_with_alpha(&self, bump_type: VersionBumpType) -> Result<Self, VersionBumpError> {
        self.bump_impl(bump_type, true)
    }

    /// Bumps the version according to the specified bump type and returns the new version.
    pub fn bump(&self, bump_type: VersionBumpType) -> Result<Self, VersionBumpError> {
        self.bump_impl(bump_type, false)
    }

    /// Returns a new version after bumping it according to the specified bump type.
    /// Note: if a version ends with a character, the next bigger version will use `a` as the character.
    /// For example: `1.1l` -> `1.2a`, but also `1.1.0alpha` -> `1.1.1a`.
    fn bump_impl(&self, bump_type: VersionBumpType, with_alpha: bool) -> Result<Self, VersionBumpError> {
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

            // Determine whether this is the segment that needs to be bumped.
            let is_segment_to_bump = segment_to_bump == idx;

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

            if with_alpha {
                
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
    use std::str::FromStr;

    use rstest::rstest;

    use crate::{Version, VersionBumpType};

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

    #[rstest]
    #[case("1.1.9", "1.1.10")]
    #[case("2.1l.5alpha", "2.1l.6a")]
    #[case("5!1.8.alpha+3.4", "5!1.8.1a+3.4")]
    #[case("1", "1.0.1")]
    #[case("1alpha", "1alpha.0.1")]
    #[case("5!1+3.4", "5!1.0.1+3.4")]
    #[case("2.1", "2.1.1")]
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
}
