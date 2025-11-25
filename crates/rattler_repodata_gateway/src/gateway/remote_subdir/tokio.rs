use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    fetch::{fetch_repo_data, FetchRepoDataError, FetchRepoDataOptions, Variant},
    gateway::{
        error::SubdirNotFoundError, local_subdir::LocalSubdirClient, GatewayError, SourceConfig,
    },
    Reporter,
};
use rattler_conda_types::{Channel, Platform};
use rattler_networking::LazyClient;

pub struct RemoteSubdirClient {
    pub(super) sparse: LocalSubdirClient,
}

impl RemoteSubdirClient {
    pub async fn new(
        channel: Channel,
        platform: Platform,
        client: LazyClient,
        cache_dir: PathBuf,
        source_config: SourceConfig,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Self, GatewayError> {
        let subdir_url = channel.platform_url(platform);

        // Fetch the repodata from the remote server
        let repodata = fetch_repo_data(
            subdir_url,
            client,
            cache_dir,
            FetchRepoDataOptions {
                cache_action: source_config.cache_action,
                jlap_enabled: source_config.jlap_enabled,
                zstd_enabled: source_config.zstd_enabled,
                bz2_enabled: source_config.bz2_enabled,
                ..FetchRepoDataOptions::default()
            },
            reporter,
        )
        .await
        .map_err(|e| match e {
            FetchRepoDataError::NotFound(e) => {
                GatewayError::SubdirNotFoundError(Box::new(SubdirNotFoundError {
                    channel: channel.clone(),
                    subdir: platform.to_string(),
                    source: e.into(),
                }))
            }
            e => GatewayError::FetchRepoDataError(e),
        })?;

        // Create a new sparse repodata client that can be used to read records from the
        // repodata.
        let sparse = simple_spawn_blocking::tokio::run_blocking_task(move || {
            LocalSubdirClient::from_file(
                &repodata.repo_data_json_path,
                channel.clone(),
                platform.as_str(),
            )
        })
        .await?;

        Ok(Self { sparse })
    }

    /// Clears the on-disk cache for the given channel and platform.
    ///
    /// This removes all cached repodata files (JSON, info, and lock files) for
    /// the specified channel and platform combination.
    pub fn clear_cache(
        cache_dir: &Path,
        channel: &Channel,
        platform: Platform,
    ) -> Result<(), std::io::Error> {
        let subdir_url = channel.platform_url(platform);
        let cache_key = crate::utils::url_to_cache_filename(
            &subdir_url
                .join(Variant::default().file_name())
                .expect("valid filename"),
        );

        // Remove the cached repodata files
        let json_path = cache_dir.join(format!("{cache_key}.json"));
        let info_path = cache_dir.join(format!("{cache_key}.info.json"));
        let lock_path = cache_dir.join(format!("{cache_key}.lock"));

        for path in [json_path, info_path, lock_path] {
            if path.exists() {
                fs_err::remove_file(&path)?;
                tracing::debug!("deleted repodata cache file: {:?}", path);
            }
        }

        Ok(())
    }
}
