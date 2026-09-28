//! Subdir-specific code.
use std::{cmp::Ordering, fmt, fmt::Formatter, str::FromStr};

use itertools::Itertools;
use serde::{Deserializer, Serializer};
use strum::{EnumIter, IntoEnumIterator};
use thiserror::Error;

/// A channel subdir, as defined by CEP 26: either `noarch` or `{os}-{arch}`.
#[allow(missing_docs)]
#[non_exhaustive] // The `Subdir` enum is non-exhaustive to allow for future extensions without breaking changes.
#[derive(EnumIter, Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum Subdir {
    NoArch,

    Linux32,
    Linux64,
    LinuxAarch64,
    LinuxArmV6l,
    LinuxArmV7l,
    LinuxLoongArch64,
    LinuxPpc64le,
    LinuxPpc64,
    LinuxPpc,
    LinuxS390X,
    LinuxRiscv32,
    LinuxRiscv64,

    FreeBsd32,
    FreeBsd64,
    FreeBsdArm64,

    Osx64,
    OsxArm64,

    // iOS and Android map the PyPI wheel-tag model (PEP 730/738) onto conda:
    // a wheel tag like `ios_13_0_arm64_iphoneos` packs arch, ABI and
    // minimum OS version into one string. Conda splits these axes: arch + ABI
    // become the subdir, while the minimum OS version (min iOS version /
    // Android API level) is expressed through the `__ios`/`__android` virtual
    // packages, so version compatibility is handled by the solver just like
    // `__osx` and `__glibc`.
    //
    // Each subdir is single-arch by construction and keeps to a single
    // `<os>-<arch>` dash so tools that split the subdir on `-` keep working;
    // the device/simulator split is folded into the os token
    // (`iossimulator`).
    IosArm64,
    IosSimulatorArm64,
    IosSimulator64,

    // Android runs a Linux kernel but links against Bionic instead of glibc,
    // which is why `android-*` gets its own subdirs and is deliberately not
    // `is_linux()`: `linux-*` packages declare their libc requirement via
    // `__glibc`, a constraint Bionic cannot satisfy, so mixing the two
    // subdirs would install packages whose libc requirement is silently
    // violated.
    AndroidAarch64,
    AndroidArmV7a,
    Android64,
    Android32,

    Win32,
    Win64,
    WinArm64,

    EmscriptenWasm32,
    EmscriptenWasm64,
    WasiWasm32,

    ZosZ,
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

/// Known architectures supported by Conda.
#[allow(missing_docs)]
#[non_exhaustive] // The `Arch` enum is non-exhaustive to allow for future extensions without breaking changes.
#[derive(EnumIter, Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum Arch {
    X86,
    X86_64,
    // aarch64 is only used for linux
    Aarch64,
    // for historical reasons we also need `arm64` for win-arm64 and osx-arm64
    Arm64,
    ArmV6l,
    ArmV7l,
    // armv7a is used for Android's `armeabi-v7a` ABI. It is distinct from
    // `armv7l` (the `uname -m` value used for `linux-armv7l`): both are
    // 32-bit ARMv7, but `armeabi-v7a` uses Android's softfp calling
    // convention and links against Bionic instead of glibc, so binaries are
    // not interchangeable between the two.
    ArmV7a,
    LoongArch64,
    Ppc64le,
    Ppc64,
    Ppc,
    S390X,
    Riscv32,
    Riscv64,
    Wasm32,
    Wasm64,
    Z,
}

impl Subdir {
    /// Returns the platform for which the current binary was built, or `None`
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

    /// Returns a string representation of the platform.
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// Iterate over all Subdir variants
    pub fn all() -> impl Iterator<Item = Self> {
        Subdir::iter()
    }

    /// Returns true if the platform is a windows based platform.
    pub const fn is_windows(self) -> bool {
        matches!(self, Subdir::Win32 | Subdir::Win64 | Subdir::WinArm64)
    }

    /// Returns true if the platform is a unix based platform.
    pub const fn is_unix(self) -> bool {
        self.is_linux()
            || self.is_osx()
            || self.is_ios()
            || self.is_android()
            || matches!(
                self,
                Subdir::EmscriptenWasm32
                    | Subdir::EmscriptenWasm64
                    | Subdir::FreeBsd32
                    | Subdir::FreeBsd64
                    | Subdir::FreeBsdArm64
            )
    }

    /// Returns true if the platform is a linux based platform.
    pub const fn is_linux(self) -> bool {
        matches!(
            self,
            Subdir::Linux32
                | Subdir::Linux64
                | Subdir::LinuxAarch64
                | Subdir::LinuxArmV6l
                | Subdir::LinuxArmV7l
                | Subdir::LinuxLoongArch64
                | Subdir::LinuxPpc64le
                | Subdir::LinuxPpc64
                | Subdir::LinuxPpc
                | Subdir::LinuxS390X
                | Subdir::LinuxRiscv32
                | Subdir::LinuxRiscv64
        )
    }

    /// Returns true if the platform is an macOS based platform.
    pub const fn is_osx(self) -> bool {
        matches!(self, Subdir::Osx64 | Subdir::OsxArm64)
    }

    /// Returns true if the platform is an iOS based platform (device or
    /// simulator).
    pub const fn is_ios(self) -> bool {
        matches!(
            self,
            Subdir::IosArm64 | Subdir::IosSimulatorArm64 | Subdir::IosSimulator64
        )
    }

    /// Returns true if the platform is an Android based platform.
    pub const fn is_android(self) -> bool {
        matches!(
            self,
            Subdir::AndroidAarch64 | Subdir::AndroidArmV7a | Subdir::Android64 | Subdir::Android32
        )
    }

    /// Return only the OS part of the platform (e.g. `linux`, `win`, `osx`,
    /// `freebsd`, `ios`, `iossimulator`, `android`), or `None` for `noarch`
    /// and unknown platforms.
    pub fn only_platform(&self) -> Option<&str> {
        match self {
            Subdir::NoArch => None,
            Subdir::Linux32
            | Subdir::Linux64
            | Subdir::LinuxAarch64
            | Subdir::LinuxArmV6l
            | Subdir::LinuxArmV7l
            | Subdir::LinuxLoongArch64
            | Subdir::LinuxPpc64le
            | Subdir::LinuxPpc64
            | Subdir::LinuxPpc
            | Subdir::LinuxS390X
            | Subdir::LinuxRiscv32
            | Subdir::LinuxRiscv64 => Some("linux"),
            Subdir::FreeBsd32 | Subdir::FreeBsd64 | Subdir::FreeBsdArm64 => Some("freebsd"),
            Subdir::Osx64 | Subdir::OsxArm64 => Some("osx"),
            Subdir::IosArm64 => Some("ios"),
            Subdir::IosSimulatorArm64 | Subdir::IosSimulator64 => Some("iossimulator"),
            Subdir::AndroidAarch64
            | Subdir::AndroidArmV7a
            | Subdir::Android64
            | Subdir::Android32 => Some("android"),
            Subdir::Win32 | Subdir::Win64 | Subdir::WinArm64 => Some("win"),
            Subdir::EmscriptenWasm32 | Subdir::EmscriptenWasm64 => Some("emscripten"),
            Subdir::WasiWasm32 => Some("wasi"),
            Subdir::ZosZ => Some("zos"),
        }
    }
}

/// The maximum length of a subdir name, as specified by CEP 26.
const MAX_SUBDIR_LEN: usize = 32;

/// Returns whether `s` is a syntactically valid subdir name according to
/// [CEP 26](https://github.com/conda/ceps/blob/main/cep-0026.md): either the
/// literal `noarch`, or `{os}-{arch}` where both parts consist of lowercase
/// ASCII letters and digits. The name must not exceed 32 characters.
pub fn is_valid_subdir_name(s: &str) -> bool {
    if s.len() > MAX_SUBDIR_LEN {
        return false;
    }
    if s == "noarch" {
        return true;
    }
    let is_lowercase_alphanumeric = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    };
    match s.split_once('-') {
        Some((os, arch)) => is_lowercase_alphanumeric(os) && is_lowercase_alphanumeric(arch),
        None => false,
    }
}

/// An error that can occur when parsing a subdir from a string.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum ParseSubdirError {
    /// The string is not a valid subdir name according to CEP 26.
    #[error(
        "'{0}' is not a valid subdir name; it must be 'noarch' or '{{os}}-{{arch}}' with both \
         parts consisting of lowercase ASCII letters and digits, at most 32 characters in total"
    )]
    InvalidName(String),

    /// The string is a valid subdir name, but not one this version knows about.
    #[error(
        "'{name}' is not a known subdir. Known subdirs are {}",
        Subdir::all().map(|subdir| format!("'{subdir}'")).join(", ")
    )]
    UnknownSubdir {
        /// The subdir name that is not known.
        name: String,
    },
}

impl FromStr for Subdir {
    type Err = ParseSubdirError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "noarch" => Subdir::NoArch,
            "linux-32" => Subdir::Linux32,
            "linux-64" => Subdir::Linux64,
            "linux-aarch64" => Subdir::LinuxAarch64,
            "linux-armv6l" => Subdir::LinuxArmV6l,
            "linux-armv7l" => Subdir::LinuxArmV7l,
            "linux-loongarch64" => Subdir::LinuxLoongArch64,
            "linux-ppc64le" => Subdir::LinuxPpc64le,
            "linux-ppc64" => Subdir::LinuxPpc64,
            "linux-ppc" => Subdir::LinuxPpc,
            "linux-s390x" => Subdir::LinuxS390X,
            "linux-riscv32" => Subdir::LinuxRiscv32,
            "linux-riscv64" => Subdir::LinuxRiscv64,
            "freebsd-32" => Subdir::FreeBsd32,
            "freebsd-64" => Subdir::FreeBsd64,
            "freebsd-arm64" => Subdir::FreeBsdArm64,
            "osx-64" => Subdir::Osx64,
            "osx-arm64" => Subdir::OsxArm64,
            "ios-arm64" => Subdir::IosArm64,
            "iossimulator-arm64" => Subdir::IosSimulatorArm64,
            "iossimulator-64" => Subdir::IosSimulator64,
            "android-aarch64" => Subdir::AndroidAarch64,
            "android-armv7a" => Subdir::AndroidArmV7a,
            "android-64" => Subdir::Android64,
            "android-32" => Subdir::Android32,
            "win-32" => Subdir::Win32,
            "win-64" => Subdir::Win64,
            "win-arm64" => Subdir::WinArm64,
            "emscripten-wasm32" => Subdir::EmscriptenWasm32,
            "emscripten-wasm64" => Subdir::EmscriptenWasm64,
            "wasi-wasm32" => Subdir::WasiWasm32,
            "zos-z" => Subdir::ZosZ,
            string if is_valid_subdir_name(string) => {
                return Err(ParseSubdirError::UnknownSubdir {
                    name: string.to_owned(),
                });
            }
            string => {
                return Err(ParseSubdirError::InvalidName(string.to_owned()));
            }
        })
    }
}

impl TryFrom<&str> for Subdir {
    type Error = ParseSubdirError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Subdir> for &'static str {
    fn from(platform: Subdir) -> Self {
        match platform {
            Subdir::NoArch => "noarch",
            Subdir::Linux32 => "linux-32",
            Subdir::Linux64 => "linux-64",
            Subdir::LinuxAarch64 => "linux-aarch64",
            Subdir::LinuxArmV6l => "linux-armv6l",
            Subdir::LinuxArmV7l => "linux-armv7l",
            Subdir::LinuxLoongArch64 => "linux-loongarch64",
            Subdir::LinuxPpc64le => "linux-ppc64le",
            Subdir::LinuxPpc64 => "linux-ppc64",
            Subdir::LinuxPpc => "linux-ppc",
            Subdir::LinuxS390X => "linux-s390x",
            Subdir::LinuxRiscv32 => "linux-riscv32",
            Subdir::LinuxRiscv64 => "linux-riscv64",
            Subdir::FreeBsd32 => "freebsd-32",
            Subdir::FreeBsd64 => "freebsd-64",
            Subdir::FreeBsdArm64 => "freebsd-arm64",
            Subdir::Osx64 => "osx-64",
            Subdir::OsxArm64 => "osx-arm64",
            Subdir::IosArm64 => "ios-arm64",
            Subdir::IosSimulatorArm64 => "iossimulator-arm64",
            Subdir::IosSimulator64 => "iossimulator-64",
            Subdir::AndroidAarch64 => "android-aarch64",
            Subdir::AndroidArmV7a => "android-armv7a",
            Subdir::Android64 => "android-64",
            Subdir::Android32 => "android-32",
            Subdir::Win32 => "win-32",
            Subdir::Win64 => "win-64",
            Subdir::WinArm64 => "win-arm64",
            Subdir::EmscriptenWasm32 => "emscripten-wasm32",
            Subdir::EmscriptenWasm64 => "emscripten-wasm64",
            Subdir::WasiWasm32 => "wasi-wasm32",
            Subdir::ZosZ => "zos-z",
        }
    }
}

impl Subdir {
    /// Return the arch string for the platform
    /// The arch is usually the part after the `-` of the platform string.
    /// Only for 32 and 64 bit platforms the arch is `x86` and `x86_64`
    /// respectively.
    pub fn arch(&self) -> Option<Arch> {
        match self {
            Subdir::NoArch => None,
            Subdir::LinuxArmV6l => Some(Arch::ArmV6l),
            Subdir::LinuxArmV7l => Some(Arch::ArmV7l),
            Subdir::LinuxLoongArch64 => Some(Arch::LoongArch64),
            Subdir::LinuxPpc64le => Some(Arch::Ppc64le),
            Subdir::LinuxPpc64 => Some(Arch::Ppc64),
            Subdir::LinuxPpc => Some(Arch::Ppc),
            Subdir::LinuxS390X => Some(Arch::S390X),
            Subdir::LinuxRiscv32 => Some(Arch::Riscv32),
            Subdir::LinuxRiscv64 => Some(Arch::Riscv64),
            Subdir::Linux32 | Subdir::Win32 | Subdir::FreeBsd32 | Subdir::Android32 => {
                Some(Arch::X86)
            }
            Subdir::Linux64
            | Subdir::Win64
            | Subdir::Osx64
            | Subdir::FreeBsd64
            | Subdir::IosSimulator64
            | Subdir::Android64 => Some(Arch::X86_64),
            Subdir::LinuxAarch64 | Subdir::AndroidAarch64 => Some(Arch::Aarch64),
            Subdir::WinArm64
            | Subdir::OsxArm64
            | Subdir::FreeBsdArm64
            | Subdir::IosArm64
            | Subdir::IosSimulatorArm64 => Some(Arch::Arm64),
            Subdir::AndroidArmV7a => Some(Arch::ArmV7a),
            Subdir::EmscriptenWasm32 | Subdir::WasiWasm32 => Some(Arch::Wasm32),
            Subdir::EmscriptenWasm64 => Some(Arch::Wasm64),
            Subdir::ZosZ => Some(Arch::Z),
        }
    }
}

impl fmt::Display for Subdir {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
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

impl Arch {
    /// Returns the arch for which the current binary was built, or `None`
    /// when [`Subdir::current`] is `None`.
    pub fn current() -> Option<Self> {
        Subdir::current().and_then(|platform| platform.arch())
    }

    /// Returns a string representation of the arch.
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// An error that can occur when parsing an arch from a string.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
#[error("'{string}' is not a known arch")]
pub struct ParseArchError {
    /// The arch string that could not be parsed.
    pub string: String,
}

impl FromStr for Arch {
    type Err = ParseArchError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "x86" => Arch::X86,
            "x86_64" => Arch::X86_64,
            "aarch64" => Arch::Aarch64,
            "arm64" => Arch::Arm64,
            "armv6l" => Arch::ArmV6l,
            "armv7l" => Arch::ArmV7l,
            "armv7a" => Arch::ArmV7a,
            "loongarch64" => Arch::LoongArch64,
            "ppc64le" => Arch::Ppc64le,
            "ppc64" => Arch::Ppc64,
            "ppc" => Arch::Ppc,
            "s390x" => Arch::S390X,
            "riscv32" => Arch::Riscv32,
            "riscv64" => Arch::Riscv64,
            "wasm32" => Arch::Wasm32,
            "wasm64" => Arch::Wasm64,
            "z" => Arch::Z,
            string => {
                return Err(ParseArchError {
                    string: string.to_owned(),
                });
            }
        })
    }
}

impl From<Arch> for &'static str {
    fn from(arch: Arch) -> Self {
        match arch {
            Arch::X86 => "x86",
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64",
            Arch::Aarch64 => "aarch64",
            Arch::ArmV6l => "armv6l",
            Arch::ArmV7l => "armv7l",
            Arch::ArmV7a => "armv7a",
            Arch::LoongArch64 => "loongarch64",
            Arch::Ppc64le => "ppc64le",
            Arch::Ppc64 => "ppc64",
            Arch::Ppc => "ppc",
            Arch::S390X => "s390x",
            Arch::Riscv32 => "riscv32",
            Arch::Riscv64 => "riscv64",
            Arch::Wasm32 => "wasm32",
            Arch::Wasm64 => "wasm64",
            Arch::Z => "z",
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
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
    fn test_parse_platform() {
        assert_eq!("linux-64".parse::<Subdir>().unwrap(), Subdir::Linux64);
        assert_eq!("linux-32".parse::<Subdir>().unwrap(), Subdir::Linux32);
        assert_eq!(
            "linux-aarch64".parse::<Subdir>().unwrap(),
            Subdir::LinuxAarch64
        );
        assert_eq!(
            "linux-armv6l".parse::<Subdir>().unwrap(),
            Subdir::LinuxArmV6l
        );
        assert_eq!("freebsd-32".parse::<Subdir>().unwrap(), Subdir::FreeBsd32);
        assert_eq!("freebsd-64".parse::<Subdir>().unwrap(), Subdir::FreeBsd64);
        assert_eq!(
            "freebsd-arm64".parse::<Subdir>().unwrap(),
            Subdir::FreeBsdArm64
        );
        assert_eq!("win-arm64".parse::<Subdir>().unwrap(), Subdir::WinArm64);
        assert_eq!(
            "emscripten-wasm32".parse::<Subdir>().unwrap(),
            Subdir::EmscriptenWasm32
        );
        assert_eq!(
            "emscripten-wasm64".parse::<Subdir>().unwrap(),
            Subdir::EmscriptenWasm64
        );
        assert_eq!("wasi-wasm32".parse::<Subdir>().unwrap(), Subdir::WasiWasm32);
        assert_eq!("noarch".parse::<Subdir>().unwrap(), Subdir::NoArch);
        assert_eq!("zos-z".parse::<Subdir>().unwrap(), Subdir::ZosZ);
        assert_eq!("ios-arm64".parse::<Subdir>().unwrap(), Subdir::IosArm64);
        assert_eq!(
            "iossimulator-arm64".parse::<Subdir>().unwrap(),
            Subdir::IosSimulatorArm64
        );
        assert_eq!(
            "iossimulator-64".parse::<Subdir>().unwrap(),
            Subdir::IosSimulator64
        );
        assert_eq!(
            "android-aarch64".parse::<Subdir>().unwrap(),
            Subdir::AndroidAarch64
        );
        assert_eq!(
            "android-armv7a".parse::<Subdir>().unwrap(),
            Subdir::AndroidArmV7a
        );
        assert_eq!("android-64".parse::<Subdir>().unwrap(), Subdir::Android64);
        assert_eq!("android-32".parse::<Subdir>().unwrap(), Subdir::Android32);
    }

    #[test]
    fn test_ios_android_platform() {
        // iOS and Android round-trip through their subdir strings.
        for subdir in [
            "ios-arm64",
            "iossimulator-arm64",
            "iossimulator-64",
            "android-aarch64",
            "android-armv7a",
            "android-64",
            "android-32",
        ] {
            let platform: Subdir = subdir.parse().unwrap();
            assert_eq!(platform.to_string(), subdir);
        }

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
    fn test_parse_platform_error() {
        let err = "foo".parse::<Subdir>().unwrap_err();
        println!("{err}");
    }

    #[test]
    fn test_display() {
        assert_eq!(Subdir::Linux64.to_string(), "linux-64");
        assert_eq!(Subdir::Linux32.to_string(), "linux-32");
        assert_eq!(Subdir::LinuxAarch64.to_string(), "linux-aarch64");
        assert_eq!(Subdir::ZosZ.to_string(), "zos-z");
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
    }

    #[test]
    fn test_cep26_subdir_names() {
        // `noarch` and every known subdir satisfy the CEP 26 syntax.
        assert!(is_valid_subdir_name("noarch"));
        for subdir in Subdir::all() {
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
    fn test_parse_subdir_errors() {
        // A well-formed subdir that this version does not know about is
        // reported separately from a name that violates CEP 26, so callers can
        // tell "not supported yet" apart from "not a subdir at all".
        assert_matches!(
            "linux-esp32s3".parse::<Subdir>(),
            Err(ParseSubdirError::UnknownSubdir { .. })
        );
        assert_matches!(
            "unknown".parse::<Subdir>(),
            Err(ParseSubdirError::InvalidName(_))
        );
        assert_matches!(
            "Linux-64".parse::<Subdir>(),
            Err(ParseSubdirError::InvalidName(_))
        );

        // `TryFrom<&str>` agrees with `FromStr`.
        assert_eq!(Subdir::try_from("linux-64").unwrap(), Subdir::Linux64);
        assert_matches!(
            Subdir::try_from("nope"),
            Err(ParseSubdirError::InvalidName(_))
        );
    }
}
