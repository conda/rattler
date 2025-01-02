//! Middleware to handle `s3://` URLs to pull artifacts from an S3 bucket
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_s3::presigning::{PresignedRequest, PresigningConfig};
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MiddlewareResult};
use url::Url;

/// S3 middleware to authenticate requests
pub struct S3Middleware {
    client: aws_sdk_s3::Client,
}

impl S3Middleware {
    /// Create a new S3 middleware
    pub async fn new(
        config_file: Option<&str>,
        profile: Option<&str>,
        force_path_style: Option<bool>,
    ) -> Self {
        let mut builder = aws_runtime::env_config::file::EnvConfigFiles::builder();
        if let Some(config_file) = config_file {
            builder = builder.with_file(
                aws_runtime::env_config::file::EnvConfigFileKind::Config,
                config_file,
            )
        }
        let env_config_files = builder.build();

        let mut builder = aws_config::defaults(BehaviorVersion::latest());
        if config_file.is_some() {
            builder = builder.profile_files(env_config_files)
        };
        if let Some(profile) = profile {
            builder = builder.profile_name(profile)
        };
        let sdk_config = builder.load().await;

        let mut builder = aws_sdk_s3::config::Builder::from(&sdk_config);
        if let Some(force_path_style) = force_path_style {
            builder = builder.force_path_style(force_path_style)
        };
        let s3_config = builder.build();

        let client: aws_sdk_s3::Client = aws_sdk_s3::Client::from_conf(s3_config);
        Self { client }
    }

    /// Generate a presigned S3 GetObject request
    async fn generate_presigned_s3_request(
        &self,
        bucket_name: &str,
        key: &str,
    ) -> MiddlewareResult<PresignedRequest> {
        let builder = self.client.get_object().bucket(bucket_name).key(key);
        Ok(builder
            .presigned(PresigningConfig::expires_in(std::time::Duration::from_secs(300)).unwrap())
            .await
            .unwrap())
    }
}

#[async_trait]
impl Middleware for S3Middleware {
    /// Create a new authentication middleware for S3
    async fn handle(
        &self,
        mut req: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> MiddlewareResult<Response> {
        if req.url().scheme() == "s3" {
            let url = req.url().clone();
            let bucket_name = url.host_str().expect("Host should be present in S3 URL");
            let key = url.path();
            let presigned_request = self.generate_presigned_s3_request(bucket_name, key).await?;

            *req.url_mut() = Url::parse(presigned_request.uri()).unwrap();
        }
        next.run(req, extensions).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_presigned_s3_request() {
        if std::env::var("AWS_ACCESS_KEY_ID").is_err() {
            eprintln!("Skipping test as AWS_ACCESS_KEY_ID is not set");
            return;
        };
        eprintln!("Running test");

        let middleware = S3Middleware::new(None, None, None).await;

        let presigned = middleware
            .generate_presigned_s3_request("rattler-s3-testing", "input.txt")
            .await
            .unwrap();
        assert!(
            presigned
                .uri()
                .to_string()
                .starts_with("https://rattler-s3-testing.s3.eu-central-1.amazonaws.com/input.txt?"),
            "Unexpected presigned URL: {:?}",
            presigned.uri()
        );
    }
}
