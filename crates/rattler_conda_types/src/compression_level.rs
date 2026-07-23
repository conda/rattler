//! This module defines the `CompressionLevel` enum, which is used to specify
//! the compression level for Conda packages to be created.

/// Select the compression level to use for the package
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionLevel {
    /// Use the lowest compression level (zstd: 1, bzip2: 1)
    Lowest,
    /// Use the highest compression level (zstd: 22, bzip2: 9)
    Highest,
    /// Use the default compression level (zstd: 15, bzip2: 9)
    #[default]
    Default,
    /// Use a numeric compression level (zstd: 1-22, bzip2: 1-9)
    Numeric(i32),
}

impl CompressionLevel {
    /// convert the compression level to a zstd compression level
    pub fn to_zstd_level(self) -> Result<i32, std::io::Error> {
        match self {
            CompressionLevel::Lowest => Ok(-7),
            CompressionLevel::Highest => Ok(22),
            CompressionLevel::Default => Ok(15),
            CompressionLevel::Numeric(n) => {
                if (-7..=22).contains(&n) {
                    Ok(n)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "zstd compression level must be between -7 and 22",
                    ))
                }
            }
        }
    }

    /// convert the compression level to a bzip2 compression level
    pub fn to_bzip2_level(self) -> Result<u32, std::io::Error> {
        match self {
            CompressionLevel::Lowest => Ok(1),
            CompressionLevel::Default | CompressionLevel::Highest => Ok(9),
            CompressionLevel::Numeric(n) => {
                if (1..=9).contains(&n) {
                    // this conversion from i32 to u32 cannot panic because of the check above
                    Ok(n.try_into().unwrap())
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "bzip2 compression level must be between 1 and 9",
                    ))
                }
            }
        }
    }
}
