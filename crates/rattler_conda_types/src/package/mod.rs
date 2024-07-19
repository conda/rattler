//! Contains models of files that are found in the `info/` directory of a package.

mod about;
mod archive_identifier;
mod archive_type;
mod entry_point;
mod files;
mod has_prefix;
mod index;
mod link;
mod no_link;
mod no_softlink;
mod package_metadata;
mod paths;
mod run_exports;

use std::fs::File;
use std::io::Read;
use std::path::Path;
pub use {
    about::AboutJson,
    archive_identifier::ArchiveIdentifier,
    archive_type::ArchiveType,
    entry_point::EntryPoint,
    files::Files,
    has_prefix::HasPrefix,
    has_prefix::HasPrefixEntry,
    index::IndexJson,
    link::{LinkJson, NoArchLinks, PythonEntryPoints},
    no_link::NoLink,
    no_softlink::NoSoftlink,
    package_metadata::PackageMetadata,
    paths::{FileMode, PathType, PathsEntry, PathsJson, PrefixPlaceholder},
    run_exports::RunExportsJson,
};

/// A trait implemented for structs that represent specific files in a Conda archive.
///
/// This trait provides a standardised interface for accessing the contents of known files in a
/// Conda package, such as the `index.json` (see [`IndexJson`]) or `about.json` (see [`AboutJson`])
/// files. Structs that represent these files should implement this trait in order to ensure that
/// they can be easily accessed and manipulated by other code that expects a consistent interface.
pub trait PackageFile: Sized {
    /// Returns the path to the file within the Conda archive.
    ///
    /// The path is relative to the root of the archive and include any necessary directories.
    fn package_path() -> &'static Path;

    /// Parses the object from a string, using a format appropriate for the file type.
    ///
    /// For example, if the file is in JSON format, this function parses the JSON string and returns
    /// the resulting object. If the file is not in a parsable format, this function returns an
    /// error.
    fn from_str(str: &str) -> Result<Self, std::io::Error>;

    /// Parses the object from a `Read` trait object, using a format appropriate for the file type.
    ///
    /// For example, if the file is in JSON format, this function reads the data from the `Read`
    /// object, parse the JSON string and return the resulting object. If the file is not in a
    /// parsable format, this function returns an error.
    fn from_reader(mut reader: impl Read) -> Result<Self, std::io::Error> {
        let mut str = String::new();
        reader.read_to_string(&mut str)?;
        Self::from_str(&str)
    }

    /// Parses the object from a file specified by a `path`, using a format appropriate for the file
    /// type.
    ///
    /// For example, if the file is in JSON format, this function reads the data from the file at
    /// the specified path, parse the JSON string and return the resulting object. If the file is
    /// not in a parsable format or if the file could not read, this function returns an error.
    fn from_path(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        Self::from_reader(File::open(path)?)
    }

    /// Parses the object by looking up the appropriate file from the root of the specified Conda
    /// archive directory, using a format appropriate for the file type.
    ///
    /// For example, if the file is in JSON format, this function reads the appropriate file from
    /// the archive, parse the JSON string and return the resulting object. If the file is not in a
    /// parsable format or if the file could not be read, this function returns an error.
    fn from_package_directory(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        Self::from_path(path.as_ref().join(Self::package_path()))
    }
}
