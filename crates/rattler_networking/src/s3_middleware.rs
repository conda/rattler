//! Middleware to handle `s3://` URLs to pull artifacts from an S3 bucket
use std::path::PathBuf;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_s3::presigning::{PresignedRequest, PresigningConfig};
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MiddlewareResult};
use url::Url;

/// S3 middleware to authenticate requests.
pub struct S3Middleware {
    config_file: Option<PathBuf>,
    profile: Option<String>,
    force_path_style: Option<bool>,
    expiration: std::time::Duration,
}

async fn create_client(
    config_file: Option<PathBuf>,
    profile: Option<String>,
    force_path_style: Option<bool>,
) -> aws_sdk_s3::Client {
    let mut aws_config_builder = aws_config::defaults(BehaviorVersion::latest());
    if let Some(config_file) = config_file {
        let mut builder = aws_runtime::env_config::file::EnvConfigFiles::builder();
        builder = builder.with_file(
            aws_runtime::env_config::file::EnvConfigFileKind::Config,
            config_file,
        );
        let env_config_files = builder.build();
        aws_config_builder = aws_config_builder.profile_files(env_config_files);
    }

    if let Some(profile) = profile {
        aws_config_builder = aws_config_builder.profile_name(profile);
    };
    let sdk_config = aws_config_builder.load().await;

    let mut builder = aws_sdk_s3::config::Builder::from(&sdk_config);
    if let Some(force_path_style) = force_path_style {
        builder = builder.force_path_style(force_path_style);
    };
    let s3_config = builder.build();

    let client: aws_sdk_s3::Client = aws_sdk_s3::Client::from_conf(s3_config);
    client
}

impl S3Middleware {
    /// Create a new S3 middleware.
    pub fn new(
        config_file: Option<PathBuf>,
        profile: Option<String>,
        force_path_style: Option<bool>,
    ) -> Self {
        Self {
            config_file,
            profile,
            force_path_style,
            expiration: std::time::Duration::from_secs(300),
        }
    }

    /// Generate a presigned S3 `GetObject` request.
    async fn generate_presigned_s3_request(
        &self,
        bucket_name: &str,
        key: &str,
    ) -> MiddlewareResult<PresignedRequest> {
        let client = create_client(
            self.config_file.clone(),
            self.profile.clone(),
            self.force_path_style,
        )
        .await;
        let builder = client.get_object().bucket(bucket_name).key(key);
        builder
            .presigned(
                PresigningConfig::expires_in(self.expiration)
                    .map_err(reqwest_middleware::Error::middleware)?,
            )
            .await
            .map_err(reqwest_middleware::Error::middleware)
    }
}

#[async_trait]
impl Middleware for S3Middleware {
    /// Create a new authentication middleware for S3.
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

            *req.url_mut() = Url::parse(presigned_request.uri())
                .map_err(reqwest_middleware::Error::middleware)?;
        }
        next.run(req, extensions).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use rstest::{fixture, rstest};
    use serial_test::serial;
    use tempfile::{tempdir, TempDir};

    async fn with_env(
        env: HashMap<&str, &str>,
        f: impl FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    ) {
        for (key, value) in &env {
            std::env::set_var(key, value);
        }
        f().await;
        for (key, _) in env {
            std::env::remove_var(key);
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_presigned_s3_request() {
        with_env(
            HashMap::from([
                ("AWS_ACCESS_KEY_ID", "minioadmin"),
                ("AWS_SECRET_ACCESS_KEY", "minioadmin"),
                ("AWS_REGION", "eu-central-1"),
                ("AWS_ENDPOINT_URL", "http://localhost:9000"),
            ]),
            move || {
                Box::pin(async {
                    let middleware = S3Middleware::new(None, None, Some(true));

                    let presigned = middleware
                        .generate_presigned_s3_request(
                            "rattler-s3-testing",
                            "my-channel/noarch/repodata.json",
                        )
                        .await
                        .unwrap();
                    assert!(
                        presigned.uri().to_string().starts_with(
                            "http://localhost:9000/rattler-s3-testing/my-channel/noarch/repodata.json?"
                        ),
                        "Unexpected presigned URL: {:?}",
                        presigned.uri()
                    );
                })
            },
        )
        .await;
    }

    #[tokio::test]
    #[serial]
    async fn test_presigned_s3_request_aws() {
        with_env(
            HashMap::from([
                ("AWS_ACCESS_KEY_ID", "minioadmin"),
                ("AWS_SECRET_ACCESS_KEY", "minioadmin"),
                ("AWS_REGION", "eu-central-1"),
            ]),
            move || {
                Box::pin(async {
                    let middleware = S3Middleware::new(None, None, None);

                    let presigned = middleware
                        .generate_presigned_s3_request(
                            "rattler-s3-testing",
                            "my-channel/noarch/repodata.json",
                        )
                        .await
                        .unwrap();
                    assert!(
                        presigned.uri().to_string().starts_with(
                            "https://rattler-s3-testing.s3.eu-central-1.amazonaws.com/my-channel/noarch/repodata.json?"
                        ),
                        "Unexpected presigned URL: {:?}",
                        presigned.uri()
                    );
                })
            },
        )
        .await;
    }

    #[fixture]
    fn aws_config() -> (TempDir, std::path::PathBuf) {
        let temp_dir = tempdir().unwrap();
        let aws_config = r#"
[profile default]
aws_access_key_id = minioadmin
aws_secret_access_key = minioadmin
region = eu-central-1

[profile packages]
aws_access_key_id = minioadmin
aws_secret_access_key = minioadmin
endpoint_url = http://localhost:8000
region = eu-central-1
"#;
        let aws_config_path = temp_dir.path().join("aws.config");
        std::fs::write(&aws_config_path, aws_config).unwrap();
        (temp_dir, aws_config_path)
    }

    #[rstest]
    #[tokio::test]
    #[serial]
    async fn test_presigned_s3_request_custom_config(aws_config: (TempDir, std::path::PathBuf)) {
        let middleware = S3Middleware::new(Some(aws_config.1), None, None);

        let presigned = middleware
            .generate_presigned_s3_request("rattler-s3-testing", "my-channel/noarch/repodata.json")
            .await
            .unwrap();
        assert!(
            presigned.uri().to_string().starts_with(
                "https://rattler-s3-testing.s3.eu-central-1.amazonaws.com/my-channel/noarch/repodata.json?"
            ),
            "Unexpected presigned URL: {:?}",
            presigned.uri()
        );
    }

    #[rstest]
    #[tokio::test]
    #[serial]
    async fn test_presigned_s3_request_different_profile(
        aws_config: (TempDir, std::path::PathBuf),
    ) {
        let middleware = S3Middleware::new(Some(aws_config.1), Some("packages".into()), None);

        let presigned = middleware
            .generate_presigned_s3_request("rattler-s3-testing", "my-channel/noarch/repodata.json")
            .await
            .unwrap();
        assert!(
            presigned.uri().to_string().contains("localhost:8000"),
            "Unexpected presigned URL: {:?}",
            presigned.uri()
        );
    }

    #[rstest]
    #[tokio::test]
    #[serial]
    async fn test_presigned_s3_request_custom_config_from_env(
        aws_config: (TempDir, std::path::PathBuf),
    ) {
        with_env(
            HashMap::from([
                ("AWS_CONFIG_FILE", aws_config.1.to_str().unwrap()),
                ("AWS_PROFILE", "packages"),
            ]),
            move || {
                Box::pin(async move {
                    let middleware = S3Middleware::new(None, None, None);

                    let presigned = middleware
                        .generate_presigned_s3_request(
                            "rattler-s3-testing",
                            "my-channel/noarch/repodata.json",
                        )
                        .await
                        .unwrap();
                    assert!(
                        presigned.uri().to_string().contains("localhost:8000"),
                        "Unexpected presigned URL: {:?}",
                        presigned.uri()
                    );
                })
            },
        )
        .await;
    }

    /// Test that environment variables take precedence over the configuration file.
    #[rstest]
    #[tokio::test]
    #[serial]
    async fn test_presigned_s3_request_env_precedence(aws_config: (TempDir, std::path::PathBuf)) {
        with_env(
            HashMap::from([("AWS_ENDPOINT_URL", "http://localhost:9000")]),
            move || {
                let aws_config_path = aws_config.1;
                Box::pin(async move {
                    let middleware =
                        S3Middleware::new(Some(aws_config_path), Some("default".into()), None);

                    let presigned = middleware
                        .generate_presigned_s3_request(
                            "rattler-s3-testing",
                            "my-channel/noarch/repodata.json",
                        )
                        .await
                        .unwrap();
                    assert!(
                        presigned.uri().to_string().contains("localhost:9000"),
                        "Unexpected presigned URL: {:?}",
                        presigned.uri()
                    );
                })
            },
        )
        .await;
    }
}
