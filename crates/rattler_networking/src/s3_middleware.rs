//! Middleware to handle `s3://` URLs to pull artifacts from an S3 bucket
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_s3::presigning::{PresignedRequest, PresigningConfig};
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MiddlewareResult};
use url::Url;

use crate::{Authentication, AuthenticationStorage};

/// S3 middleware to authenticate requests.
pub struct S3Middleware {
    config: Option<S3Config>,
    expiration: std::time::Duration,
}

/// Configuration for the S3 middleware.
#[derive(Clone, Debug)]
pub struct S3Config {
    /// The authentication storage to use for the S3 client.
    pub auth_storage: AuthenticationStorage,
    /// The endpoint URL to use for the S3 client.
    pub endpoint_url: Url,
    /// The region to use for the S3 client.
    pub region: String,
    /// Whether to force path style for the S3 client.
    pub force_path_style: bool,
}

/// Create an S3 client for given channel with provided configuration.
pub async fn create_s3_client(config: Option<S3Config>, url: Option<Url>) -> aws_sdk_s3::Client {
    let sdk_config = aws_config::defaults(BehaviorVersion::latest()).load().await;
    if let (Some(config), Some(url)) = (config, url) {
        let auth = config.auth_storage.get_by_url(url).unwrap(); // todo
        let config_builder = match auth {
            (
                _,
                Some(Authentication::S3Credentials {
                    access_key_id,
                    secret_access_key,
                    session_token,
                }),
            ) => aws_sdk_s3::config::Builder::from(&sdk_config)
                .endpoint_url(config.endpoint_url)
                .region(aws_sdk_s3::config::Region::new(config.region))
                .force_path_style(config.force_path_style)
                .credentials_provider(aws_sdk_s3::config::Credentials::new(
                    access_key_id,
                    secret_access_key,
                    session_token,
                    None,
                    "pixi",
                )),
            (_, Some(_)) => {
                panic!("Unsupported authentication method"); // todo proper error message
            }
            (_, None) => aws_sdk_s3::config::Builder::from(&sdk_config)
                .endpoint_url(config.endpoint_url)
                .region(aws_sdk_s3::config::Region::new(config.region))
                .force_path_style(config.force_path_style),
        };
        let aws_config = config_builder.build();
        aws_sdk_s3::Client::from_conf(aws_config)
    } else {
        // TODO: infer path style from endpoint URL or other means and set
        // .force_path_style(true) on builder if necessary
        let config = aws_sdk_s3::config::Builder::from(&sdk_config).build();
        aws_sdk_s3::Client::from_conf(config)
    }
}

impl S3Middleware {
    /// Create a new S3 middleware.
    pub fn new(config: Option<S3Config>) -> Self {
        Self {
            config,
            expiration: std::time::Duration::from_secs(300),
        }
    }

    /// Generate a presigned S3 `GetObject` request.
    async fn generate_presigned_s3_request(&self, url: Url) -> MiddlewareResult<PresignedRequest> {
        let client = create_s3_client(self.config.clone(), Some(url.clone())).await;

        let bucket_name = url.host_str().expect("Host should be present in S3 URL");
        let key = url.path().strip_prefix("/").ok_or_else(|| {
            reqwest_middleware::Error::middleware(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Missing prefix",
            ))
        })?;

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
            let presigned_request = self.generate_presigned_s3_request(url).await?;

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
    async fn test_presigned_s3_request_endpoint_url() {
        with_env(
            HashMap::from([
                ("AWS_ACCESS_KEY_ID", "minioadmin"),
                ("AWS_SECRET_ACCESS_KEY", "minioadmin"),
                ("AWS_REGION", "eu-central-1"),
                ("AWS_ENDPOINT_URL", "http://custom-aws"),
            ]),
            move || {
                Box::pin(async {
                    let middleware = S3Middleware::new(None);

                    let presigned = middleware
                        .generate_presigned_s3_request(
                            Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json")
                                .unwrap(),
                        )
                        .await
                        .unwrap();
                    assert!(
                        presigned.uri().to_string().starts_with(
                            "http://rattler-s3-testing.custom-aws/my-channel/noarch/repodata.json?"
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
                    let middleware = S3Middleware::new(None);

                    let presigned = middleware
                        .generate_presigned_s3_request(
                            Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json").unwrap()
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
                    let middleware = S3Middleware::new(None);

                    let presigned = middleware
                        .generate_presigned_s3_request(
                            Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json")
                                .unwrap(),
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

    #[rstest]
    #[tokio::test]
    #[serial]
    async fn test_presigned_s3_request_env_precedence(aws_config: (TempDir, std::path::PathBuf)) {
        with_env(
            HashMap::from([
                ("AWS_ENDPOINT_URL", "http://localhost:9000"),
                ("AWS_CONFIG_FILE", aws_config.1.to_str().unwrap()),
            ]),
            move || {
                Box::pin(async move {
                    let middleware = S3Middleware::new(None);

                    let presigned = middleware
                        .generate_presigned_s3_request(
                            Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json")
                                .unwrap(),
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
