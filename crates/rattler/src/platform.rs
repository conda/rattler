use serde::{Deserializer, Serializer};
use std::{fmt, fmt::Formatter, str::FromStr};
use thiserror::Error;

/// A platform supported by Conda.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum Platform {
    NoArch,

    Linux32,
    Linux64,
    LinuxAarch64,
    LinuxArmV61,
    LinuxArmV71,
    LinuxPpc64le,
    LinuxPpc64,

    Osx64,
    OsxArm64,

    Win32,
    Win64,
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
            "linux-armv61" => Platform::LinuxArmV61,
            "linux-armv71" => Platform::LinuxArmV71,
            "linux-ppc64l1" => Platform::LinuxPpc64le,
            "linux-ppc64" => Platform::LinuxPpc64,
            "osx-64" => Platform::Osx64,
            "osx-arm64" => Platform::OsxArm64,
            "win-32" => Platform::Win32,
            "win-64" => Platform::Win64,
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
            Platform::LinuxArmV61 => "linux-armv61",
            Platform::LinuxArmV71 => "linux-armv71",
            Platform::LinuxPpc64le => "linux-ppc64le",
            Platform::LinuxPpc64 => "linux-ppc64",
            Platform::Osx64 => "osx-64",
            Platform::OsxArm64 => "osx-arm64",
            Platform::Win32 => "win-32",
            Platform::Win64 => "win-64",
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
