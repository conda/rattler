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
use rattler_conda_types::{
    GenericVirtualPackage, PackageName, ParseVersionError, Platform, Version,
};
use std::env;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::Arc;

use crate::osx::ParseOsxVersionError;
use libc::DetectLibCError;
use linux::ParseLinuxVersionError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Traits for overridable virtual packages
/// Use as `Cuda::from_default_env_var.unwrap_or(Cuda::current().into()).unwrap()`
pub trait EnvOverride: Sized {
    /// Parse `env_var_value`
    fn from_env_var_name_with_var(
        env_var_name: &str,
        env_var_value: &str,
    ) -> Result<Self, ParseVersionError>;

    /// Read the environment variable and if it exists, try to parse it with [`EnvOverride::from_env_var_name_with_var`]
    /// If the output is:
    /// - `None`, then the environment variable did not exist,
    /// - `Some(Err(None))`, then the environment variable exist but was set to zero, so the package should be disabled
    /// - `Some(Ok(pkg))`, then the override was for the package.
    fn from_env_var_name_or<F>(
        env_var_name: &str,
        f: F,
    ) -> Result<Option<Self>, DetectVirtualPackageError>
    where
        F: FnOnce() -> Result<Option<Self>, DetectVirtualPackageError>,
    {
        match env::var(env_var_name) {
            Ok(var) => {
                if var.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(Self::from_env_var_name_with_var(env_var_name, &var)?))
                }
            }
            Err(env::VarError::NotPresent) => f(),
            Err(e) => Err(DetectVirtualPackageError::VarError(e)),
        }
    }

    /// Helper method for [`EnvOverride::from_env_var_name_or`] that uses [`EnvOverride::current`] as the default.
    fn from_env_var_name_or_current(
        env_var_name: &str,
    ) -> Result<Option<Self>, DetectVirtualPackageError> {
        Self::from_env_var_name_or(env_var_name, Self::current)
    }

    /// Helper method for [`EnvOverride::from_env_var_name_or`] that does not have any default.
    fn from_env_var_name(env_var_name: &str) -> Result<Option<Self>, DetectVirtualPackageError> {
        Self::from_env_var_name_or(env_var_name, || Ok(None))
    }

    /// Default name of the environment variable that overrides the virtual package.
    const DEFAULT_ENV_NAME: &'static str;

    /// Shortcut for `EnvOverride::from_env_var_name(EnvOverride::DEFAULT_ENV_NAME)`.
    fn from_default_env_var_or<F>(f: F) -> Result<Option<Self>, DetectVirtualPackageError>
    where
        F: FnOnce() -> Result<Option<Self>, DetectVirtualPackageError>,
    {
        Self::from_env_var_name_or(Self::DEFAULT_ENV_NAME, f)
    }

    /// Shortcut for `EnvOverride::from_env_var_name(EnvOverride::DEFAULT_ENV_NAME)`.
    fn from_default_env_var() -> Result<Option<Self>, DetectVirtualPackageError> {
        Self::from_default_env_var_or(|| Ok(None))
    }

    /// This method is here so that `<Self as EnvOverride>::current` always returns the same error type.
    fn current() -> Result<Option<Self>, DetectVirtualPackageError>;
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
    pub fn current() -> Result<Vec<Self>, DetectVirtualPackageError> {
        try_detect_virtual_packages()
    }

    /// disable overrides
    pub fn current_no_overrides() -> Result<Vec<Self>, DetectVirtualPackageError> {
        try_detect_virtual_packages_no_overrides()
    }

    /// use custom overrides
    pub fn current_with_overrides(
        overrides: &VirtualPackageOverride,
    ) -> Result<Vec<Self>, DetectVirtualPackageError> {
        try_detect_virtual_packages_with_overrides(overrides)
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

    #[error(transparent)]
    VarError(#[from] env::VarError),

    #[error(transparent)]
    VersionParseError(#[from] ParseVersionError),
}

/// Configure the overrides used in in this crate.

pub struct VirtualPackageOverride {
    osx: Option<String>,
    libc: Option<String>,
    cuda: Option<String>,
}

impl VirtualPackageOverride {
    /// Disable all overrides
    pub fn none() -> Self {
        Self {
            osx: None,
            libc: None,
            cuda: None,
        }
    }
}

impl Default for VirtualPackageOverride {
    fn default() -> Self {
        Self {
            osx: Some(Osx::DEFAULT_ENV_NAME.to_string()),
            libc: Some(LibC::DEFAULT_ENV_NAME.to_string()),
            cuda: Some(Cuda::DEFAULT_ENV_NAME.to_string()),
        }
    }
}

// Detect the available virtual packages on the system
fn try_detect_virtual_packages_with_overrides(
    overrides: &VirtualPackageOverride,
) -> Result<Vec<VirtualPackage>, DetectVirtualPackageError> {
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
        if let Some(libc) = overrides.libc.as_ref().map(String::as_str).map_or_else(
            <LibC as EnvOverride>::current,
            LibC::from_env_var_name_or_current,
        )? {
            result.push(libc.into());
        }
    }

    if platform.is_osx() {
        if let Some(osx) = overrides.osx.as_ref().map(String::as_str).map_or_else(
            <Osx as EnvOverride>::current,
            Osx::from_env_var_name_or_current,
        )? {
            result.push(osx.into());
        }
    }

    if let Some(cuda) = overrides.cuda.as_ref().map(String::as_str).map_or_else(
        <Cuda as EnvOverride>::current,
        Cuda::from_env_var_name_or_current,
    )? {
        result.push(cuda.into());
    }

    if let Some(archspec) = Archspec::current() {
        result.push(archspec.into());
    }

    Ok(result)
}

fn try_detect_virtual_packages() -> Result<Vec<VirtualPackage>, DetectVirtualPackageError> {
    try_detect_virtual_packages_with_overrides(&VirtualPackageOverride::default())
}

fn try_detect_virtual_packages_no_overrides(
) -> Result<Vec<VirtualPackage>, DetectVirtualPackageError> {
    try_detect_virtual_packages_with_overrides(&VirtualPackageOverride::none())
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
    const DEFAULT_ENV_NAME: &'static str = "CONDA_OVERRIDE_GLIBC";

    fn from_env_var_name_with_var(
        _env_var_name: &str,
        env_var_value: &str,
    ) -> Result<Self, ParseVersionError> {
        Version::from_str(env_var_value).map(|version| Self {
            family: "glibc".into(),
            version,
        })
    }

    fn current() -> Result<Option<Self>, DetectVirtualPackageError> {
        Ok(Self::current()?)
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
    fn from_env_var_name_with_var(
        _env_var_name: &str,
        env_var_value: &str,
    ) -> Result<Self, ParseVersionError> {
        Version::from_str(env_var_value).map(|version| Self { version })
    }
    fn current() -> Result<Option<Self>, DetectVirtualPackageError> {
        Ok(Self::current())
    }
    const DEFAULT_ENV_NAME: &'static str = "CONDA_OVERRIDE_CUDA";
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
    fn from_env_var_name_with_var(
        _env_var_name: &str,
        env_var_value: &str,
    ) -> Result<Self, ParseVersionError> {
        Version::from_str(env_var_value).map(|version| Self { version })
    }
    fn current() -> Result<Option<Self>, DetectVirtualPackageError> {
        Ok(Self::current()?)
    }
    const DEFAULT_ENV_NAME: &'static str = "CONDA_OVERRIDE_OSX";
}

#[cfg(test)]
mod test {
    use std::env;
    use std::str::FromStr;

    use rattler_conda_types::Version;

    use crate::Cuda;
    use crate::EnvOverride;
    use crate::LibC;
    use crate::Osx;
    use crate::VirtualPackage;

    #[test]
    fn doesnt_crash() {
        let virtual_packages = VirtualPackage::current().unwrap();
        println!("{virtual_packages:?}");
    }
    #[test]
    fn parse_libc() {
        let v = "1.23";
        let res = LibC {
            version: Version::from_str(v).unwrap(),
            family: "glibc".into(),
        };
        env::set_var(LibC::DEFAULT_ENV_NAME, v);
        assert_eq!(LibC::from_default_env_var().unwrap().unwrap(), res);
        env::set_var(LibC::DEFAULT_ENV_NAME, "");
        assert_eq!(LibC::from_default_env_var().unwrap(), None);
        env::remove_var(LibC::DEFAULT_ENV_NAME);
        assert_eq!(
            LibC::from_default_env_var_or(|| Ok(Some(res.clone())))
                .unwrap()
                .unwrap(),
            res
        );
    }

    #[test]
    fn parse_cuda() {
        let v = "1.234";
        let res = Cuda {
            version: Version::from_str(v).unwrap(),
        };
        env::set_var(Cuda::DEFAULT_ENV_NAME, v);
        assert_eq!(Cuda::from_default_env_var().unwrap().unwrap(), res);
    }

    #[test]
    fn parse_osx() {
        let v = "2.345";
        let res = Osx {
            version: Version::from_str(v).unwrap(),
        };
        env::set_var(Osx::DEFAULT_ENV_NAME, v);
        assert_eq!(Osx::from_default_env_var().unwrap().unwrap(), res);
    }
}
