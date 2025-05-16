use crate::types::{
    ArtifactFromBytes, ArtifactFromSource, HasArtifactName, NormalizedPackageName, PackageInfo,
    PypiVersion, ReadPyProjectError, SDistFilename, SDistFormat, SourceArtifactName,
};
use crate::types::{WheelCoreMetaDataError, WheelCoreMetadata};
use crate::utils::ReadAndSeek;
use flate2::read::GzDecoder;

use fs_err as fs;
use miette::IntoDiagnostic;

use std::ffi::OsStr;
use std::io::{ErrorKind, Read, Seek};
use std::path::{Path, PathBuf};
use tar::Archive;
use zip::ZipArchive;

/// Represents a source distribution artifact.
pub struct SDist {
    /// Name of the source distribution
    pub name: SDistFilename,

    /// Source dist archive
    file: parking_lot::Mutex<Box<dyn ReadAndSeek + Send>>,
}

#[derive(thiserror::Error, Debug)]
pub enum SDistError {
    #[error("IO error while reading PKG-INFO: {0}")]
    PkgInfoIOError(#[source] std::io::Error),

    #[error("No PKG-INFO found in archive")]
    NoPkgInfoFound,

    #[error(transparent)]
    PyProjectTomlError(#[from] ReadPyProjectError),

    #[error("Could not parse metadata")]
    WheelCoreMetaDataError(#[from] WheelCoreMetaDataError),
}

/// Utility function to skip the first component of a path
fn skip_first_component(path: &Path) -> PathBuf {
    path.components().skip(1).collect()
}

impl SDist {
    /// Create this struct from a path
    #[allow(dead_code)]
    pub fn from_path(
        path: &Path,
        normalized_package_name: &NormalizedPackageName,
    ) -> miette::Result<Self> {
        let file_name = path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| miette::miette!("path does not contain a filename"))?;
        let name =
            SDistFilename::from_filename(file_name, normalized_package_name).into_diagnostic()?;
        let bytes = fs::File::open(path).into_diagnostic()?;
        Self::from_bytes(name, Box::new(bytes))
    }

    /// Find entry in tar archive
    fn find_entry(&self, name: impl AsRef<Path>) -> std::io::Result<Option<Vec<u8>>> {
        let mut lock = self.file.lock();
        let archives = generic_archive_reader(&mut lock, self.name.format)?;

        match archives {
            Archives::TarArchive(mut archive) => {
                // Loop over entries
                for entry in archive.entries()? {
                    let mut entry = entry?;

                    // Find name in archive and return this
                    if skip_first_component(entry.path()?.as_ref()) == name.as_ref() {
                        let mut bytes = Vec::new();
                        entry.read_to_end(&mut bytes)?;
                        return Ok(Some(bytes));
                    }
                }
                Ok(None)
            }
            Archives::Zip(mut archive) => {
                // Loop over zip entries and extract zip file by index
                // If file's path is not safe, ignore it and record a warning message
                for i in 0..archive.len() {
                    let mut file = archive.by_index(i)?;
                    if let Some(file_path) = file.enclosed_name() {
                        if skip_first_component(&file_path) == name.as_ref() {
                            let mut bytes = Vec::new();
                            file.read_to_end(&mut bytes)?;
                            return Ok(Some(bytes));
                        }
                    } else {
                        tracing::warn!(
                            "Ignoring {0} as it cannot be converted to a valid path",
                            file.name()
                        );
                    }
                }
                Ok(None)
            }
        }
    }

    /// Read .PKG-INFO from the archive
    pub fn read_package_info(&self) -> Result<(Vec<u8>, PackageInfo), SDistError> {
        if let Some(bytes) = self
            .find_entry("PKG-INFO")
            .map_err(SDistError::PkgInfoIOError)?
        {
            let metadata = PackageInfo::from_bytes(bytes.as_slice())?;

            Ok((bytes, metadata))
        } else {
            Err(SDistError::NoPkgInfoFound)
        }
    }

    /// Checks if this artifact implements PEP 643
    /// and returns the metadata if it does
    pub fn pep643_metadata(&self) -> Result<Option<(Vec<u8>, WheelCoreMetadata)>, SDistError> {
        // Assume we have a PKG-INFO
        let (bytes, metadata) = self.read_package_info()?;
        let metadata =
            WheelCoreMetadata::try_from(metadata).map_err(SDistError::WheelCoreMetaDataError)?;
        if metadata.metadata_version.implements_pep643() {
            Ok(Some((bytes, metadata)))
        } else {
            Ok(None)
        }
    }

    /// Get a lock on the inner data
    pub fn lock_data(&self) -> parking_lot::MutexGuard<'_, Box<dyn ReadAndSeek + Send>> {
        self.file.lock()
    }
}

impl HasArtifactName for SDist {
    type Name = SDistFilename;

    fn name(&self) -> &Self::Name {
        &self.name
    }
}

impl ArtifactFromBytes for SDist {
    fn from_bytes(name: Self::Name, bytes: Box<dyn ReadAndSeek + Send>) -> miette::Result<Self> {
        Ok(Self {
            name,
            file: parking_lot::Mutex::new(bytes),
        })
    }
}

impl ArtifactFromSource for SDist {
    fn try_get_bytes(&self) -> Result<Vec<u8>, std::io::Error> {
        let mut vec = vec![];
        let mut inner = self.lock_data();
        inner.rewind()?;
        inner.read_to_end(&mut vec)?;
        Ok(vec)
    }

    fn distribution_name(&self) -> String {
        self.name().distribution.as_source_str().to_owned()
    }

    fn version(&self) -> PypiVersion {
        PypiVersion::Version {
            version: self.name().version.clone(),
            package_allows_prerelease: false,
        }
    }

    fn artifact_name(&self) -> SourceArtifactName {
        SourceArtifactName::SDist(self.name().to_owned())
    }

    fn read_pyproject_toml(&self) -> Result<pyproject_toml::PyProjectToml, ReadPyProjectError> {
        if let Some(bytes) = self.find_entry("pyproject.toml")? {
            let source = String::from_utf8(bytes).map_err(|e| {
                ReadPyProjectError::PyProjectTomlParseError(format!(
                    "could not parse pyproject.toml (bad encoding): {e}"
                ))
            })?;
            let project = pyproject_toml::PyProjectToml::new(&source).map_err(|e| {
                ReadPyProjectError::PyProjectTomlParseError(format!(
                    "could not parse pyproject.toml (bad toml): {e}"
                ))
            })?;
            Ok(project)
        } else {
            Err(ReadPyProjectError::NoPyProjectTomlFound)
        }
    }

    /// Extract the contents of the sdist archive to the given directory
    fn extract_to(&self, work_dir: &Path) -> std::io::Result<()> {
        let mut lock = self.file.lock();
        let archives = generic_archive_reader(&mut lock, self.name.format)?;
        match archives {
            Archives::TarArchive(mut archive) => {
                // when unpacking tomli-2.0.1.tar.gz we face the issue that
                // python std zipfile library does not support timestamps before 1980
                // happens when unpacking the `tomli-2.0.1` source distribution
                // https://github.com/alexcrichton/tar-rs/issues/349
                archive.set_preserve_mtime(false);
                archive.unpack(work_dir)?;
                Ok(())
            }
            Archives::Zip(mut archive) => {
                archive.extract(work_dir)?;
                Ok(())
            }
        }
    }
}

enum RawAndGzReader<'a> {
    Raw(&'a mut Box<dyn ReadAndSeek + Send>),
    Gz(GzDecoder<&'a mut Box<dyn ReadAndSeek + Send>>),
}

impl Read for RawAndGzReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Raw(r) => r.read(buf),
            Self::Gz(r) => r.read(buf),
        }
    }
}

enum Archives<'a> {
    TarArchive(Box<Archive<RawAndGzReader<'a>>>),
    Zip(Box<ZipArchive<&'a mut Box<dyn ReadAndSeek + Send>>>),
}

fn generic_archive_reader(
    file: &mut Box<dyn ReadAndSeek + Send>,
    format: SDistFormat,
) -> std::io::Result<Archives<'_>> {
    file.rewind()?;

    match format {
        SDistFormat::TarGz => {
            let bytes = GzDecoder::new(file);
            Ok(Archives::TarArchive(Box::new(Archive::new(RawAndGzReader::Gz(bytes)))))
        }
        SDistFormat::Tar => Ok(Archives::TarArchive(Box::new(Archive::new(RawAndGzReader::Raw(file))))),
        SDistFormat::Zip => {
            let zip = ZipArchive::new(file)?;
            Ok(Archives::Zip(Box::new(zip)))
        },
        unsupported_format => Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("sdist archive format currently {unsupported_format} unsupported (only tar | tar.gz | zip are supported)"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use crate::artifacts::SDist;
    use crate::types::ArtifactFromSource;
    use insta::{assert_debug_snapshot, assert_ron_snapshot};
    use std::path::Path;

    #[test]
    pub fn read_rich_build_info() {
        // Read path
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/sdists/rich-13.6.0.tar.gz");

        // Load sdist
        let sdist = super::SDist::from_path(&path, &"rich".parse().unwrap()).unwrap();

        let build_system = sdist.read_pyproject_toml().unwrap().build_system.unwrap();

        assert_ron_snapshot!(build_system, @r#"
        BuildSystem(
          requires: [
            "poetry-core>=1.0.0",
          ],
          r#build-backend: Some("poetry.core.masonry.api"),
          r#backend-path: None,
        )
        "#);
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn read_zip_archive_for_a_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test-data/sdists/zip_read_package-1.0.0.zip");

        let sdist = SDist::from_path(&path, &"zip_read_package".parse().unwrap()).unwrap();

        let content = sdist.find_entry("test_file.txt").unwrap().unwrap();
        let content_text = String::from_utf8(content).unwrap();

        assert!(content_text.contains("hello world"));

        let content = sdist
            .find_entry("inner_folder/inner_file.txt")
            .unwrap()
            .unwrap();
        let content_text = String::from_utf8(content).unwrap();

        assert!(content_text.contains("hello inner world"));
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn read_tar_gz_archive_for_a_file() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/sdists/rich-13.6.0.tar.gz");

        let sdist = SDist::from_path(&path, &"rich".parse().unwrap()).unwrap();

        let pkg_info = sdist.find_entry("PKG-INFO").unwrap().unwrap();
        let pkg_info_text = String::from_utf8(pkg_info).unwrap();
        assert_debug_snapshot!(pkg_info_text);

        let init_file = sdist.find_entry("rich/__init__.py").unwrap().unwrap();
        let init_file_text = String::from_utf8(init_file).unwrap();
        assert_debug_snapshot!(init_file_text);
    }
}
