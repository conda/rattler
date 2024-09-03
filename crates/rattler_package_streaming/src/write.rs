//! Functionality for writing conda packages
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};

use chrono::{Datelike, Timelike};
use rattler_conda_types::package::PackageMetadata;
use zip::DateTime;

/// Trait for progress bars
pub trait ProgressBar {
    /// Set the current progress and progress message
    fn set_progress(&mut self, progress: u64, message: &str);
    /// Set the total amount of bytes
    fn set_total(&mut self, total: u64);
}

/// A wrapper for a reader that updates a progress bar
struct ProgressBarReader {
    reader: Option<File>,
    progress_bar: Option<Box<dyn ProgressBar>>,
    progress: u64,
    total: u64,
    message: String,
}

impl ProgressBarReader {
    fn new(progress_bar: Option<Box<dyn ProgressBar>>) -> Self {
        Self {
            reader: None,
            progress_bar,
            progress: 0,
            total: 0,
            message: String::new(),
        }
    }

    fn set_file(&mut self, file: File) {
        self.reader = Some(file);
    }

    fn reset_position(&mut self) {
        self.progress = 0;
        if let Some(progress_bar) = &mut self.progress_bar {
            progress_bar.set_progress(0, &self.message);
        }
    }

    fn set_total(&mut self, total_size: u64) {
        self.total = total_size;
        if let Some(progress_bar) = &mut self.progress_bar {
            progress_bar.set_total(total_size);
        }
    }
}

impl Read for ProgressBarReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.reader.as_ref().expect("No reader set!").read(buf)?;
        self.progress += n as u64;
        if let Some(progress_bar) = &mut self.progress_bar {
            progress_bar.set_progress(self.progress, &self.message);
        }
        Ok(n)
    }
}

/// a function that sorts paths into two iterators, one that starts with `info/` and one that does not
/// both iterators are sorted alphabetically for reproducibility
fn sort_paths<'a>(paths: &'a [PathBuf], base_path: &'a Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let info = Path::new("info/");
    let (mut info_paths, mut other_paths): (Vec<_>, Vec<_>) = paths
        .iter()
        .map(|p| p.strip_prefix(base_path).unwrap())
        .map(Path::to_path_buf)
        .partition(|path| path.starts_with(info));

    info_paths.sort();
    other_paths.sort();

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

fn total_size(base_path: &Path, paths: &[PathBuf]) -> u64 {
    paths
        .iter()
        .map(|p| base_path.join(p).metadata().map(|m| m.len()).unwrap_or(0))
        .sum()
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
/// write_tar_bz2_package(&mut file, &PathBuf::from("test"), &paths, CompressionLevel::Default, None, None).unwrap();
/// ```
///
/// # See also
///
/// * [`write_conda_package`]
pub fn write_tar_bz2_package<W: Write>(
    writer: W,
    base_path: &Path,
    paths: &[PathBuf],
    compression_level: CompressionLevel,
    timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    progress_bar: Option<Box<dyn ProgressBar>>,
) -> Result<(), std::io::Error> {
    let mut archive = tar::Builder::new(bzip2::write::BzEncoder::new(
        writer,
        compression_level.to_bzip2_level()?,
    ));
    archive.follow_symlinks(false);

    let total_size = total_size(base_path, paths);
    let mut progress_bar_wrapper = ProgressBarReader::new(progress_bar);
    progress_bar_wrapper.set_total(total_size);

    // sort paths alphabetically, and sort paths beginning with `info/` first
    let (info_paths, other_paths) = sort_paths(paths, base_path);
    for path in info_paths.iter().chain(other_paths.iter()) {
        append_path_to_archive(
            &mut archive,
            base_path,
            path,
            timestamp,
            &mut progress_bar_wrapper,
        )?;
    }

    archive.into_inner()?.finish()?;

    Ok(())
}

/// Write the contents of a list of paths to a tar zst archive
fn write_zst_archive<W: Write>(
    writer: W,
    base_path: &Path,
    paths: &Vec<PathBuf>,
    compression_level: CompressionLevel,
    num_threads: Option<u32>,
    timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    progress_bar: Option<Box<dyn ProgressBar>>,
) -> Result<(), std::io::Error> {
    // Create a temporary tar file
    let tar_path = tempfile::Builder::new().tempfile_in(base_path)?;
    let mut archive = tar::Builder::new(&tar_path);
    archive.follow_symlinks(false);

    let total_size = total_size(base_path, paths);
    let mut progress_bar_wrapper = ProgressBarReader::new(progress_bar);
    progress_bar_wrapper.set_total(total_size);
    for path in paths {
        append_path_to_archive(
            &mut archive,
            base_path,
            path,
            timestamp,
            &mut progress_bar_wrapper,
        )?;
    }
    archive.finish()?;

    // Compress it as tar.zst
    let tar_file = File::open(&tar_path)?;
    let compression_level = compression_level.to_zstd_level()?;
    let mut zst_encoder = zstd::Encoder::new(writer, compression_level)?;
    zst_encoder.multithread(num_threads.unwrap_or_else(|| num_cpus::get() as u32))?;

    progress_bar_wrapper.reset_position();
    if let Ok(tar_total_size) = tar_file.metadata().map(|v| v.len()) {
        zst_encoder.set_pledged_src_size(Some(tar_total_size))?;
        progress_bar_wrapper.set_total(tar_total_size);
    };
    zst_encoder.include_contentsize(true)?;

    // Append tar.zst to the archive
    progress_bar_wrapper.set_file(tar_file);
    io::copy(&mut progress_bar_wrapper, &mut zst_encoder)?;
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
///    the number of CPU cores if `None`)
/// * `timestamp` - optional a timestamp to use for all archive files (useful for reproducible builds)
///
/// # Errors
///
/// This function will return an error if the writer returns an error, or if the paths are not
/// relative to the base path.
#[allow(clippy::too_many_arguments)]
pub fn write_conda_package<W: Write + Seek>(
    writer: W,
    base_path: &Path,
    paths: &[PathBuf],
    compression_level: CompressionLevel,
    compression_num_threads: Option<u32>,
    out_name: &str,
    timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    progress_bar: Option<Box<dyn ProgressBar>>,
) -> Result<(), std::io::Error> {
    // first create the outer zip archive that uses no compression
    let mut outer_archive = zip::ZipWriter::new(writer);

    let last_modified_time = if let Some(time) = timestamp {
        DateTime::from_date_and_time(
            time.year() as u16,
            time.month() as u8,
            time.day() as u8,
            time.hour() as u8,
            time.minute() as u8,
            time.second() as u8,
        )
        .expect("time should be in correct range")
    } else {
        // 1-1-2023 00:00:00 (Fixed date in the past for reproducible builds)
        DateTime::from_date_and_time(2023, 1, 1, 0, 0, 0)
            .expect("1-1-2023 00:00:00 should convert into datetime")
    };

    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(last_modified_time)
        .large_file(true);

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
        &other_paths,
        compression_level,
        compression_num_threads,
        timestamp,
        progress_bar,
    )?;

    // info paths come last
    let archive_path = format!("info-{out_name}.tar.zst");
    outer_archive.start_file(archive_path, options)?;
    write_zst_archive(
        &mut outer_archive,
        base_path,
        &info_paths,
        compression_level,
        compression_num_threads,
        timestamp,
        None,
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

fn append_path_to_archive(
    archive: &mut tar::Builder<impl Write>,
    base_path: &Path,
    path: &Path,
    timestamp: Option<&chrono::DateTime<chrono::Utc>>,
    progress_bar: &mut ProgressBarReader,
) -> Result<(), std::io::Error> {
    // create a tar header
    let mut header = prepare_header(&base_path.join(path), timestamp)
        .map_err(|err| trace_file_error(&base_path.join(path), err))?;

    if header.entry_type().is_file() {
        let file = fs::File::open(base_path.join(path))
            .map_err(|err| trace_file_error(&base_path.join(path), err))?;
        // wrap the file reader in a progress bar reader
        progress_bar.set_file(file);
        archive.append_data(&mut header, path, progress_bar)?;
    } else if header.entry_type().is_symlink() || header.entry_type().is_hard_link() {
        let target = fs::read_link(base_path.join(path))
            .map_err(|err| trace_file_error(&base_path.join(path), err))?;

        archive.append_link(&mut header, path, target)?;
    } else if header.entry_type().is_dir() {
        archive.append_data(&mut header, path, std::io::empty())?;
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unsupported file type",
        ));
    }

    Ok(())
}
