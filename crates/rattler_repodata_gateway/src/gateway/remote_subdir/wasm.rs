use std::sync::Arc;

use rattler_conda_types::{Channel, Platform};
use rattler_networking::LazyClient;

use crate::{
    Reporter,
    fetch::{
        FetchRepoDataError, Variant,
        no_cache::{FetchRepoDataOptions, fetch_repo_data, fetch_repo_data_js},
    },
    gateway::{
        GatewayError, SourceConfig, error::SubdirNotFoundError, local_subdir::LocalSubdirClient,
    },
    utils::js_fetch::JsFetcher,
};

pub struct RemoteSubdirClient {
    pub(super) sparse: LocalSubdirClient,
}

impl RemoteSubdirClient {
    pub async fn new(
        channel: Channel,
        platform: Platform,
        client: LazyClient,
        js_fetch: Option<JsFetcher>,
        source_config: SourceConfig,
        reporter: Option<Arc<dyn Reporter>>,
    ) -> Result<Self, GatewayError> {
        let subdir_url = channel.platform_url(platform);
        let variants = source_config
            .repodata_variants
            .filter(|variants| !variants.is_empty())
            .unwrap_or_else(|| indexmap::IndexSet::from([Variant::default()]));
        let variant_count = variants.len();
        let mut repodata_bytes = None;
        for (index, variant) in variants.into_iter().enumerate() {
            let options = FetchRepoDataOptions {
                variant,
                zstd_enabled: source_config.zstd_enabled,
                bz2_enabled: source_config.bz2_enabled,
                ..FetchRepoDataOptions::default()
            };
            let result = match js_fetch.clone() {
                Some(fetcher) => fetch_repo_data_js(subdir_url.clone(), fetcher, options).await,
                None => {
                    fetch_repo_data(
                        subdir_url.clone(),
                        client.clone(),
                        options,
                        reporter.clone(),
                    )
                    .await
                }
            };
            match result {
                Ok(value) => {
                    repodata_bytes = Some(value);
                    break;
                }
                Err(FetchRepoDataError::NotFound(_)) if index + 1 < variant_count => {}
                Err(FetchRepoDataError::NotFound(error)) => {
                    return Err(GatewayError::SubdirNotFoundError(Box::new(
                        SubdirNotFoundError {
                            channel: channel.clone(),
                            subdir: platform.to_string(),
                            source: error.into(),
                        },
                    )));
                }
                Err(error) => return Err(GatewayError::FetchRepoDataError(error)),
            }
        }
        let repodata_bytes = repodata_bytes.expect("repodata variant lists are non-empty");

        // Create a new sparse repodata client that can be used to read records from the
        // repodata.
        let sparse =
            LocalSubdirClient::from_bytes(repodata_bytes, channel.clone(), platform.as_str())?;

        Ok(Self { sparse })
    }
}
