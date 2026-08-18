//! Detect virtual packages using detection plugins that a conda channel
//! registers in its repodata.
//!
//! **Experimental.** The design, including the parts not implemented yet, is
//! written up in `docs/virtual-package-plugins.md` next to this crate.
//!
//! A channel registers a plugin package and the virtual packages it speaks for.
//! Detecting them means installing that plugin into an environment of its own,
//! running it, and reading its verdicts back:
//!
//! - [`environment`] installs a plugin into an environment of its own.
//! - [`activation`] works out what that environment looks like once activated.
//! - [`runner`] runs a plugin out of that environment.
//! - [`protocol`] parses what a plugin writes to stdout.
//! - [`contract`] checks those verdicts against what the channel registered.
//! - [`demand`] works out which virtual packages a solve could ask for at all.
//! - [`resolve`] decides which plugin speaks for a virtual package two channels
//!   both claim.
//! - [`assemble`] composes those into the set a solve should be given.
//! - [`factory`] is the common shape for anything that produces virtual
//!   packages, whether this client detected them or a channel's plugin did.
//! - [`detect`] composes all of those with a cache and returns
//!   [`SourcedVirtualPackage`](rattler_conda_types::SourcedVirtualPackage)s.
//!
//! [`detect::detect_virtual_packages`] is the entry point. The rest is public
//! because it is useful on its own to a caller that wants to do part of this
//! itself.

#![deny(missing_docs)]

#[cfg(feature = "experimental-virtual-package-plugins")]
pub mod activation;
#[cfg(feature = "experimental-virtual-package-plugins")]
pub mod assemble;
#[cfg(feature = "experimental-virtual-package-plugins")]
pub mod contract;
#[cfg(feature = "experimental-virtual-package-plugins")]
pub mod demand;
#[cfg(feature = "experimental-virtual-package-plugins")]
pub mod detect;
#[cfg(feature = "experimental-virtual-package-plugins")]
pub mod environment;
#[cfg(feature = "experimental-virtual-package-plugins")]
pub mod factory;
#[cfg(feature = "experimental-virtual-package-plugins")]
pub mod overrides;
#[cfg(feature = "experimental-virtual-package-plugins")]
pub mod protocol;
#[cfg(feature = "experimental-virtual-package-plugins")]
pub mod resolve;
#[cfg(feature = "experimental-virtual-package-plugins")]
pub mod runner;

#[cfg(feature = "experimental-virtual-package-plugins")]
pub use activation::{ActivationError, activated_environment};
#[cfg(feature = "experimental-virtual-package-plugins")]
pub use assemble::{AssembleError, AssembleOptions, virtual_packages_for_solve};
#[cfg(feature = "experimental-virtual-package-plugins")]
pub use contract::{ContractViolation, validate};
#[cfg(feature = "experimental-virtual-package-plugins")]
pub use demand::virtual_packages_mentioned;
#[cfg(feature = "experimental-virtual-package-plugins")]
pub use detect::{
    DetectError, DetectOptions, Detection, DetectionTimings, detect_virtual_packages,
};
#[cfg(feature = "experimental-virtual-package-plugins")]
pub use environment::{
    EnvironmentError, EnvironmentTimings, PluginEnvironment, PluginEnvironmentOptions,
    ensure_plugin_environment, environment_sha256,
};
#[cfg(feature = "experimental-virtual-package-plugins")]
pub use factory::{
    BuiltinVirtualPackages, FactoryError, PluginContext, PluginFailure, PluginVirtualPackages,
    STANDARDIZED_VIRTUAL_PACKAGES, VirtualPackageFactory, combine, resolve_needed,
};
#[cfg(feature = "experimental-virtual-package-plugins")]
pub use overrides::{Overridden, OverrideError, PluginOverrides};
#[cfg(feature = "experimental-virtual-package-plugins")]
pub use protocol::{
    CachePolicy, Detected, PROTOCOL_VERSION, PluginReport, ProtocolError, parse_report,
};
#[cfg(feature = "experimental-virtual-package-plugins")]
pub use resolve::{
    ConflictingClaim, MAX_VIRTUAL_PACKAGES_PER_PLUGIN, ResolvedPlugin, ResolvedPlugins,
    channel_registrations, resolve_registrations,
};
#[cfg(feature = "experimental-virtual-package-plugins")]
pub use runner::{PluginRun, RunOptions, RunTimeout, RunnerError, run_plugin};
