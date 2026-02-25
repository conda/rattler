use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use crate::{
    fetch::{fetch_repo_data, FetchRepoDataError, FetchRepoDataOptions, Variant},
    gateway::{
        error::SubdirNotFoundError, local_subdir::LocalSubdirClient, GatewayError, SourceConfig,
    },
    Reporter,
};
use cache_control::{Cachability, CacheControl};
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
    ) -> Result<(Self, Option<SystemTime>), GatewayError> {
        let subdir_url = channel.platform_url(platform);

        // Fetch the repodata from the remote server
        let repodata = fetch_repo_data(
            subdir_url,
            client,
            cache_dir,
            FetchRepoDataOptions {
                cache_action: source_config.cache_action,
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

        let expires_at = cache_expires_at(
            repodata.cache_state.cache_headers.cache_control.as_deref(),
            repodata.cache_state.cache_last_modified,
        );

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

        Ok((Self { sparse }, expires_at))
    }

    /// Clears the on-disk cache for the given channel and platform.
    ///
    /// This removes all cached repodata files (JSON and info files) for
    /// the specified channel and platform combination. The lock file is
    /// retained to avoid the ABA problem with concurrent processes.
    ///
    /// If the cache directory or lock file doesn't exist, this is a no-op
    /// since there's nothing to clear.
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

        // Acquire a lock before modifying the cache files.
        // If the lock file doesn't exist (e.g., parent directory doesn't exist),
        // then there's no cache to clear, so we can return early.
        let lock_path = cache_dir.join(format!("{cache_key}.lock"));
        let _lock = match crate::utils::LockedFile::open_rw(&lock_path, "repodata cache clear") {
            Ok(lock) => lock,
            Err(_) => {
                // Lock file doesn't exist or can't be created - no cache to clear
                return Ok(());
            }
        };

        // Remove the cached repodata files (but NOT the lock file)
        let json_path = cache_dir.join(format!("{cache_key}.json"));
        let info_path = cache_dir.join(format!("{cache_key}.info.json"));

        for path in [json_path, info_path] {
            if path.exists() {
                fs_err::remove_file(&path)?;
                tracing::debug!("deleted repodata cache file: {:?}", path);
            }
        }

        Ok(())
    }
}

fn cache_expires_at(
    cache_control: Option<&str>,
    cache_last_modified: SystemTime,
) -> Option<SystemTime> {
    let cache_control = match cache_control.and_then(CacheControl::from_value) {
        Some(cache_control) => cache_control,
        None => return Some(SystemTime::now()),
    };

    match cache_control {
        CacheControl {
            cachability: Some(Cachability::Public),
            max_age: Some(duration),
            ..
        } => cache_last_modified
            .checked_add(duration)
            .or(Some(SystemTime::now())),
        _ => Some(SystemTime::now()),
    }
}
