use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use rattler_conda_types::package::IndexJson;
use rattler_package_streaming::{
    read::{extract_conda_via_buffering, extract_conda_via_streaming, extract_tar_bz2},
    ExtractError,
};
use rstest::rstest;
use rstest_reuse::{self, apply, template};
use serde_json::json;
use url::Url;

fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
}

#[template]
#[rstest]
#[case::conda(
    "https://conda.anaconda.org/conda-forge/win-64/conda-22.11.1-py38haa244fe_1.conda",
    "a8a44c5ff2b2f423546d49721ba2e3e632233c74a813c944adf8e5742834930e",
    "9987c96161034575f5a9c2be848960c5"
)]
#[case::mamba(
    "https://conda.anaconda.org/conda-forge/win-64/mamba-1.1.0-py39hb3d9227_2.conda",
    "c172acdf9cb7655dd224879b30361a657b09bb084b65f151e36a2b51e51a080a",
    "d87eb6aecfc0fe58299e6d6cfb252a7f"
)]
#[case::mock(
    "https://conda.anaconda.org/conda-forge/noarch/mock-5.0.0-pyhd8ed1ab_0.conda",
    "8ef7378ae3bcac5f1db9d95291b5c5ef98464ce51c18f8ec902d9e2c7c1bc49b",
    "d9d75bfae9eab6df13d8cbe650b9762d"
)]
#[case::mujoco(
    "https://conda.anaconda.org/conda-forge/linux-ppc64le/mujoco-2.3.1-ha3edaa6_0.conda",
    "007f27a98a150ac3fbbd5bdd708d35f807ba2e117a194f218b130890d461ce77",
    "910c94e2d1234e98196c4a64a82ff07e"
)]
#[case::ruff(
    "https://conda.anaconda.org/conda-forge/win-64/ruff-0.0.171-py310h298983d_0.conda",
    "25c755b97189ee066576b4ae3999d5e7ff4406d236b984742194e63941838dcd",
    "1ecacf57f20c0d1e4a04af0c8d4b54a3"
)]
#[case::stir(
    "https://conda.anaconda.org/conda-forge/win-64/stir-5.0.2-py38h9224444_7.conda",
    "352fe747f7f09b09baa4b6561485b3f0d4271f6f798d34dae7116c3c9c6ba896",
    "7bb9eb9ddaaf4505777512c5ad2fc108"
)]
fn conda_archives(#[case] input: Url, #[case] sha256: &str, #[case] md5: &str) {}

#[template]
#[rstest]
#[case::conda(
    "https://conda.anaconda.org/conda-forge/win-64/conda-22.9.0-py38haa244fe_2.tar.bz2",
    "3c2c2e8e81bde5fb1ac4b014f51a62411feff004580c708c97a0ec2b7058cdc4",
    "36194591e28b9f2c107aa3d952ac4649"
)]
#[case::mamba(
    "https://conda.anaconda.org/conda-forge/win-64/mamba-1.0.0-py38hecfeebb_2.tar.bz2",
    "f44c4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97",
    "dede6252c964db3f3e41c7d30d07f6bf"
)]
#[case::micromamba(
    "https://conda.anaconda.org/conda-forge/win-64/micromamba-1.1.0-0.tar.bz2",
    "5a1e1fe69a301e817cf2795ace03c9e4a42e97cd8984b6edbc8872dad00d5097",
    "3774689d66819fb50ff87fccefff6e88"
)]
#[case::mock(
    "https://conda.anaconda.org/conda-forge/win-64/mock-2.0.0-py37_1000.tar.bz2",
    "e85695f074ce4f77715f8f4873cc02fa5150efe2e5dadf4c85292edd5ffb5163",
    "df844836b49b9bd0bc857e70783f221e"
)]
#[case::pytweening(
    "https://conda.anaconda.org/conda-forge/noarch/pytweening-1.0.4-pyhd8ed1ab_0.tar.bz2",
    "81644bcb60d295f7923770b41daf2d90152ef54b9b094c26513be50fccd62125",
    "d5e0fafeaa727f0de1c81bfb6e0e63d8"
)]
#[case::rosbridge(
    "https://conda.anaconda.org/robostack/linux-64/ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2",
    "4dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc8",
    "47d2678d67ec7ebd49ade2b9943e597e"
)]
#[case::zlib(
    "https://conda.anaconda.org/conda-forge/win-64/zlib-1.2.8-vc10_0.tar.bz2",
    "ee9172dbe9ebd158e8e68d6d0f7dc2060f0c8230b44d2e9a3595b7cd7336b915",
    "8415564d07857a1069c0cd74e7eeb369"
)]
fn tar_bz2_archives(#[case] input: Url, #[case] sha256: &str, #[case] md5: &str) {}

#[template]
#[rstest]
#[case::ruff(
    "https://conda.anaconda.org/conda-forge/win-64/ruff-0.0.205-py39h5b3f8fb_0.conda",
    "8affd54b71aabc63ddc3944135a4b31462b09da7d1677a53cd31df50ffe07173",
    "bdfa0d81d2337eb713a66119754ad67a"
)]
#[case::python(
    "https://conda.anaconda.org/conda-forge/win-64/python-3.11.0-hcf16a7b_0_cpython.tar.bz2",
    "20d1f1b5dc620b745c325844545fd5c0cdbfdb2385a0e27ef1507399844c8c6d",
    "13ee3577afc291dabd2d9edc59736688"
)]
fn url_archives(#[case] input: Url, #[case] sha256: &str, #[case] md5: &str) {}

#[apply(conda_archives)]
fn test_extract_conda(#[case] input: Url, #[case] sha256: &str, #[case] md5: &str) {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));

    println!("Target dir: {}", temp_dir.display());
    let file_path = tools::download_and_cache_file(input, sha256).unwrap();
    let target_dir = temp_dir.join(file_path.file_stem().unwrap());
    let result = extract_conda_via_streaming(
        File::open(test_data_dir().join(file_path)).unwrap(),
        &target_dir,
    )
    .unwrap();

    assert_eq!(&format!("{:x}", result.sha256), sha256);
    assert_eq!(&format!("{:x}", result.md5), md5);
}

#[apply(conda_archives)]
fn test_stream_info(#[case] input: Url, #[case] sha256: &str, #[case] _md5: &str) {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());

    let file_path = tools::download_and_cache_file(input, sha256).unwrap();

    let mut info_stream = rattler_package_streaming::seek::stream_conda_info(
        File::open(test_data_dir().join(&file_path)).unwrap(),
    )
    .unwrap();

    let target_dir = temp_dir.join(format!(
        "{}-info",
        &file_path.file_stem().unwrap().to_string_lossy()
    ));

    info_stream.unpack(target_dir).unwrap();
}

#[apply(conda_archives)]
fn read_package_file(#[case] input: Url, #[case] sha256: &str, #[case] _md5: &str) {
    let file_path = tools::download_and_cache_file(input.clone(), sha256).unwrap();
    let index_json: IndexJson =
        rattler_package_streaming::seek::read_package_file(file_path).unwrap();
    let name = format!(
        "{}-{}-{}",
        index_json.name.as_normalized(),
        index_json.version,
        index_json.build
    );
    assert!(input
        .path_segments()
        .and_then(Iterator::last)
        .unwrap()
        .starts_with(&name));
}

#[apply(tar_bz2_archives)]
fn test_extract_tar_bz2(#[case] input: Url, #[case] sha256: &str, #[case] md5: &str) {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());

    let file_path = tools::download_and_cache_file(input, sha256).unwrap();

    let target_dir = temp_dir.join(file_path.file_stem().unwrap());
    let result = extract_tar_bz2(
        File::open(test_data_dir().join(file_path)).unwrap(),
        &target_dir,
    )
    .unwrap();

    assert_eq!(&format!("{:x}", result.sha256), sha256);
    assert_eq!(&format!("{:x}", result.md5), md5);
}

#[apply(tar_bz2_archives)]
#[tokio::test]
async fn test_extract_tar_bz2_async(#[case] input: Url, #[case] sha256: &str, #[case] md5: &str) {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tokio");
    println!("Target dir: {}", temp_dir.display());

    let file_path = tools::download_and_cache_file_async(input, sha256)
        .await
        .unwrap();
    let target_dir = temp_dir.join(file_path.file_stem().unwrap());
    let result = rattler_package_streaming::tokio::async_read::extract_tar_bz2(
        tokio::fs::File::open(&test_data_dir().join(file_path))
            .await
            .unwrap(),
        &target_dir,
    )
    .await
    .unwrap();

    assert_eq!(&format!("{:x}", result.sha256), sha256);
    assert_eq!(&format!("{:x}", result.md5), md5);
}

#[apply(conda_archives)]
#[tokio::test]
async fn test_extract_conda_async(#[case] input: Url, #[case] sha256: &str, #[case] md5: &str) {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tokio");
    println!("Target dir: {}", temp_dir.display());

    let file_path = tools::download_and_cache_file_async(input, sha256)
        .await
        .unwrap();

    let target_dir = temp_dir.join(file_path.file_stem().unwrap());
    let result: rattler_package_streaming::ExtractResult =
        rattler_package_streaming::tokio::async_read::extract_conda(
            tokio::fs::File::open(&test_data_dir().join(file_path))
                .await
                .unwrap(),
            &target_dir,
        )
        .await
        .unwrap();

    assert_eq!(&format!("{:x}", result.sha256), sha256);
    assert_eq!(&format!("{:x}", result.md5), md5);
}

#[cfg(feature = "reqwest")]
#[apply(url_archives)]
#[tokio::test]
async fn test_extract_url_async(#[case] url: &str, #[case] sha256: &str, #[case] md5: &str) {
    use reqwest::Client;
    use reqwest_middleware::ClientWithMiddleware;

    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tokio");
    println!("Target dir: {}", temp_dir.display());

    let (_, filename) = url.rsplit_once('/').unwrap();
    let name = Path::new(filename);
    println!("Name: {}", name.display());

    let target_dir = temp_dir.join(name);
    let url = url::Url::parse(url).unwrap();
    let result = rattler_package_streaming::reqwest::tokio::extract(
        ClientWithMiddleware::from(Client::new()),
        url,
        &target_dir,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(&format!("{:x}", result.sha256), sha256);
    assert_eq!(&format!("{:x}", result.md5), md5);
}

#[rstest]
fn test_extract_flaky_conda(#[values(0, 1, 13, 50, 74, 150, 8096, 16384, 20000)] cutoff: usize) {
    let package_path = tools::download_and_cache_file(
        "https://conda.anaconda.org/conda-forge/win-64/conda-22.11.1-py38haa244fe_1.conda"
            .parse()
            .unwrap(),
        "a8a44c5ff2b2f423546d49721ba2e3e632233c74a813c944adf8e5742834930e",
    )
    .unwrap();
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());
    let target_dir = temp_dir.join(package_path.file_stem().unwrap());
    let result = extract_conda_via_streaming(
        FlakyReader {
            reader: File::open(package_path).unwrap(),
            total_read: 0,
            cutoff,
        },
        &target_dir,
    )
    .expect_err("this should error out and not panic");

    assert_matches::assert_matches!(result, ExtractError::IoError(_));
}

#[rstest]
fn test_extract_data_descriptor_package_fails_streaming_and_uses_buffering() {
    let package_path = "tests/resources/ca-certificates-2024.7.4-hbcca054_0.conda";

    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let target_dir = temp_dir.join("package_using_data_descriptors");
    let result = extract_conda_via_streaming(File::open(package_path).unwrap(), &target_dir)
        .expect_err("this should error out and not panic");

    assert_matches::assert_matches!(
        result,
        ExtractError::ZipError(zip::result::ZipError::UnsupportedArchive(
            "The file length is not available in the local header"
        ))
    );

    let new_result =
        extract_conda_via_buffering(File::open(package_path).unwrap(), &target_dir).unwrap();

    let combined_result = json!({
        "sha256": format!("{:x}", new_result.sha256),
        "md5": format!("{:x}", new_result.md5),
    });

    insta::assert_snapshot!(combined_result, @r###"{"sha256":"6a5d6d8a1a7552dbf8c617312ef951a77d2dac09f2aeaba661deebce603a7a97","md5":"a1d1adb5a5dc516dfb3dccc7b9b574a9"}"###);
}

struct FlakyReader<R: Read> {
    reader: R,
    cutoff: usize,
    total_read: usize,
}

impl<R: Read> Read for FlakyReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.cutoff.saturating_sub(self.total_read);
        if remaining == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "flaky"));
        }
        let max_read = buf.len().min(remaining);
        let bytes_read = self.reader.read(&mut buf[..max_read])?;
        self.total_read += bytes_read;
        Ok(bytes_read)
    }
}
