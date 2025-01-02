use std::{collections::HashMap, path::PathBuf, sync::Arc};

use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage, S3Middleware};
use reqwest::Client;
use rstest::*;
use serial_test::serial;
use tempfile::{tempdir, TempDir};

/* ----------------------------------- MINIO UTILS ---------------------------------- */

struct MinioServer {
    handle: std::process::Child,
    #[allow(dead_code)]
    directory: tempfile::TempDir,
}

impl MinioServer {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("Failed to create temp directory");
        let args = [
            "server",
            directory.path().to_str().unwrap(),
            "--address",
            "127.0.0.1:9000",
        ];
        let handle = std::process::Command::new("minio")
            .args(args)
            .spawn()
            .expect("Failed to start Minio server");
        eprintln!(
            "Starting Minio server with args (PID={}): {:?}",
            handle.id(),
            args
        );
        MinioServer { handle, directory }
    }
}

impl Drop for MinioServer {
    fn drop(&mut self) {
        eprintln!("Shutting down Minio server (PID={})", self.handle.id());
        self.handle.kill().expect("Failed to kill Minio server");
    }
}

fn run_subprocess(cmd: &str, args: &[&str], env: &HashMap<&str, &str>) -> std::process::Output {
    let mut command = std::process::Command::new(cmd);
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().expect("Failed to run command");
    assert!(output.status.success());
    output
}

fn init_channel() {
    let env = &HashMap::from([(
        "MC_HOST_local",
        "http://minioadmin:minioadmin@localhost:9000",
    )]);
    run_subprocess("mc", &["mb", "local/rattler-s3-testing"], env);
    run_subprocess(
        "mc",
        &[
            "cp",
            PathBuf::from("../../test-data/test-server/repo/noarch/repodata.json")
                .to_str()
                .unwrap(),
            "local/rattler-s3-testing/my-channel/noarch/repodata.json",
        ],
        env,
    );
    run_subprocess(
        "mc",
        &[
            "cp",
            PathBuf::from("../../test-data/test-server/repo/noarch/test-package-0.1-0.tar.bz2")
                .to_str()
                .unwrap(),
            "local/rattler-s3-testing/my-channel/noarch/test-package-0.1-0.tar.bz2",
        ],
        env,
    );
}

#[fixture]
fn minio_server() -> MinioServer {
    let server = MinioServer::new();
    init_channel();
    server
}

#[fixture]
fn aws_config() -> (TempDir, std::path::PathBuf) {
    let temp_dir = tempdir().unwrap();
    let aws_config = r#"
[profile default]
aws_access_key_id = minioadmin
aws_secret_access_key = minioadmin
endpoint_url = http://localhost:9000
region = eu-central-1
"#;
    let aws_config_path = temp_dir.path().join("aws.config");
    std::fs::write(&aws_config_path, aws_config).unwrap();
    (temp_dir, aws_config_path)
}

#[rstest]
#[tokio::test]
#[serial]
async fn test_minio_download_repodata(
    #[allow(unused_variables)] minio_server: MinioServer,
    aws_config: (TempDir, std::path::PathBuf),
) {
    let middleware = S3Middleware::new(Some(aws_config.1), Some("default".into()), Some(true));

    let download_client = Client::builder()
        .no_gzip()
        .build()
        .expect("failed to create client");

    let authentication_storage = AuthenticationStorage::default();
    let download_client = reqwest_middleware::ClientBuilder::new(download_client)
        .with_arc(Arc::new(AuthenticationMiddleware::new(
            authentication_storage,
        )))
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