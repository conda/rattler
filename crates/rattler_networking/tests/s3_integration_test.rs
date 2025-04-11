#![cfg(feature = "s3")]

use std::{collections::HashMap, sync::Arc};

use rattler_networking::{
    authentication_storage::backends::file::FileStorage, s3_middleware::S3Config,
    AuthenticationMiddleware, AuthenticationStorage, S3Middleware,
};

use reqwest::Client;
use rstest::*;
use temp_env::async_with_vars;
use tempfile::{tempdir, TempDir};
use url::Url;

/* ------------------------------------ FIXTURES ------------------------------------ */

#[fixture]
fn r2_host() -> String {
    "https://e1a7cde76f1780ec06bac859036dbaf7.eu.r2.cloudflarestorage.com".into()
}

#[fixture]
fn r2_credentials() -> Option<(String, String)> {
    let r2_access_key_id = std::env::var("RATTLER_TEST_R2_READONLY_ACCESS_KEY_ID").ok();
    let r2_secret_access_key = std::env::var("RATTLER_TEST_R2_READONLY_SECRET_ACCESS_KEY").ok();
    if r2_access_key_id.is_none()
        || r2_access_key_id.clone().unwrap().is_empty()
        || r2_secret_access_key.is_none()
        || r2_secret_access_key.clone().unwrap().is_empty()
    {
        eprintln!(
            "Skipping test as RATTLER_TEST_R2_READONLY_ACCESS_KEY_ID or RATTLER_TEST_R2_READONLY_SECRET_ACCESS_KEY is not set"
        );
        None
    } else {
        Some((r2_access_key_id.unwrap(), r2_secret_access_key.unwrap()))
    }
}

#[fixture]
fn aws_config(
    r2_host: String,
    r2_credentials: Option<(String, String)>,
) -> Option<(TempDir, std::path::PathBuf)> {
    let r2_credentials = r2_credentials?;
    let temp_dir = tempdir().unwrap();
    let aws_config = format!(
        r#"
[profile default]
aws_access_key_id = {}
aws_secret_access_key = {}
endpoint_url = {r2_host}
region = auto

[profile public]
endpoint_url = {r2_host}
region = auto
"#,
        r2_credentials.0,
        r2_credentials.1,
        r2_host = r2_host
    );
    let aws_config_path = temp_dir.path().join("aws.config");
    std::fs::write(&aws_config_path, aws_config).unwrap();
    Some((temp_dir, aws_config_path))
}

/* -------------------------------------- TESTS ------------------------------------- */

#[rstest]
#[tokio::test]
async fn test_r2_download_repodata(r2_host: String, r2_credentials: Option<(String, String)>) {
    if r2_credentials.clone().is_none() {
        return;
    }
    let r2_credentials = r2_credentials.clone().unwrap();
    let temp_dir = tempdir().unwrap();
    let credentials = format!(
        r#"
{{
    "s3://rattler-s3-testing/channel": {{
        "S3Credentials": {{
            "access_key_id": "{}",
            "secret_access_key": "{}"
        }}
    }}
}}"#,
        r2_credentials.0, r2_credentials.1
    );
    let credentials_path = temp_dir.path().join("credentials.json");
    std::fs::write(&credentials_path, credentials).unwrap();
    let mut auth_storage = AuthenticationStorage::empty();
    auth_storage.add_backend(Arc::from(FileStorage::from_path(credentials_path).unwrap()));
    let middleware = S3Middleware::new(
        HashMap::from([(
            "rattler-s3-testing".into(),
            S3Config::Custom {
                endpoint_url: Url::parse(&r2_host).unwrap(),
                region: "auto".into(),
                force_path_style: true,
            },
        )]),
        auth_storage.clone(),
    );

    let download_client = Client::builder().no_gzip().build().unwrap();
    let download_client = reqwest_middleware::ClientBuilder::new(download_client)
        .with_arc(Arc::new(AuthenticationMiddleware::from_auth_storage(
            auth_storage,
        )))
        .with(middleware)
        .build();

    let result = download_client
        .get("s3://rattler-s3-testing/channel/noarch/repodata.json")
        .send()
        .await
        .unwrap();

    assert_eq!(result.status(), 200);
    let body = result.text().await.unwrap();
    assert!(
        body.contains("my-webserver-0.1.0-pyh4616a5c_0.conda"),
        "body does not contain package: {body}"
    );
}

#[rstest]
#[tokio::test]
async fn test_r2_download_repodata_aws_profile(aws_config: Option<(TempDir, std::path::PathBuf)>) {
    if aws_config.is_none() {
        return;
    }
    let aws_config = aws_config.unwrap();
    let middleware = S3Middleware::new(HashMap::new(), AuthenticationStorage::empty());

    let download_client = Client::builder().no_gzip().build().unwrap();
    let download_client = reqwest_middleware::ClientBuilder::new(download_client)
        .with_arc(Arc::new(AuthenticationMiddleware::from_auth_storage(
            AuthenticationStorage::empty(),
        )))
        .with(middleware)
        .build();

    let result = async_with_vars(
        [
            ("AWS_CONFIG_FILE", Some(aws_config.1.to_str().unwrap())),
            ("AWS_PROFILE", Some("default")),
        ],
        async {
            download_client
                .get("s3://rattler-s3-testing/channel/noarch/repodata.json")
                .send()
                .await
                .unwrap()
        },
    )
    .await;
    assert_eq!(result.status(), 200);
    let body = result.text().await.unwrap();
    assert!(
        body.contains("my-webserver-0.1.0-pyh4616a5c_0.conda"),
        "body does not contain package: {body}"
    );
}
