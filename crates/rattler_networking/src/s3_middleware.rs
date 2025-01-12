//! Middleware to handle `s3://` URLs to pull artifacts from an S3 bucket
use anyhow::Error;
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_s3::presigning::PresigningConfig;
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next, Result as MiddlewareResult};
use tracing::debug;
use url::Url;

use crate::{Authentication, AuthenticationStorage};

/// Configuration for the S3 middleware.
#[derive(Clone, Debug)]
pub enum S3Config {
    /// Use the default AWS configuration.
    FromAWS,
    /// Use a custom configuration.
    Custom {
        /// The endpoint URL to use for the S3 client.
        endpoint_url: Url,
        /// The region to use for the S3 client.
        region: String,
        /// Whether to force path style for the S3 client.
        force_path_style: bool,
    },
}

/// Wrapper around S3 client.
#[derive(Clone, Debug)]
pub struct S3 {
    auth_storage: AuthenticationStorage,
    config: S3Config,
    expiration: std::time::Duration,
}

/// S3 middleware to authenticate requests.
#[derive(Clone, Debug)]
pub struct S3Middleware {
    s3: S3,
}

impl S3Middleware {
    /// Create a new S3 middleware.
    pub fn new(config: S3Config, auth_storage: AuthenticationStorage) -> Self {
        debug!("Creating S3 middleware using {:?}", config);
        Self {
            s3: S3::new(config, auth_storage),
        }
    }
}

impl S3 {
    /// Create a new S3 client wrapper.
    pub fn new(config: S3Config, auth_storage: AuthenticationStorage) -> Self {
        Self {
            config,
            auth_storage,
            expiration: std::time::Duration::from_secs(300),
        }
    }

    /// Create an S3 client.
    ///
    /// # Arguments
    ///
    /// * `url` - The S3 URL to obtain authentication information from the authentication storage.
    ///     Only respected for custom (non-AWS-based) configuration.
    pub async fn create_s3_client(&self, url: Option<Url>) -> Result<aws_sdk_s3::Client, Error> {
        if let (
            S3Config::Custom {
                endpoint_url,
                region,
                force_path_style,
            },
            Some(url),
        ) = (self.config.clone(), url)
        {
            let auth = self.auth_storage.get_by_url(url).unwrap(); // todo
            let config_builder = match auth {
                (
                    _,
                    Some(Authentication::S3Credentials {
                        access_key_id,
                        secret_access_key,
                        session_token,
                    }),
                ) => {
                    let sdk_config = aws_config::defaults(BehaviorVersion::latest()).load().await;
                    aws_sdk_s3::config::Builder::from(&sdk_config)
                        .endpoint_url(endpoint_url)
                        .region(aws_sdk_s3::config::Region::new(region))
                        .force_path_style(force_path_style)
                        .credentials_provider(aws_sdk_s3::config::Credentials::new(
                            access_key_id,
                            secret_access_key,
                            session_token,
                            None,
                            "pixi",
                        ))
                }
                (_, Some(_)) => {
                    return Err(anyhow::anyhow!("unsupported authentication method"));
                }
                (_, None) => {
                    tracing::info!("No authentication found, assuming bucket is public");
                    let sdk_config = aws_config::defaults(BehaviorVersion::latest())
                        .no_credentials() // Turn off request signing
                        .load()
                        .await;
                    aws_sdk_s3::config::Builder::from(&sdk_config)
                        .endpoint_url(endpoint_url)
                        .region(aws_sdk_s3::config::Region::new(region))
                        .force_path_style(force_path_style)
                }
            };
            let s3_config = config_builder.build();
            Ok(aws_sdk_s3::Client::from_conf(s3_config))
        } else {
            let mut sdk_config = aws_config::defaults(BehaviorVersion::latest()).load().await;
            if let Some(credentials_provider) = sdk_config.credentials_provider() {
                let creds = credentials_provider.as_ref().provide_credentials().await;
                if creds.is_ok() {
                    tracing::info!("Using AWS credentials from environment via default provider");
                } else {
                    tracing::warn!("No AWS credentials found, assuming bucket is public");
                    sdk_config = aws_config::defaults(BehaviorVersion::latest())
                        .no_credentials()
                        .load()
                        .await;
                }
            }
            let mut s3_config_builder = aws_sdk_s3::config::Builder::from(&sdk_config);
            // Infer if we expect path-style addressing from the endpoint URL.
            if let Some(endpoint_url) = sdk_config.endpoint_url() {
                // If the endpoint URL is localhost, we probably have to use path-style addressing.
                // There are certainly more edge cases, but this is a valid start to make the
                // integration tests with minIO work.
                // xref: https://github.com/awslabs/aws-sdk-rust/issues/1230
                if endpoint_url.starts_with("http://localhost") {
                    s3_config_builder = s3_config_builder.force_path_style(true);
                }
            }
            let client = aws_sdk_s3::Client::from_conf(s3_config_builder.build());
            Ok(client)
        }
    }

    /// Generate a presigned S3 `GetObject` request.
    async fn generate_presigned_s3_url(&self, url: Url) -> MiddlewareResult<Url> {
        let client = self.create_s3_client(Some(url.clone())).await?;

        let bucket_name = url.host_str().ok_or_else(|| {
            reqwest_middleware::Error::middleware(std::io::Error::new(
                std::io::ErrorKind::Other,
                "host should be present in S3 URL",
            ))
        })?;
        let key = url.path().strip_prefix("/").unwrap();

        let builder = client.get_object().bucket(bucket_name).key(key);

        Url::parse(
            builder
                .presigned(
                    PresigningConfig::expires_in(self.expiration)
                        .map_err(reqwest_middleware::Error::middleware)?,
                )
                .await
                .map_err(reqwest_middleware::Error::middleware)?
                .uri(),
        )
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
            let presigned_url = self.s3.generate_presigned_s3_url(url).await?;
            *req.url_mut() = presigned_url.clone();
        }
        next.run(req, extensions).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::{fixture, rstest};
    use temp_env::async_with_vars;
    use tempfile::{tempdir, TempDir};

    #[tokio::test]
    async fn test_presigned_s3_request_endpoint_url() {
        let s3 = S3::new(S3Config::FromAWS, AuthenticationStorage::default());
        let presigned = async_with_vars(
            [
                ("AWS_ACCESS_KEY_ID", Some("minioadmin")),
                ("AWS_SECRET_ACCESS_KEY", Some("minioadmin")),
                ("AWS_REGION", Some("eu-central-1")),
                ("AWS_ENDPOINT_URL", Some("http://custom-aws")),
            ],
            async {
                s3.generate_presigned_s3_url(
                    Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json").unwrap(),
                )
                .await
                .unwrap()
            },
        )
        .await;
        assert!(
            presigned.to_string().starts_with(
                "http://rattler-s3-testing.custom-aws/my-channel/noarch/repodata.json?"
            ),
            "Unexpected presigned URL: {presigned:?}"
        );
    }

    #[tokio::test]
    async fn test_presigned_s3_request_aws() {
        let s3 = S3::new(S3Config::FromAWS, AuthenticationStorage::default());
        let presigned = async_with_vars(
            [
                ("AWS_ACCESS_KEY_ID", Some("minioadmin")),
                ("AWS_SECRET_ACCESS_KEY", Some("minioadmin")),
                ("AWS_REGION", Some("eu-central-1")),
            ],
            async {
                s3.generate_presigned_s3_url(
                    Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json").unwrap(),
                )
                .await
                .unwrap()
            },
        )
        .await;
        assert!(presigned.to_string().starts_with("https://rattler-s3-testing.s3.eu-central-1.amazonaws.com/my-channel/noarch/repodata.json?"), "Unexpected presigned URL: {presigned:?}"
        );
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
endpoint_url = http://localhost:9000
region = eu-central-1

[profile public]
endpoint_url = http://localhost:9000
region = eu-central-1
"#;
        let aws_config_path = temp_dir.path().join("aws.config");
        std::fs::write(&aws_config_path, aws_config).unwrap();
        (temp_dir, aws_config_path)
    }

    #[rstest]
    #[tokio::test]
    async fn test_presigned_s3_request_custom_config_from_env(
        aws_config: (TempDir, std::path::PathBuf),
    ) {
        let s3 = S3::new(S3Config::FromAWS, AuthenticationStorage::default());
        let presigned = async_with_vars(
            [
                ("AWS_CONFIG_FILE", Some(aws_config.1.to_str().unwrap())),
                ("AWS_PROFILE", Some("packages")),
            ],
            async {
                s3.generate_presigned_s3_url(
                    Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json").unwrap(),
                )
                .await
                .unwrap()
            },
        )
        .await;
        assert!(
            presigned.to_string().contains("localhost:9000"),
            "Unexpected presigned URL: {presigned:?}"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn test_presigned_s3_request_env_precedence(aws_config: (TempDir, std::path::PathBuf)) {
        let s3 = S3::new(S3Config::FromAWS, AuthenticationStorage::default());
        let presigned = async_with_vars(
            [
                ("AWS_ENDPOINT_URL", Some("http://localhost:9000")),
                ("AWS_CONFIG_FILE", Some(aws_config.1.to_str().unwrap())),
            ],
            async {
                s3.generate_presigned_s3_url(
                    Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json").unwrap(),
                )
                .await
                .unwrap()
            },
        )
        .await;
        assert!(
            presigned.to_string().contains("localhost:9000"),
            "Unexpected presigned URL: {presigned:?}"
        );
    }

    #[tokio::test]
    async fn test_presigned_s3_request_custom_config() {
        let temp_dir = tempdir().unwrap();
        let aws_config = r#"
        {
            "s3://rattler-s3-testing/my-channel": {
                "S3Credentials": {
                    "access_key_id": "minioadmin",
                    "secret_access_key": "minioadmin"
                }
            }
        }
        "#;
        let credentials_path = temp_dir.path().join("credentials.json");
        std::fs::write(&credentials_path, aws_config).unwrap();
        let s3 = S3::new(
            S3Config::Custom {
                endpoint_url: Url::parse("http://localhost:9000").unwrap(),
                region: "eu-central-1".into(),
                force_path_style: true,
            },
            AuthenticationStorage::from_file(credentials_path.as_path()).unwrap(),
        );

        let presigned = s3
            .generate_presigned_s3_url(
                Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json").unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            presigned.path(),
            "/rattler-s3-testing/my-channel/noarch/repodata.json"
        );
        assert_eq!(presigned.scheme(), "http");
        assert_eq!(presigned.host_str().unwrap(), "localhost");
        assert!(presigned.query().unwrap().contains("X-Amz-Credential"));
    }

    #[tokio::test]
    async fn test_presigned_s3_request_public_bucket() {
        let s3 = S3::new(
            S3Config::Custom {
                endpoint_url: Url::parse("http://localhost:9000").unwrap(),
                region: "eu-central-1".into(),
                force_path_style: true,
            },
            AuthenticationStorage::new(), // empty auth storage
        );

        let presigned = s3
            .generate_presigned_s3_url(
                Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json").unwrap(),
            )
            .await
            .unwrap();
        assert!(
            presigned.to_string()
                == "http://localhost:9000/rattler-s3-testing/my-channel/noarch/repodata.json?x-id=GetObject",
            "Unexpected presigned URL: {presigned:?}"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn test_presigned_s3_request_public_bucket_aws(
        aws_config: (TempDir, std::path::PathBuf),
    ) {
        let s3 = S3::new(S3Config::FromAWS, AuthenticationStorage::new());
        let presigned = async_with_vars(
            [
                ("AWS_CONFIG_FILE", Some(aws_config.1.to_str().unwrap())),
                ("AWS_PROFILE", Some("public")),
            ],
            async {
                s3.generate_presigned_s3_url(
                    Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json").unwrap(),
                )
                .await
                .unwrap()
            },
        )
        .await;
        assert!(
        presigned.to_string()
            == "http://localhost:9000/rattler-s3-testing/my-channel/noarch/repodata.json?x-id=GetObject",
        "Unexpected presigned URL: {presigned:?}"
        );
    }
}
