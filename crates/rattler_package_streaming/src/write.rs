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
/// use rattler_package_streaming::write::write_tar_bz2_package;
///
/// let paths = vec![PathBuf::from("info/recipe/meta.yaml"), PathBuf::from("info/recipe/conda_build_config.yaml")];
/// let mut file = File::create("test.tar.bz2").unwrap();
/// write_tar_bz2_package(&mut file, &PathBuf::from("test"), &paths, 9).unwrap();
/// ```
///
/// # See also
///
/// * [write_conda_package]
pub fn write_tar_bz2_package<W: Write>(
    writer: W,
    base_path: &Path,
    paths: &[PathBuf],
    compression_level: i32,
) -> Result<(), std::io::Error> {
    let mut archive = tar::Builder::new(bzip2::write::BzEncoder::new(
        writer,
        bzip2::Compression::new(compression_level.try_into().map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid compression level: {}", e),
            )
        })?),
    ));

    // sort paths alphabetically, and sort paths beginning with `info/` first
    let (info_paths, other_paths) = sort_paths(paths, base_path);
    for path in info_paths.iter().chain(other_paths.iter()) {
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
    compression_level: i32,
) -> Result<(), std::io::Error> {
    // TODO figure out multi-threading for zstd
    let mut archive = tar::Builder::new(zstd::Encoder::new(writer, compression_level)?);

    for path in paths {
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
    compression_level: i32,
) -> Result<(), std::io::Error> {
    // first create the outer zip archive that uses no compression
    let mut outer_archive = zip::ZipWriter::new(writer);
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // write the metadata as first file in the zip archive
    let package_metadata = PackageMetadata {
        conda_pkg_format_version: 2,
    };
    let package_metadata = serde_json::to_string(&package_metadata).unwrap();
    outer_archive.start_file("metadata.json", options)?;
    outer_archive.write_all(package_metadata.as_bytes())?;

    let (info_paths, other_paths) = sort_paths(paths, base_path);

    outer_archive.start_file("pkg-archive.tar.zst", options)?;
    write_zst_archive(
        &mut outer_archive,
        base_path,
        &other_paths,
        compression_level,
    )?;

    // info paths come last
    outer_archive.start_file("info-archive.tar.zst", options)?;
    write_zst_archive(
        &mut outer_archive,
        base_path,
        &info_paths,
        compression_level,
    )?;

    outer_archive.finish()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, path::PathBuf};

    use walkdir::WalkDir;

    #[test]
    fn test_write_tar_bz2() {
        // repackage a package found at `path` and compare the contents of the tar.bz2
        // with the contents of the original package
        // `path` should be a path to a directory containing a `info/` directory
        let path = PathBuf::from("/Users/wolfv/micromamba/pkgs/zstandard-0.19.0-py311hdcbfb07_1");

        // recursively find all files under the path directory
        let files = WalkDir::new(&path)
            .into_iter()
            .filter_map(|e| e.ok())
            .map(|e| e.into_path())
            .filter(|p| p.is_file())
            .collect::<Vec<_>>();

        let outfile = File::create("test.tar.bz2").unwrap();
        write_tar_bz2_package(outfile, &path, &files, 9).unwrap();
    }

    #[test]
    fn test_write_conda() {
        // repackage a package found at `path` and compare the contents of the tar.bz2
        // with the contents of the original package
        // `path` should be a path to a directory containing a `info/` directory

        let path = PathBuf::from("/Users/wolfv/micromamba/pkgs/zstandard-0.19.0-py311hdcbfb07_1");
        println!("Making new conda package! {:?}", path);

        // recursively find all files under the path directory
        let files = WalkDir::new(&path)
            .into_iter()
            .filter_map(|e| e.ok())
            .map(|e| e.into_path())
            .filter(|p| p.is_file())
            .collect::<Vec<_>>();

        let outfile = File::create("test.conda").unwrap();
        write_conda_package(outfile, &path, &files, 15).unwrap();
    }
}
