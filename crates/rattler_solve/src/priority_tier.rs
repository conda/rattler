//! The unit that the solver backends apply channel priority to.

/// The unit that channel priority is applied to: either all channels of a
/// multichannel, or a single channel.
///
/// See [`crate::ChannelRepoData`] for how the channels of a multichannel
/// share a tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PriorityTier<'a> {
    MultiChannel(&'a str),
    Channel(Option<&'a str>),
}

impl<'a> PriorityTier<'a> {
    /// Returns the tier of a record from `channel` that was requested through
    /// `multi_channel`.
    pub(crate) fn new(multi_channel: Option<&'a str>, channel: Option<&'a str>) -> Self {
        match multi_channel {
            Some(name) => PriorityTier::MultiChannel(name),
            None => PriorityTier::Channel(channel),
        }
    }
}
