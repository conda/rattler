//! This crate provides the ability to extract a specified directory from tar.bz2 or conda archive.

use std::path::{Path, PathBuf};

use rattler_conda_types::package::ArchiveType;

use crate::fs::{extract_directory_from_conda, extract_directory_from_tar_bz2};

/// A struct representing an archive file located on disk.
pub struct LocalArchive {
    /// Archive type representing the type of archive.
    pub archive_type: ArchiveType,
    /// Location of the archive file on disk.
    pub location: PathBuf,
}

impl LocalArchive {
    /// Extracts the contents of the archive to the specified destination.
    pub fn extract_a_folder(
        &self,
        folder_to_extract: &Path,
        destination: &Path,
    ) -> Result<(), std::io::Error> {
        match self.archive_type {
            ArchiveType::TarBz2 => {
                extract_directory_from_tar_bz2(&self.location, folder_to_extract, destination)
            }
            ArchiveType::Conda => {
                extract_directory_from_conda(&self.location, folder_to_extract, destination)
            }
        }
    }

    /// Tries to convert the specified path into a `LocalArchive`.
    /// Returns an error if the path does not point to a valid archive ( `.tar.bz2` or `.conda` )
    pub fn try_from_path(path: PathBuf) -> Result<Self, std::io::Error> {
        Self::try_from(path)
    }
}

impl TryFrom<PathBuf> for LocalArchive {
    type Error = std::io::Error;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        let archive_type = ArchiveType::try_from(path.as_path()).ok_or(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "package does not point to valid archive",
        ))?;
        Ok(LocalArchive {
            archive_type,
            location: path,
        })
    }
}

#[cfg(test)]
mod tests {
    
    use tempfile::{tempdir, TempDir};
    
    

    use crate::write::{write_conda_package, write_tar_bz2_package, CompressionLevel};

    use super::*;
    use std::fs::{self, File};
    use std::io::{Read};

    fn create_tar_bz2_archive_with_folder() -> (TempDir, PathBuf) {
        let temp_dir = tempdir().unwrap();
        let archive_path = temp_dir.path().join("archive.tar.bz2");
        let archive = File::create(&archive_path).unwrap();

        // Create info/meta.yaml and info/recipe/recipe.yaml
        let info_meta_path = temp_dir.path().join("info").join("meta.yaml");
        fs::create_dir_all(info_meta_path.parent().unwrap()).unwrap();
        fs::write(&info_meta_path, b"meta: data").unwrap();

        let info_recipe_path = temp_dir
            .path()
            .join("info")
            .join("recipe")
            .join("recipe.yaml");
        fs::create_dir_all(info_recipe_path.parent().unwrap()).unwrap();
        fs::write(&info_recipe_path, b"its_recipe_yaml: yes").unwrap();

        // Create tar.bz2 archive
        write_tar_bz2_package(
            archive,
            temp_dir.path(),
            vec![info_meta_path, info_recipe_path].as_slice(),
            CompressionLevel::default(),
            None,
            None,
        )
        .unwrap();
        (temp_dir, archive_path)
    }

    fn create_conda_archive_with_folder() -> (TempDir, PathBuf) {
        let temp_dir = tempdir().unwrap();
        let archive_path = temp_dir.path().join("archive.conda");
        let archive = File::create(&archive_path).unwrap();

        // Create info/meta.yaml and info/recipe/recipe.yaml
        let info_meta_path = temp_dir.path().join("info").join("meta.yaml");
        fs::create_dir_all(info_meta_path.parent().unwrap()).unwrap();
        fs::write(&info_meta_path, b"meta: data").unwrap();

        let info_recipe_path = temp_dir
            .path()
            .join("info")
            .join("recipe")
            .join("recipe.yaml");
        fs::create_dir_all(info_recipe_path.parent().unwrap()).unwrap();
        fs::write(&info_recipe_path, b"its_recipe_yaml: yes").unwrap();

        let paths = vec![info_meta_path, info_recipe_path];

        write_conda_package(
            archive,
            temp_dir.path(),
            paths.as_slice(),
            CompressionLevel::default(),
            None,
            "test-package",
            None,
            None,
        )
        .unwrap();

        (temp_dir, archive_path)
    }

    #[test]
    fn test_local_archive_from_tar_bz() {
        let location = PathBuf::from("/path/to/archive.tar.bz2");
        LocalArchive::try_from_path(location.clone())
            .expect("Archive should be created of tar bz type");
    }

    #[test]
    fn test_local_archive_from_conda() {
        let location = PathBuf::from("/path/to/conda_archive.conda");
        LocalArchive::try_from_path(location.clone())
            .expect("Archive should be created of conda type");
    }

    #[test]
    fn test_extract_from_tar_bz2() {
        // Create a tar.bz2 archive with a folder containing one file
        let (_tmp, archive_path) = create_tar_bz2_archive_with_folder();

        let archive = LocalArchive::try_from_path(archive_path.clone()).unwrap();
        let folder_to_extract = Path::new("info/recipe");
        let destination = tempdir().unwrap().path().to_path_buf().join("extract_to");

        // Extract the folder
        archive
            .extract_a_folder(folder_to_extract, &destination)
            .unwrap();

        // Verify the extraction
        let extracted_file_path = destination.join("recipe.yaml");
        assert!(extracted_file_path.exists());

        let mut extracted_file = File::open(&extracted_file_path).unwrap();
        let mut content = Vec::default();
        extracted_file.read_to_end(&mut content).unwrap();
        assert_eq!(content, b"its_recipe_yaml: yes");
    }

    #[test]
    fn test_extract_from_conda() {
        // Create a tar.bz2 archive with a folder containing one file
        let (_tmp, archive_path) = create_conda_archive_with_folder();

        let archive = LocalArchive::try_from_path(archive_path.clone()).unwrap();
        let folder_to_extract = Path::new("info/recipe");
        let destination = tempdir().unwrap().path().to_path_buf().join("extract_to");

        // Extract the folder
        archive
            .extract_a_folder(folder_to_extract, &destination)
            .unwrap();

        // Verify the extraction
        let extracted_file_path = destination.join("recipe.yaml");
        assert!(extracted_file_path.exists());

        let mut extracted_file = File::open(&extracted_file_path).unwrap();
        let mut content = Vec::default();
        extracted_file.read_to_end(&mut content).unwrap();
        assert_eq!(content, b"its_recipe_yaml: yes");
    }
}
