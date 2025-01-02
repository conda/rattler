use std::collections::HashMap;

use rattler_networking::S3Middleware;
use rstest::*;
use serial_test::serial;

struct MinioServer {
    handle: std::process::Child,
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
            .args(&args)
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

fn run_subprocess(cmd: &str, args: &[&str], env: &HashMap<&str, &str>) -> std::process::Output {
    let mut command = std::process::Command::new(cmd);
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().expect("Failed to run command");
    if !output.status.success() {
        eprintln!("Command failed: {:?}", output);
    }
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
            "../../test-data/test-server/repo/noarch/repodata.json",
            "local/rattler-s3-testing/my-channel/noarch/repodata.json",
        ],
        env,
    );
    run_subprocess(
        "mc",
        &[
            "cp",
            "../../test-data/test-server/repo/noarch/test-package-0.1-0.tar.bz2",
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

#[rstest]
#[tokio::test]
#[serial]
async fn test_presigned_s3_request() {
    std::env::set_var("AWS_ACCESS_KEY_ID", "minioadmin");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "minioadmin");
    std::env::set_var("AWS_REGION", "eu-central-1");
    std::env::set_var("AWS_ENDPOINT_URL", "http://localhost:9000");

    let middleware = S3Middleware::new(None, None, Some(true)).await;

    // TODO: Do install or search
}
