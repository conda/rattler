//! A virtual package together with what produced it.

use rattler_digest::{Sha256Hash, serde::SerializableHash};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::{ChannelUrl, GenericVirtualPackage, PackageName};

/// A virtual package and the source that produced it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourcedVirtualPackage {
    /// What produced this.
    pub source: VirtualPackageSource,

    /// The virtual package itself, as handed to the solver.
    pub package: GenericVirtualPackage,
}

/// Where a virtual package came from.
#[serde_as]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VirtualPackageSource {
    /// Detected by this client itself, according to CEP 30.
    BuiltIn,

    /// Detected by a plugin a channel registered.
    Plugin {
        /// The channel that registered the plugin.
        channel: ChannelUrl,

        /// The package providing the plugin.
        plugin: PackageName,

        /// Identifies what actually ran: a hash over the plugin package *and*
        /// every package installed alongside it, so it changes when any
        /// dependency of the plugin does. That closure, rather than the package
        /// the registration names, is what a user extends trust to.
        #[serde_as(as = "SerializableHash::<rattler_digest::Sha256>")]
        environment: Sha256Hash,
    },

    /// Taken from a `CONDA_OVERRIDE_*` variable instead of being detected.
    Overridden {
        /// The channel whose plugin would have answered.
        channel: ChannelUrl,

        /// The plugin that would have been run.
        plugin: PackageName,
    },
}

impl VirtualPackageSource {
    /// Whether this came from the client rather than from a channel.
    pub fn is_built_in(&self) -> bool {
        match self {
            Self::BuiltIn => true,
            Self::Plugin { .. } | Self::Overridden { .. } => false,
        }
    }
}
