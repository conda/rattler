use serde::{Deserialize, Deserializer, Serializer};
use std::{fmt, fmt::Formatter, str::FromStr};
use strum::{EnumIter, IntoEnumIterator};
use thiserror::Error;

/// A platform supported by Conda.
#[derive(EnumIter, Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum Platform {
    NoArch,

    Linux32,
    Linux64,
    LinuxAarch64,
    LinuxArmV6l,
    LinuxArmV7l,
    LinuxPpc64le,
    LinuxPpc64,
    LinuxS390X,

    Osx64,
    OsxArm64,

    Win32,
    Win64,

    Emscripten32,
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

            #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
            compile_error!("unsupported linux architecture");
        }
        #[cfg(windows)]
        {
            #[cfg(target_arch = "x86")]
            return Platform::Win32;

            #[cfg(target_arch = "x86_64")]
            return Platform::Win64;

            #[cfg(not(any(target_arch = "x86_64", target_arch = "x86")))]
            compile_error!("unsupported windows architecture");
        }
        #[cfg(target_os = "macos")]
        {
            #[cfg(target_arch = "x86_64")]
            return Platform::Osx64;

            #[cfg(target_arch = "aarch64")]
            return Platform::OsxArm64;
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        compile_error!("unsupported target os");
    }

    /// Returns a string representation of the platform.
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// Iterate over all Platform variants
    pub fn all() -> impl Iterator<Item = Self> {
        Platform::iter()
    }

    /// Returns true if the platform is a windows based platform.
    pub const fn is_windows(self) -> bool {
        matches!(self, Platform::Win32 | Platform::Win64)
    }

    /// Returns true if the platform is a unix based platform.
    pub const fn is_unix(self) -> bool {
        self.is_linux() || self.is_osx()
    }

    /// Returns true if the platform is a linux based platform.
    pub const fn is_linux(self) -> bool {
        matches!(
            self,
            Platform::Linux32
                | Platform::Linux64
                | Platform::LinuxAarch64
                | Platform::LinuxArmV6l
                | Platform::LinuxArmV7l
                | Platform::LinuxPpc64le
                | Platform::LinuxPpc64
                | Platform::LinuxS390X
        )
    }

    /// Returns true if the platform is an macOS based platform.
    pub const fn is_osx(self) -> bool {
        matches!(self, Platform::Osx64 | Platform::OsxArm64)
    }
}

/// An error that can occur when parsing a platform from a string.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
#[error("'{string}' is not a known platform")]
pub struct ParsePlatformError {
    pub string: String,
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
            "linux-ppc64le" => Platform::LinuxPpc64le,
            "linux-ppc64" => Platform::LinuxPpc64,
            "linux-s390x" => Platform::LinuxS390X,
            "osx-64" => Platform::Osx64,
            "osx-arm64" => Platform::OsxArm64,
            "win-32" => Platform::Win32,
            "win-64" => Platform::Win64,
            "emscripten-32" => Platform::Emscripten32,
            string => {
                return Err(ParsePlatformError {
                    string: string.to_owned(),
                })
            }
        })
    }
}

impl From<Platform> for &'static str {
    fn from(platform: Platform) -> Self {
        match platform {
            Platform::NoArch => "noarch",
            Platform::Linux32 => "linux-32",
            Platform::Linux64 => "linux-64",
            Platform::LinuxAarch64 => "linux-aarch64",
            Platform::LinuxArmV6l => "linux-armv6l",
            Platform::LinuxArmV7l => "linux-armv7l",
            Platform::LinuxPpc64le => "linux-ppc64le",
            Platform::LinuxPpc64 => "linux-ppc64",
            Platform::LinuxS390X => "linux-s390x",
            Platform::Osx64 => "osx-64",
            Platform::OsxArm64 => "osx-arm64",
            Platform::Win32 => "win-32",
            Platform::Win64 => "win-64",
            Platform::Emscripten32 => "emscripten-32",
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
