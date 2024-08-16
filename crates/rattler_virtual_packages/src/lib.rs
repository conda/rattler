#![deny(missing_docs)]

//! A library to detect Conda virtual packages present on a system.
//!
//! A virtual package represents a package that is injected into the solver to provide system
//! information to packages. This allows packages to add dependencies on specific system features,
//! like the platform version, the machines architecture, or the availability of a Cuda driver
//! with a specific version.
//!
//! This library provides both a low- and high level API to detect versions of virtual packages for
//! the host system.
//!
//! To detect all virtual packages for the host system use the [`VirtualPackage::current`] method
//! which will return a memoized slice of all detected virtual packages. The `VirtualPackage` enum
//! represents all available virtual package types. Using it provides some flexibility to the
//! user to not care about which exact virtual packages exist but still allows users to override
//! specific virtual package behavior. Say for instance you just want to detect the capabilities of
//! the host system but you still want to restrict the targeted linux version. You can convert an
//! instance of `VirtualPackage` to `GenericVirtualPackage` which erases any typing for specific
//! virtual packages.
//!
//! Each virtual package is also represented by a struct which can be used to detect the specifics
//! of one virtual package. For instance the [`Linux::current`] method returns an instance of
//! `Linux` which contains the current Linux version. It also provides conversions to the higher
//! level API.
//!
//! Finally at the core of the library are detection functions to perform specific capability
//! detections that are not tied to anything related to virtual packages. See
//! [`cuda::detect_cuda_version_via_libcuda`] as an example.

pub mod cuda;
pub mod libc;
pub mod linux;
pub mod osx;

use archspec::cpu::Microarchitecture;
use core::fmt;
use once_cell::sync::OnceCell;
use rattler_conda_types::{GenericVirtualPackage, PackageName, Platform, Version};
use std::any::type_name;
use std::env;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::Arc;

use crate::osx::ParseOsxVersionError;
use libc::DetectLibCError;
use linux::ParseLinuxVersionError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub trait EnvOverride: Sized {
    // Used to override the mechanisms here
    fn from_env_var_name(env_var_name: &str) -> Option<Self>;

    fn default_env_name() -> String;

    fn from_default_env_var() -> Option<Self> {
        Self::from_env_var_name(&Self::default_env_name())
    }
}

/// An enum that represents all virtual package types provided by this library.
#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub enum VirtualPackage {
    /// Available on windows
    Win,

    /// Available on unix based platforms
    Unix,

    /// Available when running on Linux
    Linux(Linux),

    /// Available when running on OSX
    Osx(Osx),

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
                name: PackageName::new_unchecked("__win"),
                version: Version::major(0),
                build_string: "0".into(),
            },
            VirtualPackage::Unix => GenericVirtualPackage {
                name: PackageName::new_unchecked("__unix"),
                version: Version::major(0),
                build_string: "0".into(),
            },
            VirtualPackage::Linux(linux) => linux.into(),
            VirtualPackage::Osx(osx) => osx.into(),
            VirtualPackage::LibC(libc) => libc.into(),
            VirtualPackage::Cuda(cuda) => cuda.into(),
            VirtualPackage::Archspec(spec) => spec.into(),
        }
    }
}

impl VirtualPackage {
    /// Returns virtual packages detected for the current system or an error if the versions could
    /// not be properly detected.
    pub fn current() -> Result<&'static [Self], DetectVirtualPackageError> {
        static DETECED_VIRTUAL_PACKAGES: OnceCell<Vec<VirtualPackage>> = OnceCell::new();
        DETECED_VIRTUAL_PACKAGES
            .get_or_try_init(try_detect_virtual_packages)
            .map(Vec::as_slice)
    }
}

/// An error that might be returned by [`VirtualPackage::current`].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum DetectVirtualPackageError {
    #[error(transparent)]
    ParseLinuxVersion(#[from] ParseLinuxVersionError),

    #[error(transparent)]
    ParseMacOsVersion(#[from] ParseOsxVersionError),

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
            result.push(linux_version.into());
        }
        if let Some(libc) = LibC::current()? {
            result.push(libc.into());
        }
    }

    if platform.is_osx() {
        if let Some(osx) = Osx::current()? {
            result.push(osx.into());
        }
    }

    if let Some(cuda) = Cuda::current() {
        result.push(cuda.into());
    }

    if let Some(archspec) = Archspec::current() {
        result.push(archspec.into());
    }

    Ok(result)
}

/// Linux virtual package description
#[derive(Clone, Eq, PartialEq, Hash, Debug, Deserialize)]
pub struct Linux {
    /// The version of linux
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
            name: PackageName::new_unchecked("__linux"),
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

impl From<Version> for Linux {
    fn from(version: Version) -> Self {
        Linux { version }
    }
}

impl EnvOverride for Linux {
    fn from_env_var_name(env_var_name: &str) -> Option<Self> {
        let var = env::var(env_var_name).ok()?;
        Some(Self::from(Version::from_str(&var).ok()?))
    }

    fn default_env_name() -> String {
        "CONDA_OVERRIDE_LINUX".into()
    }
}

/// `LibC` virtual package description
#[derive(Clone, Eq, PartialEq, Hash, Debug, Deserialize)]
pub struct LibC {
    /// The family of LibC. This could be glibc for instance.
    pub family: String,

    /// The version of the libc distribution.
    pub version: Version,
}

impl LibC {
    /// Returns the `LibC` family and version of the current platform.
    ///
    /// Returns an error if determining the `LibC` family and version resulted in an error. Returns
    /// `None` if the current platform does not have an available version of `LibC`.
    pub fn current() -> Result<Option<Self>, DetectLibCError> {
        Ok(libc::libc_family_and_version()?.map(|(family, version)| Self { family, version }))
    }

    pub fn default_familiy() -> String {
        "GLIBC".into()
    }
}

#[allow(clippy::fallible_impl_from)]
impl From<LibC> for GenericVirtualPackage {
    fn from(libc: LibC) -> Self {
        GenericVirtualPackage {
            // TODO: Convert the family to a valid package name. We can simply replace invalid
            // characters.
            name: format!("__{}", libc.family.to_lowercase())
                .try_into()
                .unwrap(),
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

impl EnvOverride for LibC {
    fn default_env_name() -> String {
        "CONDA_OVERRIDE_GLIBC".into()
    }

    fn from_env_var_name(env_var_name: &str) -> Option<Self> {
        let var = env::var(env_var_name).ok()?;
        let pars = var.split("@");
        let mut version: Option<Version> = None;
        let mut family: Option<String> = None;
        for (i, x) in pars.enumerate() {
            match i {
                0 => version = Some(Version::from_str(x).ok()?),
                1 => family = Some(x.into()),
                _ => return None,
            }
        }
        family.get_or_insert_with(Self::default_familiy);
        let libc = Self {
            family: family?,
            version: version?,
        };
        Some(libc)
    }
}

/// Cuda virtual package description
#[derive(Clone, Eq, PartialEq, Hash, Debug, Deserialize)]
pub struct Cuda {
    /// The maximum supported Cuda version.
    pub version: Version,
}

impl Cuda {
    /// Returns the maximum Cuda version available on the current platform.
    pub fn current() -> Option<Self> {
        cuda::cuda_version().map(|version| Self { version })
    }
}

impl From<Version> for Cuda {
    fn from(version: Version) -> Self {
        Self { version }
    }
}

impl EnvOverride for Cuda {
    fn from_env_var_name(env_var_name: &str) -> Option<Self> {
        let var = env::var(env_var_name).ok()?;
        Some(Self::from(Version::from_str(&var).ok()?))
    }

    fn default_env_name() -> String {
        "CONDA_OVERRIDE_CUDA".into()
    }
}

impl From<Cuda> for GenericVirtualPackage {
    fn from(cuda: Cuda) -> Self {
        GenericVirtualPackage {
            name: PackageName::new_unchecked("__cuda"),
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
#[derive(Clone, Debug)]
pub struct Archspec {
    /// The associated microarchitecture
    pub spec: Arc<Microarchitecture>,
}

impl Serialize for Archspec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.spec.name().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Archspec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        let spec = archspec::cpu::Microarchitecture::known_targets()
            .get(&name)
            .cloned()
            .unwrap_or_else(|| Arc::new(archspec::cpu::Microarchitecture::generic(&name)));
        Ok(Self { spec })
    }
}

impl Hash for Archspec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.spec.name().hash(state);
    }
}

impl PartialEq<Self> for Archspec {
    fn eq(&self, other: &Self) -> bool {
        self.spec.name() == other.spec.name()
    }
}

impl Eq for Archspec {}

impl From<Arc<Microarchitecture>> for Archspec {
    fn from(arch: Arc<Microarchitecture>) -> Self {
        Self { spec: arch }
    }
}

impl Archspec {
    /// Returns the current CPU architecture
    pub fn current() -> Option<Self> {
        archspec::cpu::host().ok().map(Into::into)
    }

    /// Returns the minimal supported archspec architecture for the given
    /// platform.
    #[allow(clippy::match_same_arms)]
    pub fn from_platform(platform: Platform) -> Option<Self> {
        // The values are taken from the archspec-json library.
        // See: https://github.com/archspec/archspec-json/blob/master/cpu/microarchitectures.json
        let archspec_name = match platform {
            Platform::NoArch | Platform::Unknown => return None,
            Platform::EmscriptenWasm32 | Platform::WasiWasm32 => return None,
            Platform::Win32 | Platform::Linux32 => "x86",
            Platform::Win64 | Platform::Osx64 | Platform::Linux64 => "x86_64",
            Platform::LinuxAarch64 | Platform::LinuxArmV6l | Platform::LinuxArmV7l => "aarch64",
            Platform::LinuxPpc64le => "ppc64le",
            Platform::LinuxPpc64 => "ppc64",
            Platform::LinuxS390X => "s390x",
            Platform::LinuxRiscv32 => "riscv32",
            Platform::LinuxRiscv64 => "riscv64",
            // IBM Zos is a special case. It is not supported by archspec as far as I can see.
            Platform::ZosZ => return None,

            // TODO: There must be a minimal aarch64 version that windows supports.
            Platform::WinArm64 => "aarch64",

            // The first every Apple Silicon Macs are based on m1.
            Platform::OsxArm64 => "m1",
        };

        Some(
            archspec::cpu::Microarchitecture::known_targets()
                .get(archspec_name)
                .cloned()
                .unwrap_or_else(|| {
                    Arc::new(archspec::cpu::Microarchitecture::generic(archspec_name))
                })
                .into(),
        )
    }
}

impl EnvOverride for Archspec {
    fn from_env_var_name(env_var_name: &str) -> Option<Self> {
        let var = env::var(env_var_name).ok()?;
        Self::from_platform(Platform::from_str(&var).ok()?)
    }

    fn default_env_name() -> String {
        "CONDA_OVERRIDE_ARCH".into()
    }
}

impl From<Archspec> for GenericVirtualPackage {
    fn from(archspec: Archspec) -> Self {
        GenericVirtualPackage {
            name: PackageName::new_unchecked("__archspec"),
            version: Version::major(1),
            build_string: archspec.spec.name().into(),
        }
    }
}

impl From<Archspec> for VirtualPackage {
    fn from(archspec: Archspec) -> Self {
        VirtualPackage::Archspec(archspec)
    }
}

/// OSX virtual package description
#[derive(Clone, Eq, PartialEq, Hash, Debug, Deserialize)]
pub struct Osx {
    /// The OSX version
    pub version: Version,
}

impl Osx {
    /// Returns the OSX version of the current platform.
    ///
    /// Returns an error if determining the OSX version resulted in an error. Returns `None` if
    /// the current platform is not an OSX based platform.
    pub fn current() -> Result<Option<Self>, ParseOsxVersionError> {
        Ok(osx::osx_version()?.map(|version| Self { version }))
    }
}

impl From<Osx> for GenericVirtualPackage {
    fn from(osx: Osx) -> Self {
        GenericVirtualPackage {
            name: PackageName::new_unchecked("__osx"),
            version: osx.version,
            build_string: "0".into(),
        }
    }
}

impl From<Osx> for VirtualPackage {
    fn from(osx: Osx) -> Self {
        VirtualPackage::Osx(osx)
    }
}

impl From<Version> for Osx {
    fn from(version: Version) -> Self {
        Self { version }
    }
}

impl EnvOverride for Osx {
    fn from_env_var_name(env_var_name: &str) -> Option<Self> {
        let var = env::var(env_var_name).ok()?;
        Some(Self::from(Version::from_str(&var).ok()?))
    }

    fn default_env_name() -> String {
        "CONDA_OVERRIDE_OSX".into()
    }
}

#[cfg(test)]
mod test {
    use crate::VirtualPackage;

    #[test]
    fn doesnt_crash() {
        let virtual_packages = VirtualPackage::current().unwrap();
        println!("{virtual_packages:?}");
    }
}
