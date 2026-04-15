mod match_spec_map_or_vec;
mod ordered;
mod pep440_map_or_vec;
mod timestamp;
pub(crate) mod url_or_path;

pub(crate) use match_spec_map_or_vec::MatchSpecMapOrVec;
pub(crate) use ordered::Ordered;
pub(crate) use pep440_map_or_vec::Pep440MapOrVec;
pub(crate) use timestamp::Timestamp;

/// Returns true if the given value is the default value for its type.
pub(crate) fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    value == &T::default()
}
