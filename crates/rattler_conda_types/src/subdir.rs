//! Conda channel subdirs, as defined by CEP 26.
use std::{cmp::Ordering, fmt, fmt::Formatter, str::FromStr};

use serde::{Deserializer, Serializer};
use thiserror::Error;
use tinystr::TinyAsciiStr;

/// The maximum length of a subdir name, as specified by CEP 26.
pub const MAX_SUBDIR_LEN: usize = 32;

/// A conda channel subdir: either the literal `noarch` or `{os}-{arch}`, as
/// defined by [CEP 26](https://github.com/conda/ceps/blob/main/cep-0026.md).
///
/// Subdirs rattler has built-in knowledge of are available as associated
/// constants ([`Subdir::Linux64`], [`Subdir::NoArch`], ...) and can be listed
/// with [`Subdir::known`]. Any other name that satisfies CEP 26 parses as
/// well, so channels can publish subdirs for architectures that predate this
/// version of rattler:
///
/// ```
/// # use rattler_conda_types::Subdir;
/// let subdir: Subdir = "linux-esp32s3".parse().unwrap();
/// assert_eq!(subdir.only_platform(), Some("linux"));
/// assert!(subdir.is_linux());
/// assert!(!subdir.is_known());
/// ```
///
/// The name is stored inline, so a `Subdir` is `Copy` and never allocates.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct Subdir(TinyAsciiStr<MAX_SUBDIR_LEN>);

/// Every subdir rattler has built-in knowledge of, in canonical order.
const KNOWN_SUBDIRS: &[Subdir] = &[
    Subdir::NoArch,
    Subdir::Linux32,
    Subdir::Linux64,
    Subdir::LinuxAarch64,
    Subdir::LinuxArmV6l,
    Subdir::LinuxArmV7l,
    Subdir::LinuxLoongArch64,
    Subdir::LinuxPpc64le,
    Subdir::LinuxPpc64,
    Subdir::LinuxPpc,
    Subdir::LinuxS390X,
    Subdir::LinuxRiscv32,
    Subdir::LinuxRiscv64,
    Subdir::FreeBsd32,
    Subdir::FreeBsd64,
    Subdir::FreeBsdArm64,
    Subdir::Osx64,
    Subdir::OsxArm64,
    Subdir::IosArm64,
    Subdir::IosSimulatorArm64,
    Subdir::IosSimulator64,
    Subdir::AndroidAarch64,
    Subdir::AndroidArmV7a,
    Subdir::Android64,
    Subdir::Android32,
    Subdir::Win32,
    Subdir::Win64,
    Subdir::WinArm64,
    Subdir::EmscriptenWasm32,
    Subdir::EmscriptenWasm64,
    Subdir::WasiWasm32,
    Subdir::ZosZ,
];

/// The `{os}` tokens that rattler knows to be unix-like. A subdir whose os
/// token is missing here is not assumed to be unix, because its C library and
/// path conventions are unknown.
const UNIX_PLATFORMS: &[&str] = &[
    "linux",
    "osx",
    "ios",
    "iossimulator",
    "android",
    "freebsd",
    "emscripten",
];

#[expect(
    non_upper_case_globals,
    reason = "these constants replace enum variants and keep their spelling"
)]
impl Subdir {
    /// The `noarch` subdir.
    pub const NoArch: Subdir = Subdir::from_static("noarch");

    /// The `linux-32` subdir.
    pub const Linux32: Subdir = Subdir::from_static("linux-32");

    /// The `linux-64` subdir.
    pub const Linux64: Subdir = Subdir::from_static("linux-64");

    /// The `linux-aarch64` subdir.
    pub const LinuxAarch64: Subdir = Subdir::from_static("linux-aarch64");

    /// The `linux-armv6l` subdir.
    pub const LinuxArmV6l: Subdir = Subdir::from_static("linux-armv6l");

    /// The `linux-armv7l` subdir.
    pub const LinuxArmV7l: Subdir = Subdir::from_static("linux-armv7l");

    /// The `linux-loongarch64` subdir.
    pub const LinuxLoongArch64: Subdir = Subdir::from_static("linux-loongarch64");

    /// The `linux-ppc64le` subdir.
    pub const LinuxPpc64le: Subdir = Subdir::from_static("linux-ppc64le");

    /// The `linux-ppc64` subdir.
    pub const LinuxPpc64: Subdir = Subdir::from_static("linux-ppc64");

    /// The `linux-ppc` subdir.
    pub const LinuxPpc: Subdir = Subdir::from_static("linux-ppc");

    /// The `linux-s390x` subdir.
    pub const LinuxS390X: Subdir = Subdir::from_static("linux-s390x");

    /// The `linux-riscv32` subdir.
    pub const LinuxRiscv32: Subdir = Subdir::from_static("linux-riscv32");

    /// The `linux-riscv64` subdir.
    pub const LinuxRiscv64: Subdir = Subdir::from_static("linux-riscv64");

    /// The `freebsd-32` subdir.
    pub const FreeBsd32: Subdir = Subdir::from_static("freebsd-32");

    /// The `freebsd-64` subdir.
    pub const FreeBsd64: Subdir = Subdir::from_static("freebsd-64");

    /// The `freebsd-arm64` subdir.
    pub const FreeBsdArm64: Subdir = Subdir::from_static("freebsd-arm64");

    /// The `osx-64` subdir.
    pub const Osx64: Subdir = Subdir::from_static("osx-64");

    /// The `osx-arm64` subdir.
    pub const OsxArm64: Subdir = Subdir::from_static("osx-arm64");

    /// The `ios-arm64` subdir.
    pub const IosArm64: Subdir = Subdir::from_static("ios-arm64");

    /// The `iossimulator-arm64` subdir.
    pub const IosSimulatorArm64: Subdir = Subdir::from_static("iossimulator-arm64");

    /// The `iossimulator-64` subdir.
    pub const IosSimulator64: Subdir = Subdir::from_static("iossimulator-64");

    /// The `android-aarch64` subdir.
    pub const AndroidAarch64: Subdir = Subdir::from_static("android-aarch64");

    /// The `android-armv7a` subdir.
    pub const AndroidArmV7a: Subdir = Subdir::from_static("android-armv7a");

    /// The `android-64` subdir.
    pub const Android64: Subdir = Subdir::from_static("android-64");

    /// The `android-32` subdir.
    pub const Android32: Subdir = Subdir::from_static("android-32");

    /// The `win-32` subdir.
    pub const Win32: Subdir = Subdir::from_static("win-32");

    /// The `win-64` subdir.
    pub const Win64: Subdir = Subdir::from_static("win-64");

    /// The `win-arm64` subdir.
    pub const WinArm64: Subdir = Subdir::from_static("win-arm64");

    /// The `emscripten-wasm32` subdir.
    pub const EmscriptenWasm32: Subdir = Subdir::from_static("emscripten-wasm32");

    /// The `emscripten-wasm64` subdir.
    pub const EmscriptenWasm64: Subdir = Subdir::from_static("emscripten-wasm64");

    /// The `wasi-wasm32` subdir.
    pub const WasiWasm32: Subdir = Subdir::from_static("wasi-wasm32");

    /// The `zos-z` subdir.
    pub const ZosZ: Subdir = Subdir::from_static("zos-z");

    /// Builds a subdir from a name that is known to be valid at compile time.
    const fn from_static(name: &str) -> Subdir {
        match TinyAsciiStr::try_from_str(name) {
            Ok(name) => Subdir(name),
            Err(_) => panic!("subdir name is not valid ASCII or too long"),
        }
    }

    /// Returns the subdir for which the current binary was built, or `None`
    /// when the build target has no conda subdir (for example
    /// `wasm32-unknown-unknown` or Mac Catalyst).
    pub const fn current() -> Option<Subdir> {
        #[cfg(target_os = "linux")]
        {
            #[cfg(target_arch = "x86")]
            return Some(Subdir::Linux32);

            #[cfg(target_arch = "x86_64")]
            return Some(Subdir::Linux64);

            #[cfg(target_arch = "aarch64")]
            return Some(Subdir::LinuxAarch64);

            #[cfg(target_arch = "arm")]
            {
                #[cfg(target_feature = "v7")]
                return Some(Subdir::LinuxArmV7l);

                #[cfg(not(target_feature = "v7"))]
                return Some(Subdir::LinuxArmV6l);
            }

            #[cfg(target_arch = "loongarch64")]
            return Some(Subdir::LinuxLoongArch64);

            #[cfg(all(target_arch = "powerpc64", target_endian = "little"))]
            return Some(Subdir::LinuxPpc64le);

            #[cfg(all(target_arch = "powerpc64", target_endian = "big"))]
            return Some(Subdir::LinuxPpc64);

            #[cfg(target_arch = "powerpc")]
            return Some(Subdir::LinuxPpc);

            #[cfg(target_arch = "s390x")]
            return Some(Subdir::LinuxS390X);

            #[cfg(target_arch = "riscv32")]
            return Some(Subdir::LinuxRiscv32);

            #[cfg(target_arch = "riscv64")]
            return Some(Subdir::LinuxRiscv64);

            #[cfg(not(any(
                target_arch = "x86_64",
                target_arch = "x86",
                target_arch = "riscv32",
                target_arch = "riscv64",
                target_arch = "aarch64",
                target_arch = "arm",
                target_arch = "powerpc64",
                target_arch = "powerpc",
                target_arch = "s390x",
                target_arch = "loongarch64"
            )))]
            compile_error!("unsupported linux architecture");
        }
        #[cfg(target_os = "freebsd")]
        {
            #[cfg(target_arch = "x86")]
            return Some(Subdir::FreeBsd32);

            #[cfg(target_arch = "x86_64")]
            return Some(Subdir::FreeBsd64);

            #[cfg(target_arch = "aarch64")]
            return Some(Subdir::FreeBsdArm64);

            #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
            compile_error!("unsupported freebsd architecture");
        }
        #[cfg(windows)]
        {
            #[cfg(target_arch = "x86")]
            return Some(Subdir::Win32);

            #[cfg(target_arch = "x86_64")]
            return Some(Subdir::Win64);

            #[cfg(target_arch = "aarch64")]
            return Some(Subdir::WinArm64);

            #[cfg(not(any(target_arch = "x86_64", target_arch = "x86", target_arch = "aarch64")))]
            compile_error!("unsupported windows architecture");
        }
        #[cfg(target_os = "macos")]
        {
            #[cfg(target_arch = "x86_64")]
            return Some(Subdir::Osx64);

            #[cfg(target_arch = "aarch64")]
            return Some(Subdir::OsxArm64);
        }

        #[cfg(target_os = "ios")]
        {
            // Mac Catalyst (`*-apple-ios-macabi`) also reports `target_os =
            // "ios"`, but produces binaries that run on macOS. No conda
            // subdir exists for it.
            #[cfg(target_abi = "macabi")]
            return None;

            #[cfg(all(target_arch = "aarch64", target_abi = "sim"))]
            return Some(Subdir::IosSimulatorArm64);

            #[cfg(all(
                target_arch = "aarch64",
                not(any(target_abi = "sim", target_abi = "macabi"))
            ))]
            return Some(Subdir::IosArm64);

            // The only x86_64 iOS target (`x86_64-apple-ios`) is the simulator.
            #[cfg(all(target_arch = "x86_64", not(target_abi = "macabi")))]
            return Some(Subdir::IosSimulator64);

            #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
            compile_error!("unsupported ios architecture");
        }

        #[cfg(target_os = "android")]
        {
            #[cfg(target_arch = "aarch64")]
            return Some(Subdir::AndroidAarch64);

            // armv7-linux-androideabi and thumbv7neon-linux-androideabi → armeabi-v7a
            #[cfg(target_arch = "arm")]
            return Some(Subdir::AndroidArmV7a);

            #[cfg(target_arch = "x86")]
            return Some(Subdir::Android32);

            #[cfg(target_arch = "x86_64")]
            return Some(Subdir::Android64);

            // e.g. riscv64-linux-android
            #[cfg(not(any(
                target_arch = "aarch64",
                target_arch = "arm",
                target_arch = "x86",
                target_arch = "x86_64"
            )))]
            compile_error!("unsupported android architecture");
        }

        #[cfg(target_os = "emscripten")]
        {
            #[cfg(target_arch = "wasm32")]
            return Some(Subdir::EmscriptenWasm32);

            #[cfg(target_arch = "wasm64")]
            return Some(Subdir::EmscriptenWasm64);
        }

        #[cfg(target_os = "wasi")]
        {
            #[cfg(target_arch = "wasm32")]
            return Some(Subdir::WasiWasm32);
        }

        #[cfg(not(any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "macos",
            target_os = "ios",
            target_os = "android",
            target_os = "emscripten",
            target_os = "wasi",
            windows
        )))]
        {
            None
        }
    }

    /// Returns a string representation of the subdir.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Returns every subdir rattler has built-in knowledge of.
    ///
    /// Subdirs outside this list still parse; use [`Subdir::is_known`] to tell
    /// them apart.
    pub fn known() -> impl ExactSizeIterator<Item = Self> {
        KNOWN_SUBDIRS.iter().copied()
    }

    /// Returns whether this is a subdir rattler has built-in knowledge of.
    pub fn is_known(&self) -> bool {
        KNOWN_SUBDIRS.contains(self)
    }

    /// Parses a subdir, accepting only names rattler has built-in knowledge
    /// of.
    ///
    /// Use this instead of [`FromStr`] where an arbitrary string is being
    /// probed to see whether it denotes a subdir at all. A channel name like
    /// `conda-forge` satisfies the CEP 26 subdir syntax, so `FromStr` accepts
    /// it and cannot be used to tell a subdir from a channel.
    pub fn from_known_str(name: &str) -> Option<Subdir> {
        KNOWN_SUBDIRS
            .iter()
            .find(|subdir| subdir.as_str() == name)
            .copied()
    }

    /// Returns the `{os}` part of the subdir (e.g. `linux`, `win`, `osx`,
    /// `freebsd`, `ios`, `iossimulator`, `android`), or `None` for `noarch`.
    pub fn only_platform(&self) -> Option<&str> {
        self.as_str().split_once('-').map(|(platform, _)| platform)
    }

    /// Returns the architecture of the subdir, or `None` for `noarch`.
    ///
    /// Conda spells `x86` and `x86_64` as `-32` and `-64` in the subdir; both are
    /// reported here under their canonical architecture name.
    pub fn arch(&self) -> Option<Arch> {
        let (_, arch) = self.as_str().split_once('-')?;
        Some(match arch {
            "32" => Arch::X86,
            "64" => Arch::X86_64,
            arch => Arch(TinyAsciiStr::try_from_str(arch).ok()?),
        })
    }

    /// Returns true if the subdir is a windows based subdir.
    pub fn is_windows(&self) -> bool {
        self.only_platform() == Some("win")
    }

    /// Returns true if the subdir is a linux based subdir.
    ///
    /// Android runs a Linux kernel but links against Bionic instead of glibc,
    /// so `android-*` is deliberately not linux: `linux-*` packages declare
    /// their libc requirement through `__glibc`, which Bionic cannot satisfy.
    pub fn is_linux(&self) -> bool {
        self.only_platform() == Some("linux")
    }

    /// Returns true if the subdir is a macOS based subdir.
    pub fn is_osx(&self) -> bool {
        self.only_platform() == Some("osx")
    }

    /// Returns true if the subdir is an iOS based subdir (device or
    /// simulator).
    pub fn is_ios(&self) -> bool {
        matches!(self.only_platform(), Some("ios" | "iossimulator"))
    }

    /// Returns true if the subdir is an Android based subdir.
    pub fn is_android(&self) -> bool {
        self.only_platform() == Some("android")
    }

    /// Returns true if the subdir is a unix based subdir.
    pub fn is_unix(&self) -> bool {
        self.only_platform()
            .is_some_and(|platform| UNIX_PLATFORMS.contains(&platform))
    }
}

impl PartialOrd for Subdir {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Subdir {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

/// Returns whether `name` is a syntactically valid subdir name according to
/// [CEP 26](https://github.com/conda/ceps/blob/main/cep-0026.md): either the
/// literal `noarch`, or `{os}-{arch}` where both parts consist of lowercase
/// ASCII letters and digits. The name must not exceed 32 characters.
pub fn is_valid_subdir_name(name: &str) -> bool {
    if name.len() > MAX_SUBDIR_LEN {
        return false;
    }
    if name == "noarch" {
        return true;
    }
    let is_lowercase_alphanumeric = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    };
    match name.split_once('-') {
        Some((os, arch)) => is_lowercase_alphanumeric(os) && is_lowercase_alphanumeric(arch),
        None => false,
    }
}

/// An error that can occur when parsing a subdir from a string.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
#[error(
    "'{name}' is not a valid subdir name; it must be 'noarch' or '{{os}}-{{arch}}' with both \
     parts consisting of lowercase ASCII letters and digits, at most 32 characters in total"
)]
pub struct ParseSubdirError {
    /// The string that could not be parsed.
    pub name: String,
}

impl FromStr for Subdir {
    type Err = ParseSubdirError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        if let Some(known) = Subdir::from_known_str(name) {
            return Ok(known);
        }
        // The CEP 26 rules are a subset of what `TinyAsciiStr` accepts, so a
        // valid name always fits.
        match TinyAsciiStr::try_from_str(name) {
            Ok(name) if is_valid_subdir_name(name.as_str()) => Ok(Subdir(name)),
            _ => Err(ParseSubdirError {
                name: name.to_owned(),
            }),
        }
    }
}

impl TryFrom<&str> for Subdir {
    type Error = ParseSubdirError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl fmt::Display for Subdir {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Subdir {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for Subdir {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Subdir {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// The maximum length of an architecture name. An architecture is at most a
/// full subdir minus its `{os}` part, except that `x86` and `x86_64` are spelled
/// out here while the subdir abbreviates them.
pub const MAX_ARCH_LEN: usize = MAX_SUBDIR_LEN;

/// An architecture a conda package can be built for.
///
/// Architectures rattler has built-in knowledge of are available as associated
/// constants and can be listed with [`Arch::known`], but any lowercase ASCII
/// name parses, so [`Subdir::arch`] also works for subdirs rattler does not
/// know.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct Arch(TinyAsciiStr<MAX_ARCH_LEN>);

/// Every architecture rattler has built-in knowledge of, in canonical order.
const KNOWN_ARCHES: &[Arch] = &[
    Arch::X86,
    Arch::X86_64,
    Arch::Aarch64,
    Arch::Arm64,
    Arch::ArmV6l,
    Arch::ArmV7l,
    Arch::ArmV7a,
    Arch::LoongArch64,
    Arch::Ppc64le,
    Arch::Ppc64,
    Arch::Ppc,
    Arch::S390X,
    Arch::Riscv32,
    Arch::Riscv64,
    Arch::Wasm32,
    Arch::Wasm64,
    Arch::Z,
];

#[expect(
    non_upper_case_globals,
    reason = "these constants replace enum variants and keep their spelling"
)]
impl Arch {
    /// The `x86` architecture.
    pub const X86: Arch = Arch::from_static("x86");

    /// The `x86_64` architecture.
    pub const X86_64: Arch = Arch::from_static("x86_64");

    /// The `aarch64` architecture.
    pub const Aarch64: Arch = Arch::from_static("aarch64");

    /// The `arm64` architecture.
    pub const Arm64: Arch = Arch::from_static("arm64");

    /// The `armv6l` architecture.
    pub const ArmV6l: Arch = Arch::from_static("armv6l");

    /// The `armv7l` architecture.
    pub const ArmV7l: Arch = Arch::from_static("armv7l");

    /// The `armv7a` architecture.
    pub const ArmV7a: Arch = Arch::from_static("armv7a");

    /// The `loongarch64` architecture.
    pub const LoongArch64: Arch = Arch::from_static("loongarch64");

    /// The `ppc64le` architecture.
    pub const Ppc64le: Arch = Arch::from_static("ppc64le");

    /// The `ppc64` architecture.
    pub const Ppc64: Arch = Arch::from_static("ppc64");

    /// The `ppc` architecture.
    pub const Ppc: Arch = Arch::from_static("ppc");

    /// The `s390x` architecture.
    pub const S390X: Arch = Arch::from_static("s390x");

    /// The `riscv32` architecture.
    pub const Riscv32: Arch = Arch::from_static("riscv32");

    /// The `riscv64` architecture.
    pub const Riscv64: Arch = Arch::from_static("riscv64");

    /// The `wasm32` architecture.
    pub const Wasm32: Arch = Arch::from_static("wasm32");

    /// The `wasm64` architecture.
    pub const Wasm64: Arch = Arch::from_static("wasm64");

    /// The `z` architecture.
    pub const Z: Arch = Arch::from_static("z");

    /// Builds an arch from a name that is known to be valid at compile time.
    const fn from_static(name: &str) -> Arch {
        match TinyAsciiStr::try_from_str(name) {
            Ok(name) => Arch(name),
            Err(_) => panic!("arch name is not valid ASCII or too long"),
        }
    }

    /// Returns the arch for which the current binary was built, or `None`
    /// when [`Subdir::current`] is `None`.
    pub fn current() -> Option<Self> {
        Subdir::current().and_then(|subdir| subdir.arch())
    }

    /// Returns a string representation of the arch.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Returns every architecture rattler has built-in knowledge of.
    pub fn known() -> impl ExactSizeIterator<Item = Self> {
        KNOWN_ARCHES.iter().copied()
    }

    /// Returns whether this is an architecture rattler has built-in knowledge
    /// of.
    pub fn is_known(&self) -> bool {
        KNOWN_ARCHES.contains(self)
    }
}

impl PartialOrd for Arch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Arch {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

/// An error that can occur when parsing an arch from a string.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
#[error(
    "'{name}' is not a valid arch name; it must consist of lowercase ASCII letters, digits and \
     underscores, at most 32 characters"
)]
pub struct ParseArchError {
    /// The string that could not be parsed.
    pub name: String,
}

impl FromStr for Arch {
    type Err = ParseArchError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        let is_valid = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        match TinyAsciiStr::try_from_str(name) {
            Ok(name) if is_valid => Ok(Arch(name)),
            _ => Err(ParseArchError {
                name: name.to_owned(),
            }),
        }
    }
}

impl TryFrom<&str> for Arch {
    type Error = ParseArchError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Arch {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for Arch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Arch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;

    #[test]
    fn test_parse_subdir() {
        for subdir in Subdir::known() {
            assert_eq!(subdir.as_str().parse::<Subdir>().unwrap(), subdir);
            assert_eq!(subdir.to_string(), subdir.as_str());
            assert!(subdir.is_known());
        }
        assert_eq!("linux-64".parse::<Subdir>().unwrap(), Subdir::Linux64);
        assert_eq!("noarch".parse::<Subdir>().unwrap(), Subdir::NoArch);
        assert_eq!("zos-z".parse::<Subdir>().unwrap(), Subdir::ZosZ);
    }

    #[test]
    fn test_ios_android_subdirs() {
        // The arch axis is split out from the subdir. Following conda
        // convention, x86_64/x86 are spelled `-64`/`-32` in the subdir but
        // still report the underlying arch.
        assert_eq!(Subdir::IosArm64.arch(), Some(Arch::Arm64));
        assert_eq!(Subdir::IosSimulatorArm64.arch(), Some(Arch::Arm64));
        assert_eq!(Subdir::IosSimulator64.arch(), Some(Arch::X86_64));
        assert_eq!(Subdir::AndroidAarch64.arch(), Some(Arch::Aarch64));
        assert_eq!(Subdir::AndroidArmV7a.arch(), Some(Arch::ArmV7a));
        assert_eq!(Subdir::Android64.arch(), Some(Arch::X86_64));
        assert_eq!(Subdir::Android32.arch(), Some(Arch::X86));

        // iOS/Android classify as unix, but not as osx/linux (they use
        // different C libraries and get their own virtual packages).
        assert!(Subdir::IosArm64.is_ios());
        assert!(Subdir::IosArm64.is_unix());
        assert!(!Subdir::IosArm64.is_osx());
        assert_eq!(Subdir::IosArm64.only_platform(), Some("ios"));
        // Simulators are still iOS, but carry their own single-dash subdir
        // prefix so tools that split the subdir on `-` keep working.
        assert!(Subdir::IosSimulatorArm64.is_ios());
        assert_eq!(
            Subdir::IosSimulatorArm64.only_platform(),
            Some("iossimulator")
        );

        assert!(Subdir::AndroidAarch64.is_android());
        assert!(Subdir::AndroidAarch64.is_unix());
        assert!(!Subdir::AndroidAarch64.is_linux());
        assert_eq!(Subdir::AndroidAarch64.only_platform(), Some("android"));
    }

    #[test]
    fn test_display() {
        assert_eq!(Subdir::Linux64.to_string(), "linux-64");
        assert_eq!(Subdir::Linux32.to_string(), "linux-32");
        assert_eq!(Subdir::LinuxAarch64.to_string(), "linux-aarch64");
        assert_eq!(Subdir::ZosZ.to_string(), "zos-z");
        assert_eq!(format!("{:?}", Subdir::Linux64), "linux-64");
    }

    #[test]
    fn test_arch() {
        assert_eq!(Subdir::Linux64.arch(), Some(Arch::X86_64));
        assert_eq!(Subdir::Linux32.arch(), Some(Arch::X86));
        assert_eq!(Subdir::LinuxAarch64.arch(), Some(Arch::Aarch64));
        assert_eq!(Subdir::LinuxArmV6l.arch(), Some(Arch::ArmV6l));
        assert_eq!(Subdir::LinuxArmV7l.arch(), Some(Arch::ArmV7l));
        assert_eq!(Subdir::LinuxLoongArch64.arch(), Some(Arch::LoongArch64));
        assert_eq!(Subdir::LinuxPpc64le.arch(), Some(Arch::Ppc64le));
        assert_eq!(Subdir::LinuxPpc64.arch(), Some(Arch::Ppc64));
        assert_eq!(Subdir::LinuxPpc.arch(), Some(Arch::Ppc));
        assert_eq!(Subdir::LinuxS390X.arch(), Some(Arch::S390X));
        assert_eq!(Subdir::LinuxRiscv32.arch(), Some(Arch::Riscv32));
        assert_eq!(Subdir::LinuxRiscv64.arch(), Some(Arch::Riscv64));
        assert_eq!(Subdir::FreeBsd32.arch(), Some(Arch::X86));
        assert_eq!(Subdir::FreeBsd64.arch(), Some(Arch::X86_64));
        assert_eq!(Subdir::FreeBsdArm64.arch(), Some(Arch::Arm64));
        assert_eq!(Subdir::Osx64.arch(), Some(Arch::X86_64));
        assert_eq!(Subdir::OsxArm64.arch(), Some(Arch::Arm64));
        assert_eq!(Subdir::Win32.arch(), Some(Arch::X86));
        assert_eq!(Subdir::Win64.arch(), Some(Arch::X86_64));
        assert_eq!(Subdir::WinArm64.arch(), Some(Arch::Arm64));
        assert_eq!(Subdir::EmscriptenWasm32.arch(), Some(Arch::Wasm32));
        assert_eq!(Subdir::EmscriptenWasm64.arch(), Some(Arch::Wasm64));
        assert_eq!(Subdir::WasiWasm32.arch(), Some(Arch::Wasm32));
        assert_eq!(Subdir::NoArch.arch(), None);
        assert_eq!(Subdir::ZosZ.arch(), Some(Arch::Z));

        // Every known arch is reachable by name, and unknown arches parse too.
        for arch in Arch::known() {
            assert_eq!(arch.as_str().parse::<Arch>().unwrap(), arch);
            assert!(arch.is_known());
        }
        let xtensa: Arch = "xtensa".parse().unwrap();
        assert!(!xtensa.is_known());
        assert_matches!("Xtensa".parse::<Arch>(), Err(ParseArchError { .. }));
    }

    #[test]
    fn test_cep26_subdir_names() {
        // `noarch` and every known subdir satisfy the CEP 26 syntax.
        assert!(is_valid_subdir_name("noarch"));
        for subdir in Subdir::known() {
            assert!(
                is_valid_subdir_name(subdir.as_str()),
                "'{subdir}' is not a valid CEP 26 subdir name"
            );
        }

        // Subdirs rattler has no built-in knowledge of are still valid names.
        assert!(is_valid_subdir_name("linux-esp32s3"));
        assert!(is_valid_subdir_name("espidf-xtensa"));

        // Uppercase, underscores and other separators are not allowed, both
        // parts must be present and non-empty, and only a single dash may
        // separate them.
        assert!(!is_valid_subdir_name("Linux-64"));
        assert!(!is_valid_subdir_name("linux_64"));
        assert!(!is_valid_subdir_name("linux-x86_64"));
        assert!(!is_valid_subdir_name("linux"));
        assert!(!is_valid_subdir_name("linux-"));
        assert!(!is_valid_subdir_name("-64"));
        assert!(!is_valid_subdir_name("linux-64-extra"));
        assert!(!is_valid_subdir_name(""));

        // A subdir name is at most 32 characters.
        assert!(is_valid_subdir_name(&format!("linux-{}", "a".repeat(26))));
        assert!(!is_valid_subdir_name(&format!("linux-{}", "a".repeat(27))));
    }

    #[test]
    fn test_unknown_subdirs_parse() {
        // A subdir for an architecture rattler has never heard of still parses
        // and still answers the questions that follow from its name.
        let subdir: Subdir = "linux-esp32s3".parse().unwrap();
        assert!(!subdir.is_known());
        assert_eq!(subdir.to_string(), "linux-esp32s3");
        assert_eq!(subdir.only_platform(), Some("linux"));
        assert_eq!(
            subdir.arch().map(|arch| arch.to_string()),
            Some("esp32s3".to_string())
        );
        assert!(subdir.is_linux());
        assert!(subdir.is_unix());
        assert!(!subdir.is_windows());

        // An os rattler does not know is not assumed to be unix-like, because
        // its libc and path conventions are unknown.
        let subdir: Subdir = "espidf-xtensa".parse().unwrap();
        assert_eq!(subdir.only_platform(), Some("espidf"));
        assert!(!subdir.is_unix());
        assert!(!subdir.is_linux());

        // Round-trips through serde like any other subdir.
        let json = serde_json::to_string(&subdir).unwrap();
        assert_eq!(json, "\"espidf-xtensa\"");
        assert_eq!(serde_json::from_str::<Subdir>(&json).unwrap(), subdir);

        // Names that violate CEP 26 are still rejected.
        assert_matches!("Linux-64".parse::<Subdir>(), Err(ParseSubdirError { .. }));
        assert_matches!("unknown".parse::<Subdir>(), Err(ParseSubdirError { .. }));
        assert_matches!(Subdir::try_from("nope"), Err(ParseSubdirError { .. }));
    }

    #[test]
    fn test_known_str_does_not_swallow_channel_names() {
        // `conda-forge` satisfies the CEP 26 subdir syntax, so anything that
        // probes a path segment to see whether it is a subdir has to use
        // `from_known_str`, not `FromStr`.
        assert!("conda-forge".parse::<Subdir>().is_ok());
        assert_eq!(Subdir::from_known_str("conda-forge"), None);
        assert_eq!(Subdir::from_known_str("linux-64"), Some(Subdir::Linux64));
        assert_eq!(Subdir::from_known_str("linux-esp32s3"), None);
    }

    #[test]
    fn test_subdirs_are_small_and_copy() {
        // A subdir is stored inline; it never allocates and stays cheap to
        // pass by value.
        assert_eq!(std::mem::size_of::<Subdir>(), MAX_SUBDIR_LEN);
        assert_eq!(std::mem::size_of::<Option<Subdir>>(), MAX_SUBDIR_LEN);
    }
}
