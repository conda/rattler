//! Conda-forge package uploader.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{CondaForgeData, upload::get_default_client};
use fs_err::tokio as fs;
use miette::{IntoDiagnostic, miette};
use tracing::{debug, info};

use super::{
    anaconda,
    package::{self},
};

async fn get_channel_target_from_variant_config(
    variant_config_path: &Path,
) -> miette::Result<String> {
    let variant_config = fs::read_to_string(variant_config_path)
        .await
        .into_diagnostic()?;

    let variant_config: serde_yaml::Value =
        serde_yaml::from_str(&variant_config).into_diagnostic()?;

    let channel_target = variant_config
        .get("channel_targets")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            miette!("\"channel_targets\" not found or invalid format in variant_config")
        })?;

    let (channel, label) = channel_target
        .split_once(' ')
        .ok_or_else(|| miette!("Invalid channel_target format"))?;

    if channel != "conda-forge" {
        return Err(miette!("channel_target is not a conda-forge channel"));
    }

    Ok(label.to_string())
}

/// Uploads the package conda forge.
pub async fn upload_packages_to_conda_forge(
    package_files: &Vec<PathBuf>,
    conda_forge_data: CondaForgeData,
) -> miette::Result<()> {
    let anaconda = anaconda::Anaconda::new(
        conda_forge_data.staging_token,
        conda_forge_data.anaconda_url,
    );

    let mut channels: HashMap<String, HashMap<_, _>> = HashMap::new();

    for package_file in package_files {
        let package = package::ExtractedPackage::from_package_file(package_file)?;

        let variant_config_path = package
            .extraction_dir()
            .join("info")
            .join("recipe")
            .join("variant_config.yaml");

        let channel = get_channel_target_from_variant_config(&variant_config_path)
            .await
            .map_err(|e| {
                miette!(
                    "Failed to get channel_targets from variant config for {}: {}",
                    package.path().display(),
                    e
                )
            })?;

        if !conda_forge_data.dry_run {
            anaconda
                .create_or_update_package(&conda_forge_data.staging_channel, &package)
                .await?;

            anaconda
                .create_or_update_release(&conda_forge_data.staging_channel, &package)
                .await?;

            anaconda
                .upload_file(
                    &conda_forge_data.staging_channel,
                    &[channel.clone()],
                    false,
                    &package,
                )
                .await?;
        } else {
            debug!(
                "Would have uploaded {} to anaconda.org {}/{}",
                package.path().display(),
                conda_forge_data.staging_channel,
                channel
            );
        };

        let dist_name = format!(
            "{}/{}",
            package.subdir().ok_or(miette::miette!("No subdir found"))?,
            package
                .filename()
                .ok_or(miette::miette!("No filename found"))?
        );

        channels
            .entry(channel)
            .or_default()
            .insert(dist_name, package.sha256().into_diagnostic()?);
    }

    for (channel, checksums) in channels {
        info!("Uploading packages for conda-forge channel {}", channel);

        let comment_on_error = std::env::var("POST_COMMENT_ON_ERROR").is_ok();

        let payload = serde_json::json!({
            "feedstock": conda_forge_data.feedstock,
            "outputs": checksums,
            "channel": channel,
            "comment_on_error": comment_on_error,
            "hash_type": "sha256",
            "provider": conda_forge_data.provider
        });

        let client = get_default_client().into_diagnostic()?;

        debug!(
            "Sending payload to validation endpoint: {}",
            serde_json::to_string_pretty(&payload).into_diagnostic()?
        );

        if conda_forge_data.dry_run {
            debug!(
                "Would have sent payload to validation endpoint {}",
                conda_forge_data.validation_endpoint
            );

            continue;
        }

        let resp = client
            .post(conda_forge_data.validation_endpoint.clone())
            .json(&payload)
            .header("FEEDSTOCK_TOKEN", conda_forge_data.feedstock_token.clone())
            .send()
            .await
            .into_diagnostic()?;

        let status = resp.status();

        let body: serde_json::Value = resp.json().await.into_diagnostic()?;

        debug!(
            "Copying to conda-forge/{} returned status code {} with body: {}",
            channel,
            status,
            serde_json::to_string_pretty(&body).into_diagnostic()?
        );
    }

    info!("Done uploading packages to conda-forge");

    Ok(())
}
