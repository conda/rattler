//! Platform-specific code.
use std::{
    cmp::Ordering,
    fmt,
    fmt::{Display, Formatter},
    str::FromStr,
};

use itertools::Itertools;
use serde::{Deserializer, Serializer};
use strum::{EnumIter, IntoEnumIterator};
use thiserror::Error;

/// A platform supported by Conda.
#[allow(missing_docs)]
#[non_exhaustive] // The `Platform` enum is non-exhaustive to allow for future extensions without breaking changes.
#[derive(EnumIter, Debug, Clone, Eq, PartialEq, Hash)]
pub enum Platform {
    NoArch,
    Unknown,

    Linux32,
    Linux64,
    LinuxAarch64,
    LinuxArmV6l,
    LinuxArmV7l,
    LinuxLoong64,
    LinuxPpc64le,
    LinuxPpc64,
    LinuxPpc,
    LinuxS390X,
    LinuxRiscv32,
    LinuxRiscv64,

    FreeBsd64,

    Osx64,
    OsxArm64,

    Win32,
    Win64,
    WinArm64,

    EmscriptenWasm32,
    WasiWasm32,

    ZosZ,

    /// A custom platform string not in the predefined list.
    /// This allows users to define arbitrary platforms like "foobar-64".
    #[strum(disabled)]
    Custom(Box<str>),
}

impl PartialOrd for Platform {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Platform {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Platform::Custom(a), Platform::Custom(b)) => a.cmp(b),
            (Platform::Custom(a), other) => a.as_ref().cmp(other.as_str()),
            (this, Platform::Custom(b)) => this.as_str().cmp(b.as_ref()),
            (a, b) => a.as_str().cmp(b.as_str()),
        }
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
    Loong64,
    Ppc64le,
    Ppc64,
    Ppc,
    S390X,
    Riscv32,
    Riscv64,
    Wasm32,
    Z,
}

impl Platform {
    /// Returns the platform for which the current binary was build.
    pub const fn current() -> Platform {
        #[cfg(target_os = "linux")]
        {
            #[cfg(target_arch = "x86")]
            return Platform::Linux32;

            #[cfg(target_arch = "x86_64")]
            return Platform::Linux64;

            #[cfg(target_arch = "aarch64")]
            return Platform::LinuxAarch64;

            #[cfg(target_arch = "arm")]
            {
                #[cfg(target_feature = "v7")]
                return Platform::LinuxArmV7l;

                #[cfg(not(target_feature = "v7"))]
                return Platform::LinuxArmV6l;
            }

            #[cfg(target_arch = "loongarch64")]
            return Platform::LinuxLoong64;

            #[cfg(all(target_arch = "powerpc64", target_endian = "little"))]
            return Platform::LinuxPpc64le;

            #[cfg(all(target_arch = "powerpc64", target_endian = "big"))]
            return Platform::LinuxPpc64;

            #[cfg(target_arch = "powerpc")]
            return Platform::LinuxPpc;

            #[cfg(target_arch = "s390x")]
            return Platform::LinuxS390X;

            #[cfg(target_arch = "riscv32")]
            return Platform::LinuxRiscv32;

            #[cfg(target_arch = "riscv64")]
            return Platform::LinuxRiscv64;

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
            #[cfg(target_arch = "x86_64")]
            return Platform::FreeBsd64;

            #[cfg(not(target_arch = "x86_64"))]
            compile_error!("unsupported freebsd architecture");
        }
        #[cfg(windows)]
        {
            #[cfg(target_arch = "x86")]
            return Platform::Win32;

            #[cfg(target_arch = "x86_64")]
            return Platform::Win64;

            #[cfg(target_arch = "aarch64")]
            return Platform::WinArm64;

            #[cfg(not(any(target_arch = "x86_64", target_arch = "x86", target_arch = "aarch64")))]
            compile_error!("unsupported windows architecture");
        }
        #[cfg(target_os = "macos")]
        {
            #[cfg(target_arch = "x86_64")]
            return Platform::Osx64;

            #[cfg(target_arch = "aarch64")]
            return Platform::OsxArm64;
        }

        #[cfg(target_os = "emscripten")]
        {
            #[cfg(target_arch = "wasm32")]
            return Platform::EmscriptenWasm32;
        }

        #[cfg(target_os = "wasi")]
        {
            #[cfg(target_arch = "wasm32")]
            return Platform::WasiWasm32;
        }

        #[cfg(not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "emscripten",
            target_os = "wasi",
            windows
        )))]
        {
            return Platform::Unknown;
        }
    }

    /// Returns a string representation of the platform.
    pub fn as_str(&self) -> &str {
        match self {
            Platform::Custom(s) => s.as_ref(),
            _ => self.static_str(),
        }
    }

    /// Returns a static string representation of the platform.
    /// Returns None for custom platforms.
    const fn static_str(&self) -> &'static str {
        match self {
            Platform::NoArch => "noarch",
            Platform::Linux32 => "linux-32",
            Platform::Linux64 => "linux-64",
            Platform::LinuxAarch64 => "linux-aarch64",
            Platform::LinuxArmV6l => "linux-armv6l",
            Platform::LinuxArmV7l => "linux-armv7l",
            Platform::LinuxLoong64 => "linux-loong64",
            Platform::LinuxPpc64le => "linux-ppc64le",
            Platform::LinuxPpc64 => "linux-ppc64",
            Platform::LinuxPpc => "linux-ppc",
            Platform::LinuxS390X => "linux-s390x",
            Platform::LinuxRiscv32 => "linux-riscv32",
            Platform::LinuxRiscv64 => "linux-riscv64",
            Platform::FreeBsd64 => "freebsd-64",
            Platform::Osx64 => "osx-64",
            Platform::OsxArm64 => "osx-arm64",
            Platform::Win32 => "win-32",
            Platform::Win64 => "win-64",
            Platform::WinArm64 => "win-arm64",
            Platform::EmscriptenWasm32 => "emscripten-wasm32",
            Platform::WasiWasm32 => "wasi-wasm32",
            Platform::ZosZ => "zos-z",
            Platform::Unknown => "unknown",
            Platform::Custom(_) => "custom",
        }
    }

    /// Iterate over all Platform variants
    pub fn all() -> impl Iterator<Item = Self> {
        Platform::iter()
    }

    /// Returns true if the platform is a windows based platform.
    pub fn is_windows(&self) -> bool {
        matches!(self, Platform::Win32 | Platform::Win64 | Platform::WinArm64)
    }

    /// Returns true if the platform is a unix based platform.
    pub fn is_unix(&self) -> bool {
        self.is_linux()
            || self.is_osx()
            || matches!(self, Platform::EmscriptenWasm32 | Platform::FreeBsd64)
    }

    /// Returns true if the platform is a linux based platform.
    pub fn is_linux(&self) -> bool {
        matches!(
            self,
            Platform::Linux32
                | Platform::Linux64
                | Platform::LinuxAarch64
                | Platform::LinuxArmV6l
                | Platform::LinuxArmV7l
                | Platform::LinuxLoong64
                | Platform::LinuxPpc64le
                | Platform::LinuxPpc64
                | Platform::LinuxPpc
                | Platform::LinuxS390X
                | Platform::LinuxRiscv32
                | Platform::LinuxRiscv64
        )
    }

    /// Returns true if the platform is an macOS based platform.
    pub fn is_osx(&self) -> bool {
        matches!(self, Platform::Osx64 | Platform::OsxArm64)
    }

    /// Return only the platform (linux, win, or osx from the platform enum)
    pub fn only_platform(&self) -> Option<&str> {
        match self {
            Platform::NoArch | Platform::Unknown => None,
            Platform::Linux32
            | Platform::Linux64
            | Platform::LinuxAarch64
            | Platform::LinuxArmV6l
            | Platform::LinuxArmV7l
            | Platform::LinuxLoong64
            | Platform::LinuxPpc64le
            | Platform::LinuxPpc64
            | Platform::LinuxPpc
            | Platform::LinuxS390X
            | Platform::LinuxRiscv32
            | Platform::LinuxRiscv64 => Some("linux"),
            Platform::FreeBsd64 => Some("freebsd"),
            Platform::Osx64 | Platform::OsxArm64 => Some("osx"),
            Platform::Win32 | Platform::Win64 | Platform::WinArm64 => Some("win"),
            Platform::EmscriptenWasm32 => Some("emscripten"),
            Platform::WasiWasm32 => Some("wasi"),
            Platform::ZosZ => Some("zos"),
            // For custom platforms, try to extract the OS part (before the hyphen)
            Platform::Custom(s) => s.split('-').next(),
        }
    }
}

/// An error that can occur when parsing a platform from a string.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub struct ParsePlatformError {
    /// The platform string that could not be parsed.
    pub string: String,
}

impl Display for ParsePlatformError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "'{}' is not a known platform. Valid platforms are {}",
            self.string,
            Platform::all().map(|p| format!("'{p}'")).join(", ")
        )
    }
}

impl FromStr for Platform {
    type Err = ParsePlatformError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "noarch" => Platform::NoArch,
            "linux-32" => Platform::Linux32,
            "linux-64" => Platform::Linux64,
            "linux-aarch64" => Platform::LinuxAarch64,
            "linux-armv6l" => Platform::LinuxArmV6l,
            "linux-armv7l" => Platform::LinuxArmV7l,
            "linux-loong64" => Platform::LinuxLoong64,
            "linux-ppc64le" => Platform::LinuxPpc64le,
            "linux-ppc64" => Platform::LinuxPpc64,
            "linux-ppc" => Platform::LinuxPpc,
            "linux-s390x" => Platform::LinuxS390X,
            "linux-riscv32" => Platform::LinuxRiscv32,
            "linux-riscv64" => Platform::LinuxRiscv64,
            "freebsd-64" => Platform::FreeBsd64,
            "osx-64" => Platform::Osx64,
            "osx-arm64" => Platform::OsxArm64,
            "win-32" => Platform::Win32,
            "win-64" => Platform::Win64,
            "win-arm64" => Platform::WinArm64,
            "emscripten-wasm32" => Platform::EmscriptenWasm32,
            "wasi-wasm32" => Platform::WasiWasm32,
            "zos-z" => Platform::ZosZ,
            // Accept any other string as a custom platform
            string => Platform::Custom(string.into()),
        })
    }
}


impl Platform {
    /// Return the arch string for the platform
    /// The arch is usually the part after the `-` of the platform string.
    /// Only for 32 and 64 bit platforms the arch is `x86` and `x86_64`
    /// respectively.
    pub fn arch(&self) -> Option<Arch> {
        match self {
            Platform::Unknown | Platform::NoArch => None,
            Platform::LinuxArmV6l => Some(Arch::ArmV6l),
            Platform::LinuxArmV7l => Some(Arch::ArmV7l),
            Platform::LinuxLoong64 => Some(Arch::Loong64),
            Platform::LinuxPpc64le => Some(Arch::Ppc64le),
            Platform::LinuxPpc64 => Some(Arch::Ppc64),
            Platform::LinuxPpc => Some(Arch::Ppc),
            Platform::LinuxS390X => Some(Arch::S390X),
            Platform::LinuxRiscv32 => Some(Arch::Riscv32),
            Platform::LinuxRiscv64 => Some(Arch::Riscv64),
            Platform::Linux32 | Platform::Win32 => Some(Arch::X86),
            Platform::Linux64 | Platform::Win64 | Platform::Osx64 | Platform::FreeBsd64 => {
                Some(Arch::X86_64)
            }
            Platform::LinuxAarch64 => Some(Arch::Aarch64),
            Platform::WinArm64 | Platform::OsxArm64 => Some(Arch::Arm64),
            Platform::EmscriptenWasm32 | Platform::WasiWasm32 => Some(Arch::Wasm32),
            Platform::ZosZ => Some(Arch::Z),
            // For custom platforms, try to parse the architecture part (after the hyphen)
            Platform::Custom(s) => {
                s.split('-').nth(1).and_then(|arch| arch.parse::<Arch>().ok())
            }
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl serde::Serialize for Platform {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Platform {
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
    /// Returns the current arch.
    pub fn current() -> Self {
        // this cannot be `noarch` so unwrap is fine
        Platform::current().arch().unwrap()
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
            "loong64" => Arch::Loong64,
            "ppc64le" => Arch::Ppc64le,
            "ppc64" => Arch::Ppc64,
            "ppc" => Arch::Ppc,
            "s390x" => Arch::S390X,
            "riscv32" => Arch::Riscv32,
            "riscv64" => Arch::Riscv64,
            "wasm32" => Arch::Wasm32,
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
            Arch::Loong64 => "loong64",
            Arch::Ppc64le => "ppc64le",
            Arch::Ppc64 => "ppc64",
            Arch::Ppc => "ppc",
            Arch::S390X => "s390x",
            Arch::Riscv32 => "riscv32",
            Arch::Riscv64 => "riscv64",
            Arch::Wasm32 => "wasm32",
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
    use super::*;

    #[test]
    fn test_parse_platform() {
        assert_eq!("linux-64".parse::<Platform>().unwrap(), Platform::Linux64);
        assert_eq!("linux-32".parse::<Platform>().unwrap(), Platform::Linux32);
        assert_eq!(
            "linux-aarch64".parse::<Platform>().unwrap(),
            Platform::LinuxAarch64
        );
        assert_eq!(
            "linux-armv6l".parse::<Platform>().unwrap(),
            Platform::LinuxArmV6l
        );
        assert_eq!(
            "freebsd-64".parse::<Platform>().unwrap(),
            Platform::FreeBsd64
        );
        assert_eq!("win-arm64".parse::<Platform>().unwrap(), Platform::WinArm64);
        assert_eq!(
            "emscripten-wasm32".parse::<Platform>().unwrap(),
            Platform::EmscriptenWasm32
        );
        assert_eq!(
            "wasi-wasm32".parse::<Platform>().unwrap(),
            Platform::WasiWasm32
        );
        assert_eq!("noarch".parse::<Platform>().unwrap(), Platform::NoArch);
        assert_eq!("zos-z".parse::<Platform>().unwrap(), Platform::ZosZ);
    }

    #[test]
    fn test_parse_platform_unknown_becomes_custom() {
        // Unknown platforms are now parsed as custom platforms
        let platform = "foo".parse::<Platform>().unwrap();
        assert!(matches!(platform, Platform::Custom(_)));
        assert_eq!(platform.as_str(), "foo");
    }

    #[test]
    fn test_display() {
        assert_eq!(Platform::Linux64.to_string(), "linux-64");
        assert_eq!(Platform::Linux32.to_string(), "linux-32");
        assert_eq!(Platform::LinuxAarch64.to_string(), "linux-aarch64");
        assert_eq!(Platform::ZosZ.to_string(), "zos-z");
    }

    #[test]
    fn test_arch() {
        assert_eq!(Platform::Linux64.arch(), Some(Arch::X86_64));
        assert_eq!(Platform::Linux32.arch(), Some(Arch::X86));
        assert_eq!(Platform::LinuxAarch64.arch(), Some(Arch::Aarch64));
        assert_eq!(Platform::LinuxArmV6l.arch(), Some(Arch::ArmV6l));
        assert_eq!(Platform::LinuxArmV7l.arch(), Some(Arch::ArmV7l));
        assert_eq!(Platform::LinuxLoong64.arch(), Some(Arch::Loong64));
        assert_eq!(Platform::LinuxPpc64le.arch(), Some(Arch::Ppc64le));
        assert_eq!(Platform::LinuxPpc64.arch(), Some(Arch::Ppc64));
        assert_eq!(Platform::LinuxPpc.arch(), Some(Arch::Ppc));
        assert_eq!(Platform::LinuxS390X.arch(), Some(Arch::S390X));
        assert_eq!(Platform::LinuxRiscv32.arch(), Some(Arch::Riscv32));
        assert_eq!(Platform::LinuxRiscv64.arch(), Some(Arch::Riscv64));
        assert_eq!(Platform::FreeBsd64.arch(), Some(Arch::X86_64));
        assert_eq!(Platform::Osx64.arch(), Some(Arch::X86_64));
        assert_eq!(Platform::OsxArm64.arch(), Some(Arch::Arm64));
        assert_eq!(Platform::Win32.arch(), Some(Arch::X86));
        assert_eq!(Platform::Win64.arch(), Some(Arch::X86_64));
        assert_eq!(Platform::WinArm64.arch(), Some(Arch::Arm64));
        assert_eq!(Platform::EmscriptenWasm32.arch(), Some(Arch::Wasm32));
        assert_eq!(Platform::WasiWasm32.arch(), Some(Arch::Wasm32));
        assert_eq!(Platform::NoArch.arch(), None);
        assert_eq!(Platform::ZosZ.arch(), Some(Arch::Z));
    }

    #[test]
    fn test_custom_platform() {
        // Parse a custom platform
        let platform: Platform = "foobar-64".parse().unwrap();
        assert!(matches!(platform, Platform::Custom(_)));
        assert_eq!(platform.as_str(), "foobar-64");
        assert_eq!(platform.to_string(), "foobar-64");

        // Test with another custom platform
        let platform2: Platform = "myos-arm64".parse().unwrap();
        assert!(matches!(platform2, Platform::Custom(_)));
        assert_eq!(platform2.as_str(), "myos-arm64");

        // Test only_platform extraction
        assert_eq!(platform.only_platform(), Some("foobar"));
        assert_eq!(platform2.only_platform(), Some("myos"));

        // Test arch extraction for custom platforms
        let custom_with_known_arch: Platform = "myos-x86_64".parse().unwrap();
        assert_eq!(custom_with_known_arch.arch(), Some(Arch::X86_64));

        let custom_with_arm64: Platform = "myos-arm64".parse().unwrap();
        assert_eq!(custom_with_arm64.arch(), Some(Arch::Arm64));

        // Custom platform with unknown arch should return None
        let custom_unknown_arch: Platform = "myos-unknownarch".parse().unwrap();
        assert_eq!(custom_unknown_arch.arch(), None);
    }

    #[test]
    fn test_custom_platform_helpers() {
        let platform: Platform = "foobar-64".parse().unwrap();

        // Custom platforms should return false for all helper methods
        assert!(!platform.is_windows());
        assert!(!platform.is_unix());
        assert!(!platform.is_linux());
        assert!(!platform.is_osx());
    }

    #[test]
    fn test_custom_platform_serde() {
        // Test serialization
        let platform: Platform = "foobar-64".parse().unwrap();
        let serialized = serde_json::to_string(&platform).unwrap();
        assert_eq!(serialized, "\"foobar-64\"");

        // Test deserialization
        let deserialized: Platform = serde_json::from_str("\"foobar-64\"").unwrap();
        assert_eq!(deserialized, platform);
        assert_eq!(deserialized.as_str(), "foobar-64");
    }

    #[test]
    fn test_custom_platform_ordering() {
        let platform1: Platform = "aaa-64".parse().unwrap();
        let platform2: Platform = "bbb-64".parse().unwrap();
        let platform3: Platform = Platform::Linux64;

        assert!(platform1 < platform2);
        assert!(platform1 < platform3);
    }
}
