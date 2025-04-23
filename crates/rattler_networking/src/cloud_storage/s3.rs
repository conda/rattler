use std::path::Path;
use async_trait::async_trait;
use aws_sdk_s3::{Client, Region, Credentials};
use aws_sdk_s3::config::Config;
use url::Url;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use super::{CloudStorage, CloudStorageError, CloudStorageConfig};

pub struct S3Storage {
    client: Client,
    bucket: String,
    region: String,
}

impl S3Storage {
    pub async fn new(config: &CloudStorageConfig) -> Result<Self, CloudStorageError> {
        let region = config.region.clone()
            .ok_or_else(|| CloudStorageError::ConfigurationError("AWS region is required".to_string()))?;
            
        let credentials = if let (Some(access_key), Some(secret_key)) = (
            config.credentials.access_key.clone(),
            config.credentials.secret_key.clone(),
        ) {
            Credentials::new(
                access_key,
                secret_key,
                config.credentials.token.clone(),
                None,
                "rattler-s3-provider",
            )
        } else {
            return Err(CloudStorageError::AuthenticationError(
                "AWS credentials are required".to_string(),
            ));
        };

        let s3_config = Config::builder()
            .region(Region::new(region.clone()))
            .credentials_provider(credentials)
            .build();

        let client = Client::from_conf(s3_config);

        Ok(Self {
            client,
            bucket: config.bucket.clone(),
            region,
        })
    }
}

#[async_trait]
impl CloudStorage for S3Storage {
    async fn upload_file(&self, local_path: &Path, remote_path: &str) -> Result<Url, CloudStorageError> {
        let mut file = File::open(local_path)
            .await
            .map_err(|e| CloudStorageError::UploadError(format!("Failed to open local file: {}", e)))?;

        let mut contents = Vec::new();
        file.read_to_end(&mut contents)
            .await
            .map_err(|e| CloudStorageError::UploadError(format!("Failed to read local file: {}", e)))?;

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(remote_path)
            .body(contents.into())
            .send()
            .await
            .map_err(|e| CloudStorageError::UploadError(format!("Failed to upload to S3: {}", e)))?;

        // Construct the S3 URL
        let url = format!(
            "s3://{}.s3.{}.amazonaws.com/{}",
            self.bucket, self.region, remote_path
        );
        
        Url::parse(&url)
            .map_err(|e| CloudStorageError::UploadError(format!("Failed to create S3 URL: {}", e)))
    }

    async fn download_file(&self, remote_url: &Url, local_path: &Path) -> Result<(), CloudStorageError> {
        let key = remote_url.path().trim_start_matches('/');
        
        let response = self.client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| CloudStorageError::DownloadError(format!("Failed to download from S3: {}", e)))?;

        let data = response.body.collect().await
            .map_err(|e| CloudStorageError::DownloadError(format!("Failed to read S3 response: {}", e)))?;

        tokio::fs::write(local_path, data.into_bytes())
            .await
            .map_err(|e| CloudStorageError::DownloadError(format!("Failed to write local file: {}", e)))
    }

    async fn file_exists(&self, remote_path: &str) -> Result<bool, CloudStorageError> {
        match self.client
            .head_object()
            .bucket(&self.bucket)
            .key(remote_path)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(err) => {
                if err.to_string().contains("NotFound") {
                    Ok(false)
                } else {
                    Err(CloudStorageError::DownloadError(format!("Failed to check file existence: {}", err)))
                }
            }
        }
    }

    async fn get_download_url(&self, remote_path: &str, expiry_secs: u32) -> Result<Url, CloudStorageError> {
        let presigned_req = self.client
            .get_object()
            .bucket(&self.bucket)
            .key(remote_path)
            .presigned(aws_sdk_s3::presigning::PresigningConfig::expires_in(
                std::time::Duration::from_secs(expiry_secs as u64),
            )
            .map_err(|e| CloudStorageError::DownloadError(format!("Failed to create presigned URL: {}", e)))?)
            .await
            .map_err(|e| CloudStorageError::DownloadError(format!("Failed to create presigned URL: {}", e)))?;

        Ok(presigned_req.uri().to_string().parse().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_storage::{CloudProvider, CloudCredentials};

    #[tokio::test]
    async fn test_s3_client_creation() {
        let config = CloudStorageConfig {
            provider: CloudProvider::AWS,
            bucket: "test-bucket".to_string(),
            region: Some("us-east-1".to_string()),
            credentials: CloudCredentials {
                access_key: Some("test-key".to_string()),
                secret_key: Some("test-secret".to_string()),
                token: None,
            },
        };

        let result = S3Storage::new(&config).await;
        assert!(result.is_ok());
    }
} 