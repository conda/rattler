use std::{io::Write, path::PathBuf};

use futures_util::StreamExt;
use miette::{Context, IntoDiagnostic};
use tokio::io::AsyncWriteExt;
use url::Url;

/// Download an arbitrary file.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// URL of the file to download
    #[clap(required = true)]
    url: Url,

    /// Output path for the downloaded file, or '-' to write to stdout
    #[clap(short, long)]
    output: Option<PathBuf>,
}

fn default_output_path(url: &Url) -> miette::Result<PathBuf> {
    let file_name = url
        .path_segments()
        .and_then(Iterator::last)
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| miette::miette!("could not infer output filename from URL path"))?;

    Ok(PathBuf::from(file_name))
}

pub async fn download(opt: Opt, offline: bool) -> miette::Result<()> {
    let output = match opt.output {
        Some(output) => output,
        None => default_output_path(&opt.url)?,
    };
    let write_to_stdout = output.as_os_str() == "-";

    let client = super::client::create_client_with_middleware(offline)?;

    let response = client
        .get(opt.url.clone())
        .send()
        .await
        .into_diagnostic()
        .with_context(|| format!("failed to download {}", opt.url))?
        .error_for_status()
        .into_diagnostic()
        .with_context(|| format!("server returned an error for {}", opt.url))?;
    let mut stream = response.bytes_stream();

    if write_to_stdout {
        let mut stdout = std::io::stdout();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .into_diagnostic()
                .with_context(|| format!("failed to read response body from {}", opt.url))?;
            stdout
                .write_all(&chunk)
                .into_diagnostic()
                .context("failed to write to stdout")?;
        }
        stdout
            .flush()
            .into_diagnostic()
            .context("failed to flush stdout")?;
    } else {
        // Download into a temporary file next to the output and only move it
        // into place once the download completed. An interrupted download
        // otherwise leaves a truncated file at the output path that looks
        // complete, and the temporary file is cleaned up automatically.
        let output_dir = match output.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => std::path::Path::new("."),
        };
        let temp_path = tempfile::Builder::new()
            .prefix(".rattler-download-")
            .tempfile_in(output_dir)
            .into_diagnostic()
            .with_context(|| {
                format!(
                    "failed to create a temporary file in {}",
                    output_dir.display()
                )
            })?
            .into_temp_path();

        let mut file = tokio::fs::File::create(&temp_path)
            .await
            .into_diagnostic()
            .with_context(|| format!("failed to open {}", temp_path.display()))?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .into_diagnostic()
                .with_context(|| format!("failed to read response body from {}", opt.url))?;
            file.write_all(&chunk)
                .await
                .into_diagnostic()
                .with_context(|| format!("failed to write {}", temp_path.display()))?;
        }
        file.flush()
            .await
            .into_diagnostic()
            .with_context(|| format!("failed to flush {}", temp_path.display()))?;
        drop(file);

        temp_path
            .persist(&output)
            .into_diagnostic()
            .with_context(|| format!("failed to move the download to {}", output.display()))?;

        eprintln!("Downloaded {} to {}", opt.url, output.display());
    }
    Ok(())
}
