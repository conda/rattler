use rattler_conda_types::{compression_level::CompressionLevel, package::ArchiveType};
use serde::{de::Error, Deserialize, Serialize};
use std::str::FromStr;

use crate::config::{Config, MergeError, ValidationError};

/// Container for the package format and compression level
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PackageFormatAndCompression {
    /// The archive type that is selected
    pub archive_type: ArchiveType,
    /// The compression level that is selected
    pub compression_level: CompressionLevel,
}

#[derive(Clone, Debug, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct BuildConfig {
    /// package format and compression level
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_format: Option<PackageFormatAndCompression>,
}

// deserializer for the package format and compression level
impl<'de> Deserialize<'de> for PackageFormatAndCompression {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let s = s.as_str();
        PackageFormatAndCompression::from_str(s).map_err(D::Error::custom)
    }
}

impl FromStr for PackageFormatAndCompression {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut split = s.split(':');
        let package_format = split.next().ok_or("invalid")?;

        let compression = split.next().unwrap_or("default");

        // remove all non-alphanumeric characters
        let package_format = package_format
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>();

        let archive_type = match package_format.to_lowercase().as_str() {
            "tarbz2" => ArchiveType::TarBz2,
            "conda" => ArchiveType::Conda,
            _ => return Err(format!("Unknown package format: {package_format}")),
        };

        let compression_level = match compression {
            "max" | "highest" => CompressionLevel::Highest,
            "default" | "normal" => CompressionLevel::Default,
            "fast" | "lowest" | "min" => CompressionLevel::Lowest,
            number if number.parse::<i32>().is_ok() => {
                let number = number.parse::<i32>().unwrap_or_default();
                match archive_type {
                    ArchiveType::TarBz2 => {
                        if !(1..=9).contains(&number) {
                            return Err("Compression level for .tar.bz2 must be between 1 and 9"
                                .to_string());
                        }
                    }
                    ArchiveType::Conda => {
                        if !(-7..=22).contains(&number) {
                            return Err(
                                "Compression level for conda packages (zstd) must be between -7 and 22".to_string()
                            );
                        }
                    }
                    ArchiveType::Whl => {
                        // TODO: put correct handling of wheel file here
                    }
                }
                CompressionLevel::Numeric(number)
            }
            _ => return Err(format!("Unknown compression level: {compression}")),
        };

        Ok(PackageFormatAndCompression {
            archive_type,
            compression_level,
        })
    }
}

impl Serialize for PackageFormatAndCompression {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let package_format = match self.archive_type {
            ArchiveType::TarBz2 => "tarbz2",
            ArchiveType::Conda => "conda",
            ArchiveType::Whl => "whl",
        };
        let compression_level = match self.compression_level {
            CompressionLevel::Default => "default",
            CompressionLevel::Highest => "max",
            CompressionLevel::Lowest => "min",
            CompressionLevel::Numeric(level) => &level.to_string(),
        };

        serializer.serialize_str(format!("{package_format}:{compression_level}").as_str())
    }
}

impl Config for BuildConfig {
    fn get_extension_name(&self) -> String {
        "build".to_string()
    }

    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        Ok(Self {
            package_format: other
                .package_format
                .as_ref()
                .or(self.package_format.as_ref())
                .cloned(),
        })
    }

    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        vec!["package_format".to_string()]
    }
}
