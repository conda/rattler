use rattler_conda_types::package::ArchiveType;
use rattler_package_streaming::read::{extract_conda, extract_tar_bz2};
use std::fs::File;
use std::path::{Path, PathBuf};

fn find_all_archives() -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|d| d.path())
}

#[test]
fn test_extract_conda() {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());

    for file_path in
        find_all_archives().filter(|path| ArchiveType::try_from(path) == Some(ArchiveType::Conda))
    {
        println!("Name: {}", file_path.display());

        let target_dir = temp_dir.join(file_path.file_stem().unwrap());
        extract_conda(File::open(&file_path).unwrap(), &target_dir).unwrap();
    }
}

#[test]
fn test_stream_info() {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());

    for file_path in
        find_all_archives().filter(|path| ArchiveType::try_from(path) == Some(ArchiveType::Conda))
    {
        println!("Name: {}", file_path.display());

        let mut info_stream =
            rattler_package_streaming::seek::stream_conda_info(File::open(&file_path).unwrap())
                .unwrap();

        let target_dir = temp_dir.join(format!(
            "{}-info",
            file_path.file_stem().unwrap().to_string_lossy().as_ref()
        ));

        info_stream.unpack(target_dir).unwrap();
    }
}

#[test]
fn test_extract_tar_bz2() {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());

    for file_path in
        find_all_archives().filter(|path| ArchiveType::try_from(path) == Some(ArchiveType::TarBz2))
    {
        println!("Name: {}", file_path.display());

        let target_dir = temp_dir.join(file_path.file_stem().unwrap());
        extract_tar_bz2(File::open(&file_path).unwrap(), &target_dir).unwrap();
    }
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn test_extract_tar_bz2_async() {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tokio");
    println!("Target dir: {}", temp_dir.display());

    for file_path in
        find_all_archives().filter(|path| ArchiveType::try_from(path) == Some(ArchiveType::TarBz2))
    {
        println!("Name: {}", file_path.display());

        let target_dir = temp_dir.join(file_path.file_stem().unwrap());
        rattler_package_streaming::tokio::async_read::extract_tar_bz2(
            tokio::fs::File::open(&file_path).await.unwrap(),
            &target_dir,
        )
        .await
        .unwrap();
    }
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn test_extract_conda_async() {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tokio");
    println!("Target dir: {}", temp_dir.display());

    for file_path in
        find_all_archives().filter(|path| ArchiveType::try_from(path) == Some(ArchiveType::Conda))
    {
        println!("Name: {}", file_path.display());

        let target_dir = temp_dir.join(file_path.file_stem().unwrap());
        rattler_package_streaming::tokio::async_read::extract_conda(
            tokio::fs::File::open(&file_path).await.unwrap(),
            &target_dir,
        )
        .await
        .unwrap();
    }
}

#[cfg(feature = "reqwest")]
#[test]
fn test_extract_url() {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());

    for url in [
        "https://conda.anaconda.org/conda-forge/win-64/ruff-0.0.205-py39h5b3f8fb_0.conda",
        "https://conda.anaconda.org/conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.tar.bz2",
    ] {
        let (_, filename) = url.rsplit_once('/').unwrap();
        let name = Path::new(filename);
        println!("Name: {}", name.display());

        let target_dir = temp_dir.join(name);
        rattler_package_streaming::reqwest::extract(Default::default(), url, &target_dir).unwrap();
    }
}

#[cfg(all(feature = "reqwest", feature = "tokio"))]
#[tokio::test]
async fn test_extract_url_async() {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tokio");
    println!("Target dir: {}", temp_dir.display());

    for url in [
        "https://conda.anaconda.org/conda-forge/win-64/ruff-0.0.205-py39h5b3f8fb_0.conda",
        "https://conda.anaconda.org/conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.tar.bz2",
    ] {
        let (_, filename) = url.rsplit_once('/').unwrap();
        let name = Path::new(filename);
        println!("Name: {}", name.display());

        let target_dir = temp_dir.join(name);
        rattler_package_streaming::reqwest::tokio::extract(Default::default(), url, &target_dir)
            .await
            .unwrap();
    }
}
