use rattler_package_streaming::read::{extract_conda, extract_tar_bz2};
use rstest::rstest;
use rstest_reuse::{self, *};
use std::fs::File;
use std::path::{Path, PathBuf};

fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data")
}

#[template]
#[rstest]
#[case::conda(
    "conda-22.11.1-py38haa244fe_1.conda",
    "a8a44c5ff2b2f423546d49721ba2e3e632233c74a813c944adf8e5742834930e",
    "9987c96161034575f5a9c2be848960c5"
)]
#[case::mamba(
    "mamba-1.1.0-py39hb3d9227_2.conda",
    "c172acdf9cb7655dd224879b30361a657b09bb084b65f151e36a2b51e51a080a",
    "d87eb6aecfc0fe58299e6d6cfb252a7f"
)]
#[case::mock(
    "mock-2.0.0-py37_1000.conda",
    "181ec44eb7b06ebb833eae845bcc466ad96474be1f33ee55cab7ac1b0fdbbfa3",
    "23c226430e35a3bd994db6c36b9ac8ae"
)]
#[case::mujoco(
    "mujoco-2.3.1-ha3edaa6_0.conda",
    "007f27a98a150ac3fbbd5bdd708d35f807ba2e117a194f218b130890d461ce77",
    "910c94e2d1234e98196c4a64a82ff07e"
)]
#[case::ruff(
    "ruff-0.0.171-py310h298983d_0.conda",
    "25c755b97189ee066576b4ae3999d5e7ff4406d236b984742194e63941838dcd",
    "1ecacf57f20c0d1e4a04af0c8d4b54a3"
)]
#[case::stir(
    "stir-5.0.2-py38h9224444_7.conda",
    "352fe747f7f09b09baa4b6561485b3f0d4271f6f798d34dae7116c3c9c6ba896",
    "7bb9eb9ddaaf4505777512c5ad2fc108"
)]
fn conda_archives(#[case] input: &str, #[case] sha256: &str, #[case] md5: &str) {}

#[template]
#[rstest]
#[case::conda(
    "conda-22.9.0-py38haa244fe_2.tar.bz2",
    "3c2c2e8e81bde5fb1ac4b014f51a62411feff004580c708c97a0ec2b7058cdc4",
    "36194591e28b9f2c107aa3d952ac4649"
)]
#[case::mamba(
    "mamba-1.0.0-py38hecfeebb_2.tar.bz2",
    "f44c4bc9c6916ecc0e33137431645b029ade22190c7144eead61446dcbcc6f97",
    "dede6252c964db3f3e41c7d30d07f6bf"
)]
#[case::micromamba(
    "micromamba-1.1.0-0.tar.bz2",
    "5a1e1fe69a301e817cf2795ace03c9e4a42e97cd8984b6edbc8872dad00d5097",
    "3774689d66819fb50ff87fccefff6e88"
)]
#[case::mock(
    "mock-2.0.0-py37_1000.tar.bz2",
    "34c659b0fdc53d28ae721fd5717446fb8abebb1016794bd61e25937853f4c29c",
    "0f9cce120a73803a70abb14bd4d4900b"
)]
#[case::pytweening(
    "pytweening-1.0.4-pyhd8ed1ab_0.tar.bz2",
    "81644bcb60d295f7923770b41daf2d90152ef54b9b094c26513be50fccd62125",
    "d5e0fafeaa727f0de1c81bfb6e0e63d8"
)]
#[case::rosbridge(
    "ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2",
    "4dd9893f1eee45e1579d1a4f5533ef67a84b5e4b7515de7ed0db1dd47adc6bc8",
    "47d2678d67ec7ebd49ade2b9943e597e"
)]
#[case::zlib(
    "zlib-1.2.8-vc10_0.tar.bz2",
    "ee9172dbe9ebd158e8e68d6d0f7dc2060f0c8230b44d2e9a3595b7cd7336b915",
    "8415564d07857a1069c0cd74e7eeb369"
)]
fn tar_bz2_archives(#[case] input: &str, #[case] sha256: &str, #[case] md5: &str) {}

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
fn url_archives(#[case] input: &str, #[case] sha256: &str, #[case] md5: &str) {}

#[apply(conda_archives)]
fn test_extract_conda(#[case] input: &str, #[case] sha256: &str, #[case] md5: &str) {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());
    let file_path = Path::new(input);
    let target_dir = temp_dir.join(file_path.file_stem().unwrap());
    let result = extract_conda(
        File::open(test_data_dir().join(file_path)).unwrap(),
        &target_dir,
    )
    .unwrap();

    assert_eq!(&format!("{:x}", result.sha256), sha256);
    assert_eq!(&format!("{:x}", result.md5), md5);
}

#[apply(conda_archives)]
fn test_stream_info(#[case] input: &str, #[case] _sha256: &str, #[case] _md5: &str) {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());

    let file_path = Path::new(input);

    let mut info_stream = rattler_package_streaming::seek::stream_conda_info(
        File::open(test_data_dir().join(file_path)).unwrap(),
    )
    .unwrap();

    let target_dir = temp_dir.join(format!(
        "{}-info",
        file_path.file_stem().unwrap().to_string_lossy().as_ref()
    ));

    info_stream.unpack(target_dir).unwrap();
}

#[apply(tar_bz2_archives)]
fn test_extract_tar_bz2(#[case] input: &str, #[case] sha256: &str, #[case] md5: &str) {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());

    let file_path = Path::new(input);

    let target_dir = temp_dir.join(file_path.file_stem().unwrap());
    let result = extract_tar_bz2(
        File::open(test_data_dir().join(file_path)).unwrap(),
        &target_dir,
    )
    .unwrap();

    assert_eq!(&format!("{:x}", result.sha256), sha256);
    assert_eq!(&format!("{:x}", result.md5), md5);
}

#[cfg(feature = "tokio")]
#[apply(tar_bz2_archives)]
#[tokio::test]
async fn test_extract_tar_bz2_async(#[case] input: &str, #[case] sha256: &str, #[case] md5: &str) {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tokio");
    println!("Target dir: {}", temp_dir.display());

    let file_path = Path::new(input);
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

#[cfg(feature = "tokio")]
#[apply(conda_archives)]
#[tokio::test]
async fn test_extract_conda_async(#[case] input: &str, #[case] sha256: &str, #[case] md5: &str) {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tokio");
    println!("Target dir: {}", temp_dir.display());

    let file_path = Path::new(input);

    let target_dir = temp_dir.join(file_path.file_stem().unwrap());
    let result = rattler_package_streaming::tokio::async_read::extract_conda(
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

#[cfg(all(feature = "reqwest", feature = "blocking"))]
#[apply(url_archives)]
fn test_extract_url(#[case] url: &str, #[case] sha256: &str, #[case] md5: &str) {
    use rattler_networking::AuthenticatedClientBlocking;

    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());

    let (_, filename) = url.rsplit_once('/').unwrap();
    let name = Path::new(filename);
    println!("Name: {}", name.display());

    let target_dir = temp_dir.join(name);
    let result = rattler_package_streaming::reqwest::extract(
        AuthenticatedClientBlocking::default(),
        url,
        &target_dir,
    )
    .unwrap();

    assert_eq!(&format!("{:x}", result.sha256), sha256);
    assert_eq!(&format!("{:x}", result.md5), md5);
}

#[cfg(all(feature = "reqwest", feature = "tokio"))]
#[apply(url_archives)]
#[tokio::test]
async fn test_extract_url_async(#[case] url: &str, #[case] sha256: &str, #[case] md5: &str) {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tokio");
    println!("Target dir: {}", temp_dir.display());

    let (_, filename) = url.rsplit_once('/').unwrap();
    let name = Path::new(filename);
    println!("Name: {}", name.display());

    let target_dir = temp_dir.join(name);
    let url = url::Url::parse(url).unwrap();
    let result =
        rattler_package_streaming::reqwest::tokio::extract(Default::default(), url, &target_dir)
            .await
            .unwrap();

    assert_eq!(&format!("{:x}", result.sha256), sha256);
    assert_eq!(&format!("{:x}", result.md5), md5);
}
