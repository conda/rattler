//! Middleware to handle `s3://` URLs to pull artifacts from an S3 bucket
use core::panic;

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

/// S3 middleware to authenticate requests.
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
        Self {
            s3: S3 {
                config,
                auth_storage,
                expiration: std::time::Duration::from_secs(300),
            },
        }
    }
}

impl S3 {
    /// Create a new S3 middleware.
    pub fn new(config: S3Config, auth_storage: AuthenticationStorage) -> Self {
        Self {
            config,
            auth_storage,
            expiration: std::time::Duration::from_secs(300),
        }
    }

    /// Create an S3 client for given channel with provided configuration.
    pub async fn create_s3_client(&self, url: Option<Url>) -> aws_sdk_s3::Client {
        let sdk_config = aws_config::defaults(BehaviorVersion::latest()).load().await;
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
                ) => aws_sdk_s3::config::Builder::from(&sdk_config)
                    .endpoint_url(endpoint_url)
                    .region(aws_sdk_s3::config::Region::new(region))
                    .force_path_style(force_path_style)
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
                (_, None) => todo!("should use no credentials provider and not sign"),
                // (_, None) => aws_sdk_s3::config::Builder::from(&sdk_config)
                //     .endpoint_url(endpoint_url)
                //     .region(aws_sdk_s3::config::Region::new(region))
                //     .force_path_style(force_path_style),
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

    /// Generate a presigned S3 `GetObject` request.
    async fn generate_presigned_s3_url(&self, url: Url) -> MiddlewareResult<Url> {
        let client = self.create_s3_client(Some(url.clone())).await;

        let bucket_name = url.host_str().expect("Host should be present in S3 URL");
        let key = url.path().strip_prefix("/").ok_or_else(|| {
            reqwest_middleware::Error::middleware(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Missing prefix",
            ))
        })?;

        let builder = client.get_object().bucket(bucket_name).key(key);
        // if client has no credentials provider, don't presign but use default url
        // TODO: implement this

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

            debug!("Presigned S3 url: {:?}", presigned_url);
            *req.url_mut() = presigned_url.clone();
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

    // TODO: test no auth

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
                    let s3 = S3::new(S3Config::FromAWS, AuthenticationStorage::default());

                    let presigned = s3
                        .generate_presigned_s3_url(
                            Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json")
                                .unwrap(),
                        )
                        .await
                        .unwrap();
                    assert!(
                        presigned.to_string().starts_with(
                            "http://rattler-s3-testing.custom-aws/my-channel/noarch/repodata.json?"
                        ),
                        "Unexpected presigned URL: {:?}",
                        presigned
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
                    let s3 = S3::new(S3Config::FromAWS, AuthenticationStorage::default());

                    let presigned = s3
                        .generate_presigned_s3_url(
                            Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json").unwrap()
                        )
                        .await
                        .unwrap();
                    assert!(
                        presigned.to_string().starts_with(
                            "https://rattler-s3-testing.s3.eu-central-1.amazonaws.com/my-channel/noarch/repodata.json?"
                        ),
                        "Unexpected presigned URL: {:?}",
                        presigned
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
                    let s3 = S3::new(S3Config::FromAWS, AuthenticationStorage::default());

                    let presigned = s3
                        .generate_presigned_s3_url(
                            Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json")
                                .unwrap(),
                        )
                        .await
                        .unwrap();
                    assert!(
                        presigned.to_string().contains("localhost:8000"),
                        "Unexpected presigned URL: {:?}",
                        presigned
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
                    let s3 = S3::new(S3Config::FromAWS, AuthenticationStorage::default());

                    let presigned = s3
                        .generate_presigned_s3_url(
                            Url::parse("s3://rattler-s3-testing/my-channel/noarch/repodata.json")
                                .unwrap(),
                        )
                        .await
                        .unwrap();
                    assert!(
                        presigned.to_string().contains("localhost:9000"),
                        "Unexpected presigned URL: {:?}",
                        presigned
                    );
                })
            },
        )
        .await;
    }
}
