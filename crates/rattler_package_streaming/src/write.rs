//! Functionality for writing conda packages

use itertools::Itertools;

use std::io::Seek;
use std::io::Write;

use std::path::Path;
use std::path::PathBuf;

use bzip2;
use rattler_conda_types::package::PackageMetadata;
use tar;
use zip;

/// a function that sorts paths into two vectors, one that starts with `info/` and one that does not
/// both vectors are sorted alphabetically for reproducibility
fn sort_paths(paths: &[PathBuf], base_path: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let info = Path::new("info/");
    let (info_paths, other_paths): (Vec<_>, Vec<_>) = paths
        .iter()
        .map(|p| p.strip_prefix(base_path).unwrap())
        .partition(|path| path.starts_with(info));

    let info_paths = info_paths.into_iter();
    let other_paths = other_paths.into_iter();

    let info_paths = info_paths
        .sorted_by(|a, b| a.cmp(b))
        .map(|p| p.to_path_buf())
        .collect::<Vec<PathBuf>>();
    let other_paths = other_paths
        .sorted_by(|a, b| a.cmp(b))
        .map(|p| p.to_path_buf())
        .collect::<Vec<PathBuf>>();

    (info_paths, other_paths)
}

/// Select the compression level to use for the package
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionLevel {
    /// Use the lowest compression level (zstd: 1, bzip2: 1)
    Lowest,
    /// Use the highest compression level (zstd: 22, bzip2: 9)
    Highest,
    /// Use the default compression level (zstd: 15, bzip2: 9)
    Default,
    /// Use a numeric compression level (zstd: 1-22, bzip2: 1-9)
    Numeric(u32),
}

impl Default for CompressionLevel {
    fn default() -> Self {
        CompressionLevel::Default
    }
}

impl CompressionLevel {
    fn to_zstd_level(self) -> Result<i32, std::io::Error> {
        match self {
            CompressionLevel::Lowest => Ok(1),
            CompressionLevel::Highest => Ok(22),
            CompressionLevel::Default => Ok(15),
            CompressionLevel::Numeric(n) => {
                if !(1..=22).contains(&n) {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "zstd compression level must be between 1 and 22",
                    ))
                } else {
                    Ok(n as i32)
                }
            }
        }
    }

    fn to_bzip2_level(self) -> Result<bzip2::Compression, std::io::Error> {
        match self {
            CompressionLevel::Lowest => Ok(bzip2::Compression::new(1)),
            CompressionLevel::Highest => Ok(bzip2::Compression::new(9)),
            CompressionLevel::Default => Ok(bzip2::Compression::new(9)),
            CompressionLevel::Numeric(n) => {
                if !(1..=9).contains(&n) {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "bzip2 compression level must be between 1 and 9",
                    ))
                } else {
                    Ok(bzip2::Compression::new(n))
                }
            }
        }
    }
}

/// Write the contents of a list of paths to a tar.bz2 package
/// The paths are sorted alphabetically, and paths beginning with `info/` come first.
///
/// # Arguments
///
/// * `writer` - the writer to write the package to
/// * `base_path` - the base path of the package. All paths in `paths` are relative to this path
/// * `paths` - a list of paths to include in the package
/// * `compression_level` - the compression level to use for the inner bzip2 encoded files
///
/// # Errors
///
/// This function will return an error if the writer returns an error, or if the paths are not
/// relative to the base path.
///
/// # Examples
///
/// ```no_run
/// use std::path::PathBuf;
/// use std::fs::File;
/// use rattler_package_streaming::write::{write_tar_bz2_package, CompressionLevel};
///
/// let paths = vec![PathBuf::from("info/recipe/meta.yaml"), PathBuf::from("info/recipe/conda_build_config.yaml")];
/// let mut file = File::create("test.tar.bz2").unwrap();
/// write_tar_bz2_package(&mut file, &PathBuf::from("test"), &paths, CompressionLevel::Default).unwrap();
/// ```
///
/// # See also
///
/// * [write_conda_package]
pub fn write_tar_bz2_package<W: Write>(
    writer: W,
    base_path: &Path,
    paths: &[PathBuf],
    compression_level: CompressionLevel,
) -> Result<(), std::io::Error> {
    let mut archive = tar::Builder::new(bzip2::write::BzEncoder::new(
        writer,
        compression_level.to_bzip2_level()?,
    ));

    // sort paths alphabetically, and sort paths beginning with `info/` first
    let (info_paths, other_paths) = sort_paths(paths, base_path);
    for path in info_paths.iter().chain(other_paths.iter()) {
        // TODO we need more control over the archive headers here to
        // set uid, gid to 0.
        archive.append_path_with_name(base_path.join(path), path)?;
    }

    archive.into_inner()?.finish()?;

    Ok(())
}

/// Write the contents of a list of paths to a tar zst archive
fn write_zst_archive<W: Write>(
    writer: W,
    base_path: &Path,
    paths: &[PathBuf],
    compression_level: CompressionLevel,
) -> Result<(), std::io::Error> {
    // TODO figure out multi-threading for zstd
    let compression_level = compression_level.to_zstd_level()?;
    let mut archive = tar::Builder::new(zstd::Encoder::new(writer, compression_level)?);

    for path in paths {
        // TODO we need more control over the archive headers here to
        // set uid, gid to 0.
        archive.append_path_with_name(base_path.join(path), path)?;
    }

    archive.into_inner()?.finish()?;

    Ok(())
}

/// Write a `.conda` package to a writer
/// A `.conda` package is an outer uncompressed zip archive that contains a `metadata.json` file, a
/// `pkg-archive.tar.zst` file, and a `info-archive.tar.zst` file.
/// The inner zstd encoded files are sorted alphabetically. The `info-archive.tar.zst` file comes last in
/// the outer zip archive.
///
/// # Arguments
///
/// * `writer` - the writer to write the package to
/// * `base_path` - the base path of the package. All paths in `paths` are relative to this path
/// * `paths` - a list of paths to include in the package
/// * `compression_level` - the compression level to use for the inner zstd encoded files
///
/// # Errors
///
/// This function will return an error if the writer returns an error, or if the paths are not
/// relative to the base path.
pub fn write_conda_package<W: Write + Seek>(
    writer: W,
    base_path: &Path,
    paths: &[PathBuf],
    compression_level: CompressionLevel,
    out_name: &str,
) -> Result<(), std::io::Error> {
    // first create the outer zip archive that uses no compression
    let mut outer_archive = zip::ZipWriter::new(writer);
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // write the metadata as first file in the zip archive
    let package_metadata = PackageMetadata::default();
    let package_metadata = serde_json::to_string(&package_metadata).unwrap();
    outer_archive.start_file("metadata.json", options)?;
    outer_archive.write_all(package_metadata.as_bytes())?;

    let (info_paths, other_paths) = sort_paths(paths, base_path);

    outer_archive.start_file(format!("pkg-{out_name}.tar.zst"), options)?;
    write_zst_archive(
        &mut outer_archive,
        base_path,
        &other_paths,
        compression_level,
    )?;

    // info paths come last
    outer_archive.start_file(format!("info-{out_name}.tar.zst"), options)?;
    write_zst_archive(
        &mut outer_archive,
        base_path,
        &info_paths,
        compression_level,
    )?;

    outer_archive.finish()?;

    Ok(())
}
