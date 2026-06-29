#[cfg(test)]
mod tests {
    use std::{path::PathBuf, process::Command, thread, time::Duration};

    use tempfile::tempdir;

    use crate::{MountBackend, MountSession, mount_environment};

    // These tests require test data at src/tests/data/pixi.lock and src/tests/data/cache.
    // Run them with: cargo test -- --ignored
    #[ignore = "requires test data at src/tests/data/"]
    #[compio::test]
    async fn test_prefix_replacement_file() -> anyhow::Result<()> {
        let mount_dir = tempdir()?;

        let session: Box<dyn MountSession> = mount_environment(
            PathBuf::from("src/tests/data/pixi.lock"),
            PathBuf::from("src/tests/data/cache"),
            mount_dir.path().to_path_buf(),
            MountBackend::Nfs,
            "default".to_string(),
            false,
        )
        .await?;

        thread::sleep(Duration::from_secs(1));

        let file = mount_dir
            .path()
            .join("lib")
            .join("pkgconfig")
            .join("foo.pc");

        let contents = std::fs::read_to_string(&file)?;

        assert!(
            !contents.contains("/opt/anaconda1anaconda2anaconda3"),
            "placeholder still present"
        );
        assert!(
            contents.contains(mount_dir.path().to_str().unwrap()),
            "mount prefix wasn't substituted"
        );

        session.unmount()?;
        Ok(())
    }

    #[ignore = "requires test data at src/tests/data/"]
    #[compio::test]
    async fn test_prefix_replacement_python() -> anyhow::Result<()> {
        let mount_dir = tempdir()?;

        let session: Box<dyn MountSession> = mount_environment(
            PathBuf::from("src/tests/data/pixi.lock"),
            PathBuf::from("src/tests/data/cache"),
            mount_dir.path().to_path_buf(),
            MountBackend::Nfs,
            "default".to_string(),
            false,
        )
        .await?;

        thread::sleep(Duration::from_secs(1));

        let output = Command::new(mount_dir.path().join("bin/python"))
            .arg("-c")
            .arg("import sys; print(sys.prefix)")
            .output()?;

        assert!(output.status.success(), "python failed to execute");

        let prefix = String::from_utf8(output.stdout)?;
        assert_eq!(prefix.trim(), mount_dir.path().to_str().unwrap());

        session.unmount()?;
        Ok(())
    }
}
