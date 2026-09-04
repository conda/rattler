use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use futures_util::{StreamExt, stream};
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::package::{CondaArchiveIdentifier, CondaArchiveType};
use rattler_package_streaming::ExtractResult;
use reqwest_middleware::ClientWithMiddleware;
use url::Url;

/// Extract one or more local or remote conda packages.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Paths or URLs to conda package archives (.tar.bz2 or .conda)
    #[clap(required = true)]
    packages: Vec<String>,

    /// Destination directory. With a single package this is the directory the
    /// package is extracted into. With multiple packages each package is
    /// extracted into a subdirectory of this directory named after the package.
    /// Defaults to the current directory.
    #[clap(short, long)]
    destination: Option<PathBuf>,

    /// How local files are read. Remote packages always stream.
    #[clap(long, value_enum, default_value_t = Mode::Sync)]
    mode: Mode,

    /// Number of packages to extract concurrently.
    #[clap(long, default_value_t = 1)]
    concurrency: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    /// Read the file directly on the extraction thread, like the package
    /// cache does for local files.
    Sync,
    /// Stream the file through the same path downloads take: an async reader
    /// feeding an extraction worker.
    Async,
}

/// A package to extract, either a local file or a remote URL.
#[derive(Debug, Clone)]
enum Source {
    Url(Url),
    Path(PathBuf),
}

impl Source {
    fn parse(package: &str) -> Self {
        match Url::parse(package) {
            Ok(url) if url.scheme().len() > 1 => Self::Url(url),
            _ => Self::Path(PathBuf::from(package)),
        }
    }

    /// The directory name to extract into when no explicit destination is
    /// given: the archive identifier (`name-version-build`).
    fn package_name(&self) -> miette::Result<String> {
        let identifier = match self {
            Self::Url(url) => CondaArchiveIdentifier::try_from_url(url),
            Self::Path(path) => CondaArchiveIdentifier::try_from_path(path),
        };
        identifier
            .map(|identifier| identifier.identifier.to_string())
            .ok_or_else(|| miette::miette!("{self} is not a conda package archive name"))
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(url) => write!(f, "{url}"),
            Self::Path(path) => write!(f, "{}", path.display()),
        }
    }
}

async fn extract_one(
    source: Source,
    destination: PathBuf,
    mode: Mode,
    client: &ClientWithMiddleware,
) -> miette::Result<(Source, PathBuf, ExtractResult, Duration)> {
    let start = Instant::now();
    let result = match (&source, mode) {
        (Source::Url(url), _) => {
            rattler_package_streaming::reqwest::tokio::extract(
                client.clone(),
                url.clone(),
                &destination,
                None,
                None,
            )
            .await
        }
        (Source::Path(path), Mode::Async) => {
            use rattler_package_streaming::tokio::async_read;
            let archive_type = CondaArchiveType::try_from(path.as_path())
                .ok_or(rattler_package_streaming::ExtractError::UnsupportedArchiveType);
            match archive_type {
                Err(err) => Err(err),
                Ok(archive_type) => match tokio::fs::File::open(path).await {
                    Err(err) => Err(rattler_package_streaming::ExtractError::IoError(err)),
                    Ok(file) => match archive_type {
                        CondaArchiveType::TarBz2 => {
                            async_read::extract_tar_bz2(file, &destination).await
                        }
                        CondaArchiveType::Conda => {
                            async_read::extract_conda(file, &destination).await
                        }
                    },
                },
            }
        }
        (Source::Path(path), Mode::Sync) => {
            let path = path.clone();
            let destination = destination.clone();
            tokio::task::spawn_blocking(move || {
                rattler_package_streaming::fs::extract(&path, &destination)
            })
            .await
            .into_diagnostic()?
        }
    }
    .into_diagnostic()
    .with_context(|| format!("Failed to extract package: {source}"))?;

    Ok((source, destination, result, start.elapsed()))
}

pub async fn extract(opt: Opt, offline: bool) -> miette::Result<()> {
    let sources: Vec<Source> = opt.packages.iter().map(|p| Source::parse(p)).collect();

    let jobs = if let [source] = sources.as_slice() {
        let destination = match opt.destination {
            Some(destination) => destination,
            None => PathBuf::from(source.package_name()?),
        };
        vec![(source.clone(), destination)]
    } else {
        let base = opt.destination.unwrap_or_else(|| PathBuf::from("."));
        sources
            .iter()
            .map(|source| Ok((source.clone(), base.join(source.package_name()?))))
            .collect::<miette::Result<Vec<_>>>()?
    };

    let concurrency = opt.concurrency.max(1);
    let mode = opt.mode;
    let total = jobs.len();
    let client = super::client::create_client_with_middleware(offline)?;
    let mut results = stream::iter(jobs)
        .map(|(source, destination)| extract_one(source, destination, mode, &client))
        .buffer_unordered(concurrency);

    // Report each package as it finishes; a failure does not hide the others.
    let mut extracted = 0;
    let mut first_error = None;
    while let Some(result) = results.next().await {
        match result {
            Ok((source, destination, result, elapsed)) => {
                extracted += 1;
                println!(
                    "{} {} -> {} ({:.1?})",
                    console::style("✓").green(),
                    source,
                    destination.display(),
                    elapsed
                );
                println!("  SHA256: {}", hex::encode(result.sha256));
                println!("  MD5: {}", hex::encode(result.md5));
                println!("  Size: {} bytes", result.total_size);
            }
            Err(err) => {
                eprintln!("{} {err:?}", console::style("✗").red());
                first_error.get_or_insert(err);
            }
        }
    }

    if total > 1 {
        println!("Extracted {extracted} of {total} packages");
    }

    match first_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}
