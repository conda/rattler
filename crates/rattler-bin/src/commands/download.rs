use std::{io::Write, path::PathBuf};

use miette::{Context, IntoDiagnostic};
use tokio::io::AsyncWriteExt;
use url::Url;

#[derive(Debug, clap::Parser)]
#[command(about = "Download an arbitrary file")]
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

pub async fn download(opt: Opt) -> miette::Result<()> {
    let output = match opt.output {
        Some(output) => output,
        None => default_output_path(&opt.url)?,
    };
    let write_to_stdout = output.as_os_str() == "-";

    let client = super::client::create_client_with_middleware()?;

    let response = client
        .get(opt.url.clone())
        .send()
        .await
        .into_diagnostic()
        .with_context(|| format!("failed to download {}", opt.url))?
        .error_for_status()
        .into_diagnostic()
        .with_context(|| format!("server returned an error for {}", opt.url))?;

    let bytes = response
        .bytes()
        .await
        .into_diagnostic()
        .with_context(|| format!("failed to read response body from {}", opt.url))?;

    if write_to_stdout {
        let mut stdout = std::io::stdout();
        stdout
            .write_all(&bytes)
            .into_diagnostic()
            .context("failed to write to stdout")?;
        stdout
            .flush()
            .into_diagnostic()
            .context("failed to flush stdout")?;
    } else {
        let mut file = tokio::fs::File::create(&output)
            .await
            .into_diagnostic()
            .with_context(|| format!("failed to create {}", output.display()))?;
        file.write_all(&bytes)
            .await
            .into_diagnostic()
            .with_context(|| format!("failed to write {}", output.display()))?;
        file.flush()
            .await
            .into_diagnostic()
            .with_context(|| format!("failed to flush {}", output.display()))?;

        println!("Downloaded {} to {}", opt.url, output.display());
    }
    Ok(())
}
