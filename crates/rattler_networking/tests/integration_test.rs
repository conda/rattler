use std::{collections::HashMap, path::PathBuf, sync::Arc};

use rattler_networking::{
    s3_middleware::S3Config, AuthenticationMiddleware, AuthenticationStorage, S3Middleware,
};
use reqwest::Client;
use rstest::*;
use temp_env::async_with_vars;
use tempfile::{tempdir, TempDir};
use url::Url;

/* -------------------------------------- UTILS ------------------------------------- */

fn run_subprocess(cmd: &str, args: &[&str], env: &HashMap<&str, &str>) -> std::process::Output {
    let mut command = std::process::Command::new(cmd);
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    if !output.status.success() {
        eprintln!("Command failed: {:?}", command);
        eprintln!("Output: {:?}", String::from_utf8_lossy(&output.stdout));
        eprintln!("Error: {:?}", String::from_utf8_lossy(&output.stderr));
    }
    output
}

/* ------------------------------------ FIXTURES ------------------------------------ */

#[fixture]
#[once]
fn minio_host() -> String {
    format!(
        "http://localhost:{}",
        option_env!("MINIO_PORT").unwrap_or("9000")
    )
}

#[fixture]
#[once]
fn init_channel() {
    let host = format!(
        "http://minioadmin:minioadmin@localhost:{}",
        option_env!("MINIO_PORT").unwrap_or("9000")
    );
    let env = &HashMap::from([("MC_HOST_local", host.as_str())]);
    let mc_executable = if cfg!(windows) { "mc.bat" } else { "mc" };
    for bucket in &[
        "local/rattler-s3-testing",
        "local/rattler-s3-testing-public",
    ] {
        run_subprocess(mc_executable, &["mb", "--ignore-existing", bucket], env);
        run_subprocess(
            mc_executable,
            &[
                "cp",
                PathBuf::from("../../test-data/test-server/repo/noarch/repodata.json")
                    .to_str()
                    .unwrap(),
                format!("{bucket}/my-channel/noarch/repodata.json").as_str(),
            ],
            env,
        );
        run_subprocess(
            mc_executable,
            &[
                "cp",
                PathBuf::from("../../test-data/test-server/repo/noarch/test-package-0.1-0.tar.bz2")
                    .to_str()
                    .unwrap(),
                format!("{bucket}/my-channel/noarch/test-package-0.1-0.tar.bz2").as_str(),
            ],
            env,
        );
    }
    // Make bucket public
    run_subprocess(
        mc_executable,
        &[
            "anonymous",
            "set",
            "download",
            "local/rattler-s3-testing-public",
        ],
        env,
    );
}

#[fixture]
fn aws_config(minio_host: &str) -> (TempDir, std::path::PathBuf) {
    let temp_dir = tempdir().unwrap();
    let aws_config = format!(
        r#"
[profile default]
aws_access_key_id = minioadmin
aws_secret_access_key = minioadmin
endpoint_url = {minio_host}
region = eu-central-1

[profile public]
endpoint_url = {minio_host}
region = eu-central-1
"#
    );
    let aws_config_path = temp_dir.path().join("aws.config");
    std::fs::write(&aws_config_path, aws_config).unwrap();
    (temp_dir, aws_config_path)
}

/* -------------------------------------- TESTS ------------------------------------- */

#[rstest]
#[tokio::test]
async fn test_minio_download_repodata(
    minio_host: &str,
    #[allow(unused_variables)] init_channel: (),
) {
    let temp_dir = tempdir().unwrap();
    let aws_config = r#"
{
    "s3://rattler-s3-testing/my-channel": {
        "S3Credentials": {
            "access_key_id": "minioadmin",
            "secret_access_key": "minioadmin"
        }
    }
}"#;
    let credentials_path = temp_dir.path().join("credentials.json");
    std::fs::write(&credentials_path, aws_config).unwrap();
    let auth_storage = AuthenticationStorage::from_file(credentials_path.as_path()).unwrap();
    let middleware = S3Middleware::new(
        S3Config::Custom {
            endpoint_url: Url::parse(minio_host).unwrap(),
            region: "eu-central-1".into(),
            force_path_style: true,
        },
        auth_storage.clone(),
    );

    let download_client = Client::builder().no_gzip().build().unwrap();
    let download_client = reqwest_middleware::ClientBuilder::new(download_client)
        .with_arc(Arc::new(AuthenticationMiddleware::new(auth_storage)))
        .with(middleware)
        .build();

    let result = download_client
        .get("s3://rattler-s3-testing/my-channel/noarch/repodata.json")
        .send()
        .await
        .unwrap();

    assert_eq!(result.status(), 200);
    let body = result.text().await.unwrap();
    assert!(body.contains("test-package-0.1-0.tar.bz2"));
}

#[rstest]
#[tokio::test]
async fn test_minio_download_repodata_public(
    minio_host: &str,
    #[allow(unused_variables)] init_channel: (),
) {
    let auth_storage = AuthenticationStorage::new(); // empty storage
    let middleware = S3Middleware::new(
        S3Config::Custom {
            endpoint_url: Url::parse(minio_host).unwrap(),
            region: "eu-central-1".into(),
            force_path_style: true,
        },
        auth_storage.clone(),
    );

    let download_client = Client::builder().no_gzip().build().unwrap();
    let download_client = reqwest_middleware::ClientBuilder::new(download_client)
        .with_arc(Arc::new(AuthenticationMiddleware::new(auth_storage)))
        .with(middleware)
        .build();

    let result = download_client
        .get("s3://rattler-s3-testing-public/my-channel/noarch/repodata.json")
        .send()
        .await
        .unwrap();

    assert_eq!(result.status(), 200);
    let body = result.text().await.unwrap();
    assert!(body.contains("test-package-0.1-0.tar.bz2"));
}

#[rstest]
#[tokio::test]
async fn test_minio_download_repodata_aws_profile(
    aws_config: (TempDir, std::path::PathBuf),
    #[allow(unused_variables)] init_channel: (),
) {
    let auth_storage = AuthenticationStorage::new(); // empty storage
    let middleware = S3Middleware::new(S3Config::FromAWS, auth_storage.clone());

    let download_client = Client::builder().no_gzip().build().unwrap();
    let download_client = reqwest_middleware::ClientBuilder::new(download_client)
        .with_arc(Arc::new(AuthenticationMiddleware::new(auth_storage)))
        .with(middleware)
        .build();

    let result = async_with_vars(
        [
            ("AWS_CONFIG_FILE", Some(aws_config.1.to_str().unwrap())),
            ("AWS_PROFILE", Some("default")),
        ],
        async {
            download_client
                .get("s3://rattler-s3-testing/my-channel/noarch/repodata.json")
                .send()
                .await
                .unwrap()
        },
    )
    .await;
    assert_eq!(result.status(), 200);
    let body = result.text().await.unwrap();
    assert!(body.contains("test-package-0.1-0.tar.bz2"));
}

#[rstest]
#[tokio::test]
async fn test_minio_download_aws_profile_public(
    aws_config: (TempDir, std::path::PathBuf),
    #[allow(unused_variables)] init_channel: (),
) {
    let auth_storage = AuthenticationStorage::new(); // empty storage
    let middleware = S3Middleware::new(S3Config::FromAWS, auth_storage.clone());

    let download_client = Client::builder().no_gzip().build().unwrap();
    let download_client = reqwest_middleware::ClientBuilder::new(download_client)
        .with_arc(Arc::new(AuthenticationMiddleware::new(auth_storage)))
        .with(middleware)
        .build();
    let result = async_with_vars(
        [
            ("AWS_CONFIG_FILE", Some(aws_config.1.to_str().unwrap())),
            ("AWS_PROFILE", Some("public")),
        ],
        async {
            download_client
                .get("s3://rattler-s3-testing-public/my-channel/noarch/repodata.json")
                .send()
                .await
                .unwrap()
        },
    )
    .await;
    assert_eq!(result.status(), 200);
    let body = result.text().await.unwrap();
    assert!(body.contains("test-package-0.1-0.tar.bz2"));
}
