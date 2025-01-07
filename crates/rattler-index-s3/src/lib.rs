use url::Url;

/// Configuration options for indexing S3-based channel.
#[derive(Debug)]
pub struct IndexS3Config {
    /// The S3 channel URL, e.g. `s3://my-bucket/my-channel`.
    pub channel: Url,
    /// The endpoint URL to use for the S3 client.
    pub endpoint_url: Option<Url>,
    /// The region to use for the S3 client.
    pub region: Option<String>,
    /// Whether to force path style for the S3 client.
    pub force_path_style: Option<bool>,
}

pub async fn rattler_index_s3(config: IndexS3Config) -> anyhow::Result<()> {
    // 1. Create the S3 client
    // 2. List all files in the channel
    // 3. If there are any repodata files, download the most recent ones
    // 4. Parse the repodata files

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rattler_index_s3() {
        let result = rattler_index_s3(IndexS3Config {
            channel: Url::parse("s3://my-bucket/my-channel").unwrap(),
            endpoint_url: None,
            region: None,
            force_path_style: None,
        })
        .await;

        assert!(result.is_ok());
    }
}
