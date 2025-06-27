//! This module contains all the types for working with `PyPA` packaging repositories.
//! We have tried to follow the PEP's and `PyPA` packaging guide as closely as possible.
// mod artifact;

// mod artifact_name;

mod package_name;

mod core_metadata;

mod record;

mod extra;

// mod project_info;

mod pypi_version_types;

mod direct_url_json;

mod rfc822ish;

// mod install_paths;

// pub use artifact::{ArtifactFromBytes, ArtifactFromSource, HasArtifactName, ReadPyProjectError};

// pub use artifact_name::{
//     ArtifactName, ArtifactType, BuildTag, InnerAsArtifactName, ParseArtifactNameError,
//     SDistFilename, SDistFormat, STreeFilename, SourceArtifactName, WheelFilename,
// };

pub use direct_url_json::{DirectUrlHashes, DirectUrlJson, DirectUrlSource, DirectUrlVcs};

pub use core_metadata::{MetadataVersion, PackageInfo, WheelCoreMetaDataError, WheelCoreMetadata};

pub use record::{Record, RecordEntry};

pub use package_name::{NormalizedPackageName, PackageName, ParsePackageNameError};

pub use extra::Extra;

// pub use project_info::{ArtifactHashes, ArtifactInfo, DistInfoMetadata, Meta, ProjectInfo, Yanked};

pub use pypi_version_types::{PypiPackageName, PypiVersion};

// pub use install_paths::InstallPaths;

pub(crate) use rfc822ish::RFC822ish;

pub use pep440_rs::*;
pub use pep508_rs::*;
