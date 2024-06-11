//! Functions that enable extracting or streaming a Conda package for objects that implement the

use std::path::{Path, PathBuf};

use rattler_conda_types::package::ArchiveType;

use crate::read::{folder_from_conda, folder_from_tar_bz2};
/// Test
pub struct Archive {
    /// Test
    pub archive_type: ArchiveType,
    /// Test
    pub location: PathBuf,
}

impl Archive {
    /// Test
    pub fn new(archive_type: ArchiveType, location: PathBuf) -> Self {
        Archive {
            archive_type,
            location,
        }
    }

    /// Test
    pub fn extract_a_folder(
        &self,
        folder_to_extract: &Path,
        destination: &Path,
    ) -> Result<(), std::io::Error> {
        match self.archive_type {
            ArchiveType::TarBz2 => {
                folder_from_tar_bz2(&self.location, folder_to_extract, destination)
            }
            ArchiveType::Conda => folder_from_conda(&self.location, folder_to_extract, destination),
        }
    }
}

impl TryFrom<PathBuf> for Archive {
    type Error = std::io::Error;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        let archive_type = ArchiveType::try_from(path.as_path()).ok_or(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "package does not point to valid archive",
        ))?;
        Ok(Archive {
            archive_type,
            location: path,
        })
    }
}
