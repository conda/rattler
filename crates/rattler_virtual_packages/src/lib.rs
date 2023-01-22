//! A library to detect Conda virtual packages present on a system.

mod cuda;
mod libc;
mod linux;

use once_cell::sync::OnceCell;
use rattler_conda_types::{Platform, Version};
use std::str::FromStr;

pub use libc::DetectLibCError;
pub use linux::ParseLinuxVersionError;

/// A `GenericVirtualPackage` is a Conda package description that contains a `name` and a
/// `version` and a `build_string`. See [`VirtualPackage`] for available virtual packages.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct GenericVirtualPackage {
    pub name: String,
    pub version: Version,
    pub build_string: String,
}

#[derive(Clone, Eq, PartialEq, Hash)]
pub enum VirtualPackage {
    /// Available on windows
    Win,

    /// Available on unix based platforms
    Unix,

    /// Available when running on Linux
    Linux(Linux),

    /// Available LibC family and version
    LibC(LibC),

    /// Available Cuda version
    Cuda(Cuda),

    /// The CPU architecture
    Archspec(Archspec),
}

impl From<VirtualPackage> for GenericVirtualPackage {
    fn from(package: VirtualPackage) -> Self {
        match package {
            VirtualPackage::Win => GenericVirtualPackage {
                name: "__win".into(),
                version: Version::from_str("0").unwrap(),
                build_string: "0".into(),
            },
            VirtualPackage::Unix => GenericVirtualPackage {
                name: "__unix".into(),
                version: Version::from_str("0").unwrap(),
                build_string: "0".into(),
            },
            VirtualPackage::Linux(linux) => linux.into(),
            VirtualPackage::LibC(libc) => libc.into(),
            VirtualPackage::Cuda(cuda) => cuda.into(),
            VirtualPackage::Archspec(spec) => spec.into(),
        }
    }
}

impl VirtualPackage {
    pub fn current() -> Result<&'static [Self], DetectVirtualPackageError> {
        static DETECED_VIRTUAL_PACKAGES: OnceCell<Vec<VirtualPackage>> = OnceCell::new();
        DETECED_VIRTUAL_PACKAGES
            .get_or_try_init(try_detect_virtual_packages)
            .map(Vec::as_slice)
    }
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum DetectVirtualPackageError {
    #[error(transparent)]
    ParseLinuxVersion(#[from] ParseLinuxVersionError),

    #[error(transparent)]
    DetectLibC(#[from] DetectLibCError),
}

// Detect the available virtual packages on the system
fn try_detect_virtual_packages() -> Result<Vec<VirtualPackage>, DetectVirtualPackageError> {
    let mut result = Vec::new();
    let platform = Platform::current();

    if platform.is_unix() {
        result.push(VirtualPackage::Unix);
    }

    if platform.is_windows() {
        result.push(VirtualPackage::Win);
    }

    if platform.is_linux() {
        if let Some(linux_version) = Linux::current()? {
            result.push(linux_version.into())
        }
        if let Some(libc) = LibC::current()? {
            result.push(libc.into())
        }
    }

    if let Some(cuda) = Cuda::current() {
        result.push(cuda.into())
    }

    if let Some(archspec) = Archspec::from_platform(platform) {
        result.push(archspec.into())
    }

    Ok(result)
}

/// Linux virtual package description
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct Linux {
    pub version: Version,
}

impl Linux {
    /// Returns the Linux version of the current platform.
    ///
    /// Returns an error if determining the Linux version resulted in an error. Returns `None` if
    /// the current platform is not a Linux based platform.
    pub fn current() -> Result<Option<Self>, ParseLinuxVersionError> {
        Ok(linux::linux_version()?.map(|version| Self { version }))
    }
}

impl From<Linux> for GenericVirtualPackage {
    fn from(linux: Linux) -> Self {
        GenericVirtualPackage {
            name: "__linux".into(),
            version: linux.version,
            build_string: "0".into(),
        }
    }
}

impl From<Linux> for VirtualPackage {
    fn from(linux: Linux) -> Self {
        VirtualPackage::Linux(linux)
    }
}

/// LibC virtual package description
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct LibC {
    pub family: String,
    pub version: Version,
}

impl LibC {
    /// Returns the LibC family and version of the current platform.
    ///
    /// Returns an error if determining the LibC family and version resulted in an error. Returns
    /// `None` if the current platform does not have an available version of LibC.
    pub fn current() -> Result<Option<Self>, DetectLibCError> {
        Ok(libc::libc_family_and_version()?.map(|(family, version)| Self { family, version }))
    }
}

impl From<LibC> for GenericVirtualPackage {
    fn from(libc: LibC) -> Self {
        GenericVirtualPackage {
            name: format!("__{}", libc.family.to_lowercase()),
            version: libc.version,
            build_string: "0".into(),
        }
    }
}

impl From<LibC> for VirtualPackage {
    fn from(libc: LibC) -> Self {
        VirtualPackage::LibC(libc)
    }
}

/// Cuda virtual package description
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct Cuda {
    pub version: Version,
}

impl Cuda {
    /// Returns the maximum Cuda version available on the current platform.
    pub fn current() -> Option<Self> {
        cuda::cuda_version().map(|version| Self { version })
    }
}

impl From<Cuda> for GenericVirtualPackage {
    fn from(cuda: Cuda) -> Self {
        GenericVirtualPackage {
            name: "__cuda".into(),
            version: cuda.version,
            build_string: "0".into(),
        }
    }
}

impl From<Cuda> for VirtualPackage {
    fn from(cuda: Cuda) -> Self {
        VirtualPackage::Cuda(cuda)
    }
}

/// Archspec describes the CPU architecture
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct Archspec {
    pub spec: String,
}

impl Archspec {
    /// Returns the current CPU architecture
    pub fn current() -> Option<Self> {
        Self::from_platform(Platform::current())
    }

    /// Returns the CPU architecture for the given platform
    pub fn from_platform(platform: Platform) -> Option<Self> {
        let archspec = match platform {
            Platform::NoArch => return None,
            Platform::Emscripten32 | Platform::Win32 | Platform::Linux32 => "x86",
            Platform::Win64 | Platform::Osx64 | Platform::Linux64 => "x86_64",
            Platform::LinuxAarch64 => "aarch64",
            Platform::LinuxArmV6l => "armv6l",
            Platform::LinuxArmV7l => "armv7l",
            Platform::LinuxPpc64le => "ppc64le",
            Platform::LinuxPpc64 => "ppc64",
            Platform::LinuxS390X => "s390x",
            Platform::OsxArm64 => "arm64",
        };

        Some(Self {
            spec: archspec.into(),
        })
    }
}

impl From<Archspec> for GenericVirtualPackage {
    fn from(archspec: Archspec) -> Self {
        GenericVirtualPackage {
            name: "__archspec".into(),
            version: Version::from_str("1").unwrap(),
            build_string: archspec.spec,
        }
    }
}

impl From<Archspec> for VirtualPackage {
    fn from(archspec: Archspec) -> Self {
        VirtualPackage::Archspec(archspec)
    }
}
