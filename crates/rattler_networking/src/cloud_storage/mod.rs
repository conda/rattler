use std::path::Path;
use async_trait::async_trait;
use url::Url;
use reqwest::Response;
use thiserror::Error;

/// Errors that can occur when interacting with cloud storage
#[derive(Error, Debug)]
pub enum CloudStorageError {
    #[error("Authentication failed: {0}")]
    AuthenticationError(String),
    
    #[error("Failed to upload file: {0}")]
    UploadError(String),
    
    #[error("Failed to download file: {0}")]
    DownloadError(String),
    
    #[error("Invalid configuration: {0}")]
    ConfigurationError(String),
    
    #[error("Provider not supported: {0}")]
    UnsupportedProvider(String),
}

/// Supported cloud storage providers
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudProvider {
    AWS,
    Azure,
    GCP,
}

/// Configuration for cloud storage
#[derive(Debug, Clone)]
pub struct CloudStorageConfig {
    pub provider: CloudProvider,
    pub bucket: String,
    pub region: Option<String>,
    pub credentials: CloudCredentials,
}

/// Credentials for cloud storage authentication
#[derive(Debug, Clone)]
pub struct CloudCredentials {
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub token: Option<String>,
}

/// Trait defining cloud storage operations
#[async_trait]
pub trait CloudStorage: Send + Sync {
    /// Upload a file to cloud storage
    async fn upload_file(&self, local_path: &Path, remote_path: &str) -> Result<Url, CloudStorageError>;
    
    /// Download a file from cloud storage
    async fn download_file(&self, remote_url: &Url, local_path: &Path) -> Result<(), CloudStorageError>;
    
    /// Check if a file exists in cloud storage
    async fn file_exists(&self, remote_path: &str) -> Result<bool, CloudStorageError>;
    
    /// Get a presigned URL for downloading a file
    async fn get_download_url(&self, remote_path: &str, expiry_secs: u32) -> Result<Url, CloudStorageError>;
}

/// Create a new cloud storage client based on configuration
pub fn create_cloud_storage(config: CloudStorageConfig) -> Result<Box<dyn CloudStorage>, CloudStorageError> {
    match config.provider {
        CloudProvider::AWS => {
            // We'll implement AWS S3 support first
            todo!("AWS S3 support coming soon")
        }
        CloudProvider::Azure => {
            todo!("Azure Blob Storage support coming soon")
        }
        CloudProvider::GCP => {
            todo!("Google Cloud Storage support coming soon")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cloud_storage_config() {
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
        
        assert_eq!(config.provider, CloudProvider::AWS);
        assert_eq!(config.bucket, "test-bucket");
        assert_eq!(config.region, Some("us-east-1".to_string()));
    }
} 