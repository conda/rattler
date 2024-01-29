//! Functionality for writing conda packages
use std::fs::{self, File};
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};

use itertools::sorted;

use rattler_conda_types::package::PackageMetadata;

/// a function that sorts paths into two iterators, one that starts with `info/` and one that does not
/// both iterators are sorted alphabetically for reproducibility
fn sort_paths<'a>(
    paths: &'a [PathBuf],
    base_path: &'a Path,
) -> (
    impl Iterator<Item = PathBuf> + 'a,
    impl Iterator<Item = PathBuf> + 'a,
) {
    let info = Path::new("info/");
    let (info_paths, other_paths): (Vec<_>, Vec<_>) = paths
        .iter()
        .map(|p| p.strip_prefix(base_path).unwrap())
        .partition(|&path| path.starts_with(info));

    let info_paths = sorted(info_paths.into_iter().map(std::path::Path::to_path_buf));
    let other_paths = sorted(other_paths.into_iter().map(std::path::Path::to_path_buf));

    (info_paths, other_paths)
}

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
    pub fn to_bzip2_level(self) -> Result<bzip2::Compression, std::io::Error> {
        match self {
            CompressionLevel::Lowest => Ok(bzip2::Compression::new(1)),
            CompressionLevel::Default | CompressionLevel::Highest => Ok(bzip2::Compression::new(9)),
            CompressionLevel::Numeric(n) => {
                if (1..=9).contains(&n) {
                    // this conversion from i32 to u32 cannot panic because of the check above
                    Ok(bzip2::Compression::new(n.try_into().unwrap()))
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

/// Write the contents of a list of paths to a tar.bz2 package
/// The paths are sorted alphabetically, and paths beginning with `info/` come first.
///
/// # Arguments
///
/// * `writer` - the writer to write the package to
/// * `base_path` - the base path of the package. All paths in `paths` are relative to this path
/// * `paths` - a list of paths to include in the package
/// * `compression_level` - the compression level to use for the inner bzip2 encoded files
/// * `timestamp` - optional a timestamp to use for all archive files (useful for reproducible builds)
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
/// write_tar_bz2_package(&mut file, &PathBuf::from("test"), &paths, CompressionLevel::Default, None).unwrap();
/// ```
///
/// # See also
///
/// * [`write_conda_package`]
pub fn write_tar_bz2_package<'a, W: Write, F1: Fn(usize), F2: Fn(usize)>(
    writer: W,
    base_path: &Path,
    paths: &[PathBuf],
    compression_level: CompressionLevel,
    timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    progress_bar_func: Option<(F1, F2)>,
) -> Result<(), std::io::Error> {
    let mut archive = tar::Builder::new(bzip2::write::BzEncoder::new(
        writer,
        compression_level.to_bzip2_level()?,
    ));
    archive.follow_symlinks(false);

    // sort paths alphabetically, and sort paths beginning with `info/` first
    let (info_paths, other_paths) = sort_paths(paths, base_path);
    let paths = info_paths.chain(other_paths).collect::<Vec<_>>();

    let size = paths
        .iter()
        .try_fold(0usize, |acc, p| -> std::io::Result<usize> {
            // make sure to not follow symlinks
            let meta = p.symlink_metadata()?;
            Ok(if meta.is_file() {
                acc + meta.len() as usize
            } else {
                // 0 for all other files would mean a huge number of symblinks
                // or folders will still start with 100% filled progress bar
                1
            })
        })?;

    let mut progress_bar_driver = None;
    if let Some((progress_bar_start, progress_bar_update)) = progress_bar_func {
        progress_bar_start(size);
        _ = progress_bar_driver.insert(progress_bar_update);
    }

    for path in paths {
        append_path_to_archive(
            &mut archive,
            base_path,
            &path,
            timestamp,
            progress_bar_driver.as_ref(),
        )?;
    }

    archive.into_inner()?.finish()?;

    Ok(())
}

/// Write the contents of a list of paths to a tar zst archive
fn write_zst_archive<W: Write>(
    writer: W,
    base_path: &Path,
    paths: impl Iterator<Item = PathBuf>,
    compression_level: CompressionLevel,
    num_threads: Option<u32>,
    timestamp: Option<&chrono::DateTime<chrono::Utc>>,
) -> Result<(), std::io::Error> {
    // Create a temporary tar file
    let tar_path = tempfile::Builder::new().tempfile_in(base_path)?;
    let mut archive = tar::Builder::new(&tar_path);
    archive.follow_symlinks(false);
    for path in paths {
        append_path_to_archive::<fn(usize)>(&mut archive, base_path, &path, timestamp, None)?;
    }
    archive.finish()?;

    // Compress it as tar.zst
    let mut tar_file = File::open(&tar_path)?;
    let compression_level = compression_level.to_zstd_level()?;
    let mut zst_encoder = zstd::Encoder::new(writer, compression_level)?;
    zst_encoder.multithread(num_threads.unwrap_or_else(|| num_cpus::get() as u32))?;
    zst_encoder.set_pledged_src_size(tar_file.metadata().map(|v| v.len()).ok())?;
    zst_encoder.include_contentsize(true)?;

    // Append tar.zst to the archive
    io::copy(&mut tar_file, &mut zst_encoder)?;
    zst_encoder.finish()?;

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
/// * `compression_num_threads` - the number of threads to use for zstd compression (defaults to
/// the number of CPU cores if `None`)
/// * `timestamp` - optional a timestamp to use for all archive files (useful for reproducible builds)
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
    compression_num_threads: Option<u32>,
    out_name: &str,
    timestamp: Option<&chrono::DateTime<chrono::Utc>>,
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

    let archive_path = format!("pkg-{out_name}.tar.zst");
    outer_archive.start_file(archive_path, options)?;
    write_zst_archive(
        &mut outer_archive,
        base_path,
        other_paths,
        compression_level,
        compression_num_threads,
        timestamp,
    )?;

    // info paths come last
    let archive_path = format!("info-{out_name}.tar.zst");
    outer_archive.start_file(archive_path, options)?;
    write_zst_archive(
        &mut outer_archive,
        base_path,
        info_paths,
        compression_level,
        compression_num_threads,
        timestamp,
    )?;

    outer_archive.finish()?;

    Ok(())
}

fn prepare_header(
    path: &Path,
    timestamp: Option<&chrono::DateTime<chrono::Utc>>,
) -> Result<tar::Header, std::io::Error> {
    let mut header = tar::Header::new_gnu();
    let name = b"././@LongLink";
    header.as_gnu_mut().unwrap().name[..name.len()].clone_from_slice(&name[..]);

    let stat = fs::symlink_metadata(path)?;
    header.set_metadata_in_mode(&stat, tar::HeaderMode::Deterministic);

    if let Some(timestamp) = timestamp {
        header.set_mtime(timestamp.timestamp().unsigned_abs());
    } else {
        // 1-1-2023 00:00:00 (Fixed date in the past for reproducible builds)
        header.set_mtime(1672531200);
    }

    Ok(header)
}

fn trace_file_error(path: &Path, err: std::io::Error) -> std::io::Error {
    println!("{}: {}", path.display(), err);
    std::io::Error::new(err.kind(), format!("{}: {}", path.display(), err))
}

fn append_path_to_archive<F: Fn(usize)>(
    archive: &mut tar::Builder<impl Write>,
    base_path: &Path,
    path: &Path,
    timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    progress_bar_driver: Option<F>,
) -> Result<(), std::io::Error> {
    // create a tar header
    let mut header = prepare_header(&base_path.join(path), timestamp)
        .map_err(|err| trace_file_error(&base_path.join(path), err))?;

    if header.entry_type().is_file() {
        let file = fs::File::open(base_path.join(path))
            .map_err(|err| trace_file_error(&base_path.join(path), err))?;
        archive.append_data(&mut header, path, &file)?;

        // increment the progress bar by the file size
        let file_metadata = base_path.join(path).symlink_metadata()?;
        progress_bar_driver.map(|f| f(file_metadata.len() as usize));
    } else if header.entry_type().is_symlink() || header.entry_type().is_hard_link() {
        let target = fs::read_link(base_path.join(path))
            .map_err(|err| trace_file_error(&base_path.join(path), err))?;

        archive.append_link(&mut header, path, target)?;
        progress_bar_driver.map(|f| f(1));
    } else if header.entry_type().is_dir() {
        archive.append_data(&mut header, path, std::io::empty())?;
        progress_bar_driver.map(|f| f(1));
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unsupported file type",
        ));
    }

    Ok(())
}
