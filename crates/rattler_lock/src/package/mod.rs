mod conda;
mod pypi;

pub use conda::{CondaPackageData, ConversionError};
pub use pypi::{PyPiRuntimeConfiguration, PypiPackageData};

/// Additional runtime configuration of a package. The locked packages in a lock-file refer to
/// inert package data but sometimes runtime configuration is needed to install the package. For
/// instance Pypi packages can have optional extras that can be enabled or disabled.
#[derive(Clone, Debug)]
pub(crate) enum RuntimePackageData {
    Conda(usize),
    Pypi(usize, PyPiRuntimeConfiguration),
}
