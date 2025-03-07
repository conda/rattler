//! Unlinking packages from an environment.

use std::{
    collections::HashSet,
    ffi::OsString,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use fs_err::tokio as tokio_fs;
use rattler_conda_types::PrefixRecord;
use uuid::Uuid;

/// Error that can occur while unlinking a package.
#[derive(Debug, thiserror::Error)]
pub enum UnlinkError {
    /// Failed to delete a directory.
    #[error("failed to delete empty directory: {0}")]
    FailedToDeleteDirectory(String, std::io::Error),

    /// Failed to delete a file.
    #[error("failed to delete file: {0}")]
    FailedToDeleteFile(String, std::io::Error),

    /// Failed to read a directory.
    #[error("failed to read directory: {0}")]
    FailedToReadDirectory(String, std::io::Error),

    /// Failed to read a directory.
    #[error("failed to test existence: {0}")]
    FailedToTestExistence(String, std::io::Error),

    /// Failed to create a directory
    #[error("failed to create directory: {0}")]
    FailedToCreateDirectory(String, std::io::Error),

    /// Failed to move a file to the trash
    #[error("failed to move file: {0} to {1}")]
    FailedToMoveFile(String, String, std::io::Error),
}

pub(crate) fn recursively_remove_empty_directories(
    directory_path: &Path,
    target_prefix: &Path,
    is_python_noarch: bool,
    keep_directories: &HashSet<PathBuf>,
) -> Result<PathBuf, UnlinkError> {
    // Never delete the target prefix
    if directory_path == target_prefix
        || keep_directories.contains(directory_path)
        || !directory_path.exists()
    {
        return Ok(directory_path.to_path_buf());
    }

    // Should we make this stronger to protect the user?
    assert!(directory_path.starts_with(target_prefix));

    let mut read_dir = directory_path.read_dir().map_err(|e| {
        UnlinkError::FailedToReadDirectory(directory_path.to_string_lossy().to_string(), e)
    })?;

    match read_dir.next().transpose() {
        Ok(None) => {
            // The directory is empty, delete it
            std::fs::remove_dir(directory_path).map_err(|e| {
                UnlinkError::FailedToDeleteDirectory(
                    directory_path.to_string_lossy().to_string(),
                    e,
                )
            })?;

            // Recursively remove the parent directory
            if let Some(parent) = directory_path.parent() {
                recursively_remove_empty_directories(
                    parent,
                    target_prefix,
                    is_python_noarch,
                    keep_directories,
                )
            } else {
                Ok(directory_path.into())
            }
        }

        // Check if the only entry is a `__pycache__` directory
        Ok(Some(entry))
            if is_python_noarch
                && entry.file_name() == "__pycache__"
                && read_dir.next().is_none() =>
        {
            // The directory is empty, delete it
            std::fs::remove_dir_all(directory_path).map_err(|e| {
                UnlinkError::FailedToDeleteDirectory(
                    directory_path.to_string_lossy().to_string(),
                    e,
                )
            })?;

            // Recursively remove the parent directory
            if let Some(parent) = directory_path.parent() {
                recursively_remove_empty_directories(
                    parent,
                    target_prefix,
                    is_python_noarch,
                    keep_directories,
                )
            } else {
                Ok(directory_path.into())
            }
        }
        _ => Ok(directory_path.into()),
    }
}

/// Remove files in trash folder that are not currently in use.
pub async fn empty_trash(target_prefix: &Path) -> Result<(), UnlinkError> {
    let trash_dir = target_prefix.join(".trash");
    match tokio_fs::read_dir(&trash_dir).await {
        Ok(mut read_dir) => {
            let mut files_left_in_trash = false;
            while let Some(entry) = read_dir.next_entry().await.map_err(|e| {
                UnlinkError::FailedToReadDirectory(trash_dir.to_string_lossy().to_string(), e)
            })? {
                tokio_fs::remove_file(entry.path())
                    .await
                    .or_else(|e| match e.kind() {
                        ErrorKind::NotFound => Ok(()),
                        ErrorKind::PermissionDenied => {
                            files_left_in_trash = true;
                            Ok(())
                        }
                        _ => Err(UnlinkError::FailedToDeleteFile(
                            entry.path().to_string_lossy().to_string(),
                            e,
                        )),
                    })?;
            }
            if !files_left_in_trash {
                tokio_fs::remove_dir(&trash_dir).await.map_err(|e| {
                    UnlinkError::FailedToDeleteDirectory(trash_dir.to_string_lossy().to_string(), e)
                })?;
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            return Err(UnlinkError::FailedToReadDirectory(
                trash_dir.to_string_lossy().to_string(),
                e,
            ))
        }
    }

    Ok(())
}

async fn move_to_trash(target_prefix: &Path, path: &Path) -> Result<(), UnlinkError> {
    let mut trash_dest = target_prefix.join(".trash");
    match tokio::fs::try_exists(&trash_dest).await {
        Ok(true) => {}
        Ok(false) => tokio_fs::create_dir(&trash_dest).await.map_err(|e| {
            UnlinkError::FailedToCreateDirectory(trash_dest.to_string_lossy().to_string(), e)
        })?,
        Err(e) => {
            return Err(UnlinkError::FailedToTestExistence(
                trash_dest.to_string_lossy().to_string(),
                e,
            ))
        }
    }
    let mut new_filename = OsString::new();
    if let Some(file_name) = path.file_name() {
        new_filename.push(file_name);
        new_filename.push(".");
    }
    new_filename.push(format!("{}.trash", Uuid::new_v4().simple()));
    trash_dest.push(new_filename);
    match tokio_fs::rename(path, &trash_dest).await {
        Ok(_) => Ok(()),
        Err(e) => Err(UnlinkError::FailedToMoveFile(
            path.to_string_lossy().to_string(),
            trash_dest.to_string_lossy().to_string(),
            e,
        )),
    }
}

/// Completely remove the specified package from the environment.
pub async fn unlink_package(
    target_prefix: &Path,
    prefix_record: &PrefixRecord,
) -> Result<(), UnlinkError> {
    // Remove all entries
    for paths in prefix_record.paths_data.paths.iter() {
        let p = target_prefix.join(&paths.relative_path);
        match tokio_fs::remove_file(&p).await {
            Ok(_) => {}
            Err(e) => match e.kind() {
                // Simply ignore if the file is already gone.
                ErrorKind::NotFound => {}
                ErrorKind::PermissionDenied => move_to_trash(target_prefix, &p).await?,
                _ => {
                    return Err(UnlinkError::FailedToDeleteFile(
                        paths.relative_path.to_string_lossy().to_string(),
                        e,
                    ))
                }
            },
        }
    }

    // Remove the conda-meta file
    let conda_meta_path = target_prefix
        .join("conda-meta")
        .join(prefix_record.file_name());

    tokio_fs::remove_file(&conda_meta_path).await.map_err(|e| {
        UnlinkError::FailedToDeleteFile(conda_meta_path.to_string_lossy().to_string(), e)
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        io::Write,
        path::Path,
    };

    use rattler_conda_types::{Platform, RepoDataRecord};

    use crate::install::test_utils::download_and_get_prefix_record;
    use crate::install::{empty_trash, unlink_package, InstallDriver, Transaction};

    #[tokio::test]
    async fn test_unlink_package() {
        let environment_dir = tempfile::TempDir::new().unwrap();
        let prefix_record = download_and_get_prefix_record(
            environment_dir.path(),
            "https://conda.anaconda.org/conda-forge/win-64/ruff-0.0.171-py310h298983d_0.conda"
                .parse()
                .unwrap(),
            "25c755b97189ee066576b4ae3999d5e7ff4406d236b984742194e63941838dcd",
        )
        .await;
        let conda_meta_path = environment_dir.path().join("conda-meta");
        std::fs::create_dir_all(&conda_meta_path).unwrap();

        // Write the conda-meta information
        let pkg_meta_path = conda_meta_path.join(prefix_record.file_name());
        prefix_record.write_to_path(&pkg_meta_path, true).unwrap();

        // Unlink the package
        unlink_package(environment_dir.path(), &prefix_record)
            .await
            .unwrap();

        // Check if the conda-meta file is gone
        assert!(!pkg_meta_path.exists());

        // Set up install driver to run post-processing steps ...
        let install_driver = InstallDriver::default();

        let transaction = Transaction::from_current_and_desired(
            vec![prefix_record.clone()],
            Vec::<RepoDataRecord>::new().into_iter(),
            None,
            Platform::current(),
        )
        .unwrap();

        install_driver
            .remove_empty_directories(&transaction, &[], environment_dir.path())
            .unwrap();

        // check that the environment is completely empty except for the conda-meta
        // folder
        let entries = std::fs::read_dir(environment_dir.path())
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].as_ref().unwrap().file_name(), "conda-meta");
    }

    #[tokio::test]
    async fn test_unlink_package_python_noarch() {
        let target_prefix = tempfile::TempDir::new().unwrap();
        let prefix_record = download_and_get_prefix_record(
            target_prefix.path(),
            "https://conda.anaconda.org/conda-forge/noarch/pytweening-1.0.4-pyhd8ed1ab_0.tar.bz2"
                .parse()
                .unwrap(),
            "81644bcb60d295f7923770b41daf2d90152ef54b9b094c26513be50fccd62125",
        )
        .await;

        let conda_meta_path = target_prefix.path().join("conda-meta");
        std::fs::create_dir_all(&conda_meta_path).unwrap();

        // Write the conda-meta information
        let pkg_meta_path = conda_meta_path.join(prefix_record.file_name());
        prefix_record.write_to_path(&pkg_meta_path, true).unwrap();

        fs::create_dir(
            target_prefix
                .path()
                .join("lib/python3.10/site-packages/pytweening/__pycache__"),
        )
        .unwrap();
        let mut file =
            File::create(target_prefix.path().join(
                "lib/python3.10/site-packages/pytweening/__pycache__/__init__.cpython-310.pyc",
            ))
            .unwrap();
        file.write_all(b"some funny bytes").unwrap();
        file.sync_all().unwrap();

        // Unlink the package
        unlink_package(target_prefix.path(), &prefix_record)
            .await
            .unwrap();

        // Check if the conda-meta file is gone
        assert!(!pkg_meta_path.exists());
        let install_driver = InstallDriver::default();

        let transaction = Transaction::from_current_and_desired(
            vec![prefix_record.clone()],
            Vec::<RepoDataRecord>::new().into_iter(),
            None,
            Platform::current(),
        )
        .unwrap();

        install_driver
            .remove_empty_directories(&transaction, &[], target_prefix.path())
            .unwrap();

        // check that the environment is completely empty except for the conda-meta
        // folder
        let entries = std::fs::read_dir(target_prefix.path())
            .unwrap()
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].as_ref().unwrap().file_name(), "conda-meta");
    }

    fn count_trash(trash_dir: &Path) -> usize {
        if !trash_dir.exists() {
            return 0;
        }
        let mut count = 0;
        for entry in std::fs::read_dir(trash_dir).unwrap() {
            let entry = entry.unwrap();
            if entry.path().extension().unwrap() == "trash" {
                count += 1;
            }
        }
        count
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_unlink_package_in_use() {
        use crate::get_repodata_record;
        use crate::install::link_package;
        use crate::install::InstallOptions;
        use rattler_conda_types::PrefixRecord;
        use std::{
            env::{join_paths, split_paths, var_os},
            io::{BufRead, BufReader},
            process::{Command, Stdio},
        };

        use itertools::chain;

        let environment_dir = tempfile::TempDir::new().unwrap();
        let target_prefix = environment_dir.path();
        let trash_dir = target_prefix.join(".trash");
        let files = [
            ("https://conda.anaconda.org/conda-forge/win-64/bat-0.24.0-ha073cba_1.conda", "65a125b7a6e7fd7e5d4588ee537b5db2c984ed71e4832f7041f691c2cfd73504"),
            ("https://conda.anaconda.org/conda-forge/win-64/ucrt-10.0.22621.0-h57928b3_1.conda", "db8dead3dd30fb1a032737554ce91e2819b43496a0db09927edf01c32b577450"),
            ("https://conda.anaconda.org/conda-forge/win-64/vc-14.3-ha32ba9b_23.conda", "986ddaf8feec2904eac9535a7ddb7acda1a1dfb9482088fdb8129f1595181663"),
            ("https://conda.anaconda.org/conda-forge/win-64/vc14_runtime-14.42.34433-he29a5d6_23.conda", "c483b090c4251a260aba6ff3e83a307bcfb5fb24ad7ced872ab5d02971bd3a49"),
        ];
        let conda_meta_path = target_prefix.join("conda-meta");
        std::fs::create_dir_all(&conda_meta_path).unwrap();
        let install_driver = InstallDriver::default();
        let mut prefix_records = Vec::new();
        for (package_url, expected_sha256) in files {
            let package_path =
                tools::download_and_cache_file_async(package_url.parse().unwrap(), expected_sha256)
                    .await
                    .unwrap();

            let package_dir = tempfile::TempDir::new().unwrap();

            // Create package cache
            rattler_package_streaming::fs::extract(&package_path, package_dir.path()).unwrap();

            // Link the package
            let paths = link_package(
                package_dir.path(),
                target_prefix,
                &install_driver,
                InstallOptions::default(),
            )
            .await
            .unwrap();

            let repodata_record = get_repodata_record(&package_path);
            // Construct a PrefixRecord for the package

            let prefix_record =
                PrefixRecord::from_repodata_record(repodata_record, None, None, paths, None, None);
            prefix_record
                .write_to_path(conda_meta_path.join(prefix_record.file_name()), true)
                .unwrap();
            prefix_records.push(prefix_record);
        }

        // Start bat to block deletion of the bat package
        let cmd_path = target_prefix.join("bin").join("bat.exe");
        let mut cmd = Command::new(&cmd_path);
        cmd.arg("-p");
        cmd.stdout(Stdio::piped());
        cmd.stdin(Stdio::piped());
        cmd.stderr(Stdio::null());
        cmd.env(
            "PATH",
            join_paths(chain(
                chain(
                    [target_prefix.to_path_buf()],
                    [
                        "Library/mingw-w64/bin",
                        "Library/usr/bin",
                        "Library/bin",
                        "Scripts",
                        "bin",
                    ]
                    .iter()
                    .map(|x| target_prefix.join(x)),
                ),
                split_paths(&var_os("PATH").unwrap()),
            ))
            .unwrap(),
        );
        let mut child = cmd.spawn().expect("failed to spawn bat.exe");
        let mut stdin = child.stdin.take().expect("failed to open stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("failed to open stdout"));
        // Ensure program has started by waiting for it to repeat back to us.
        let mut line = String::new();
        stdin.write_all(b"abc\n").unwrap();
        stdout.read_line(&mut line).unwrap();

        // Unlink the package
        assert!(!trash_dir.exists());
        let prefix_record = prefix_records.first().unwrap();
        unlink_package(target_prefix, prefix_record).await.unwrap();
        // Check if the conda-meta file is gone
        assert!(!conda_meta_path.join(prefix_record.file_name()).exists());
        assert!(trash_dir.exists());
        assert!(count_trash(&trash_dir) > 0);
        assert!(!cmd_path.exists());
        empty_trash(target_prefix).await.unwrap();
        assert!(count_trash(&trash_dir) > 0);

        drop(stdin);
        drop(stdout);
        child.wait().expect("bat failed");
        empty_trash(target_prefix).await.unwrap();
        assert!(count_trash(&trash_dir) == 0);
        assert!(!trash_dir.exists());
    }

    #[tokio::test]
    async fn test_empty_trash() {
        use uuid::Uuid;

        let environment_dir = tempfile::TempDir::new().unwrap();
        let trash_path = environment_dir.path().join(".trash");
        std::fs::create_dir_all(&trash_path).unwrap();
        {
            let mut file =
                File::create(trash_path.join(format!("{}.trash", Uuid::new_v4().simple())))
                    .unwrap();
            write!(file, "some data").unwrap();
        }
        {
            let mut file =
                File::create(trash_path.join(format!("{}.trash", Uuid::new_v4().simple())))
                    .unwrap();
            write!(file, "some other data").unwrap();
        }
        assert!(count_trash(&trash_path) == 2);
        empty_trash(environment_dir.path()).await.unwrap();
        assert!(!trash_path.exists());
    }
}
