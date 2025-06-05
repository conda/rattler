use std::path::Path;

use base64::{Engine, engine::general_purpose};
use miette::IntoDiagnostic;
use rattler_conda_types::{
    PackageName, VersionWithSource as PackageVersion,
    package::{AboutJson, IndexJson, PackageFile},
};
use rattler_digest::{Md5, compute_file_digest};
use sha2::Sha256;

pub fn sha256_sum(package_file: &Path) -> Result<String, std::io::Error> {
    Ok(format!(
        "{:x}",
        compute_file_digest::<Sha256>(&package_file)?
    ))
}

pub struct ExtractedPackage<'a> {
    file: &'a Path,
    about_json: AboutJson,
    index_json: IndexJson,
    extraction_dir: tempfile::TempDir,
}

impl<'a> ExtractedPackage<'a> {
    pub fn from_package_file(file: &'a Path) -> miette::Result<Self> {
        let extraction_dir = tempfile::tempdir().into_diagnostic()?;

        rattler_package_streaming::fs::extract(file, extraction_dir.path()).into_diagnostic()?;

        let index_json =
            IndexJson::from_package_directory(extraction_dir.path()).into_diagnostic()?;

        let about_json =
            AboutJson::from_package_directory(extraction_dir.path()).into_diagnostic()?;

        Ok(Self {
            file,
            about_json,
            index_json,
            extraction_dir,
        })
    }

    pub fn path(&self) -> &Path {
        self.file
    }

    pub fn package_name(&self) -> &PackageName {
        &self.index_json.name
    }

    pub fn package_version(&self) -> &PackageVersion {
        &self.index_json.version
    }

    pub fn subdir(&self) -> Option<&String> {
        self.index_json.subdir.as_ref()
    }

    pub fn sha256(&self) -> Result<String, std::io::Error> {
        sha256_sum(self.file)
    }

    pub fn base64_md5(&self) -> Result<String, std::io::Error> {
        compute_file_digest::<Md5>(&self.file)
            .map(|digest| general_purpose::STANDARD.encode(digest))
    }

    pub fn filename(&self) -> Option<&str> {
        self.file.file_name().and_then(|s| s.to_str())
    }

    pub fn file_size(&self) -> Result<u64, std::io::Error> {
        self.file.metadata().map(|metadata| metadata.len())
    }

    pub fn about_json(&self) -> &AboutJson {
        &self.about_json
    }

    pub fn index_json(&self) -> &IndexJson {
        &self.index_json
    }

    pub fn extraction_dir(&self) -> &Path {
        self.extraction_dir.path()
    }
}
