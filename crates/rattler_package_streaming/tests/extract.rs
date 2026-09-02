use std::{
    collections::BTreeMap,
    fs::File,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use fs_err::tokio as tokio_fs;
use rattler_conda_types::package::IndexJson;
use rattler_digest::{Md5, Sha256};
use rattler_package_streaming::{
    ExtractError, ExtractResult,
    read::{extract_conda_via_buffering, extract_conda_via_streaming, extract_tar_bz2},
};
use rstest::rstest;
use rstest_reuse::{self, apply, template};
use serde_json::json;
use tokio::io::{AsyncRead, ReadBuf};
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

#[cfg(feature = "reqwest")]
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
#[case::openmp(
    "https://conda.anaconda.org/conda-forge/linux-64/openmp-8.0.1-0.tar.bz2",
    "a3332e80c633be1ee20a41c7dd8810260a2132cf7d03f363d83752cad907bcfd",
    "b35241079152e5cc891c99368395b2c6"
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

    assert_eq!(hex::encode(result.sha256), sha256);
    assert_eq!(hex::encode(result.md5), md5);
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
    assert!(
        input
            .path_segments()
            .and_then(Iterator::last)
            .unwrap()
            .starts_with(&name)
    );
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

    assert_eq!(hex::encode(result.sha256), sha256);
    assert_eq!(hex::encode(result.md5), md5);
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
        tokio_fs::File::open(&test_data_dir().join(file_path))
            .await
            .unwrap(),
        &target_dir,
    )
    .await
    .unwrap();

    assert_eq!(hex::encode(result.sha256), sha256);
    assert_eq!(hex::encode(result.md5), md5);
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
            tokio_fs::File::open(&test_data_dir().join(file_path))
                .await
                .unwrap(),
            &target_dir,
        )
        .await
        .unwrap();

    assert_eq!(hex::encode(result.sha256), sha256);
    assert_eq!(hex::encode(result.md5), md5);
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

    assert_eq!(hex::encode(result.sha256), sha256);
    assert_eq!(hex::encode(result.md5), md5);
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

// Skip on windows as the test package contains symbolic links
#[cfg_attr(target_os = "windows", ignore)]
#[test]
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
        "sha256": hex::encode(new_result.sha256),
        "md5": hex::encode(new_result.md5),
    });

    insta::assert_snapshot!(combined_result, @r###"{"sha256":"6a5d6d8a1a7552dbf8c617312ef951a77d2dac09f2aeaba661deebce603a7a97","md5":"a1d1adb5a5dc516dfb3dccc7b9b574a9"}"###);
}

/// Regression test: a tar entry with an absolute path must not let the manual
/// mtime-setting touch a file *outside* the extraction directory. The content
/// is written safely inside `destination` (tar strips the leading root), and
/// the mtime must be applied to that sanitized path, never to the raw header
/// path joined onto `destination`.
#[cfg(unix)]
#[test]
fn test_absolute_path_entry_does_not_set_mtime_outside_destination() {
    // A file that lives outside the extraction destination. Kept short so it
    // fits the 100-byte tar name field.
    let victim = Path::new("/tmp/rattler_extract_traversal_victim.txt").to_path_buf();
    std::fs::write(&victim, b"data outside the extraction directory").unwrap();
    let untouched = filetime::FileTime::from_unix_time(1_700_000_000, 0);
    filetime::set_file_mtime(&victim, untouched).unwrap();

    // Craft an archive with a regular file whose header path is the victim's
    // absolute path, and a pre-1980 mtime to take the clamping branch.
    let content = b"x";
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(1);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_path_absolute(&victim).unwrap();
    header.set_cksum();

    let mut builder = tar::Builder::new(Vec::new());
    builder.append(&header, &content[..]).unwrap();
    let tar_data = builder.into_inner().unwrap();

    let mut bz2_data = Vec::new();
    let mut encoder = bzip2::write::BzEncoder::new(&mut bz2_data, bzip2::Compression::fast());
    encoder.write_all(&tar_data).unwrap();
    encoder.finish().unwrap();

    let target_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("absolute_path_traversal");
    let _ = std::fs::remove_dir_all(&target_dir);
    extract_tar_bz2(Cursor::new(bz2_data), &target_dir).unwrap();

    // The victim outside the destination must be completely untouched.
    let after =
        filetime::FileTime::from_last_modification_time(&std::fs::metadata(&victim).unwrap());
    assert_eq!(
        after.unix_seconds(),
        1_700_000_000,
        "mtime of a file outside the extraction directory was modified"
    );

    // The content is written inside the destination (root stripped) and its
    // mtime is clamped to the 1980 floor, proving mtimes are still applied to
    // the correct, sanitized path.
    let inside = target_dir.join(victim.strip_prefix("/").unwrap());
    assert!(
        inside.exists(),
        "entry should be extracted inside destination"
    );
    let inside_mtime =
        filetime::FileTime::from_last_modification_time(&std::fs::metadata(&inside).unwrap());
    assert_eq!(inside_mtime.unix_seconds(), 315_532_800);
}

/// Test that extracting a tar archive containing entries with mtime=1
/// (Unix epoch + 1 second, a common sentinel value) completes without error.
/// This verifies the fix for exFAT filesystems that cannot represent
/// timestamps before 1980-01-01.
#[test]
fn test_extract_tar_with_pre_1980_mtime() {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let target_dir = temp_dir.join("pre_1980_mtime_test");

    // Build a tar archive in memory with mtime=1 (sentinel value)
    let mut builder = tar::Builder::new(Vec::new());

    let content = b"hello world";
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(1); // Unix epoch + 1 second (1970-01-01T00:00:01Z)
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    builder
        .append_data(&mut header, "test_file.txt", &content[..])
        .unwrap();

    let tar_data = builder.into_inner().unwrap();

    // Wrap in bzip2 to create a .tar.bz2
    let mut bz2_data = Vec::new();
    let mut encoder = bzip2::write::BzEncoder::new(&mut bz2_data, bzip2::Compression::fast());
    std::io::Write::write_all(&mut encoder, &tar_data).unwrap();
    encoder.finish().unwrap();

    // Extract — this should succeed even though mtime=1 is pre-1980
    let result = extract_tar_bz2(Cursor::new(bz2_data), &target_dir);
    assert!(
        result.is_ok(),
        "Extraction should not fail due to mtime=1: {:?}",
        result.err()
    );

    // Verify the file was actually extracted
    let extracted_file = target_dir.join("test_file.txt");
    assert!(extracted_file.exists(), "Extracted file should exist");

    let mut extracted_content = Vec::new();
    File::open(&extracted_file)
        .unwrap()
        .read_to_end(&mut extracted_content)
        .unwrap();
    assert_eq!(extracted_content, content);
}

/// Same test but for the async extraction path.
#[tokio::test]
async fn test_extract_tar_with_pre_1980_mtime_async() {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("tokio");
    let target_dir = temp_dir.join("pre_1980_mtime_test_async");

    // Build a tar archive in memory with mtime=1 (sentinel value)
    let mut builder = tar::Builder::new(Vec::new());

    let content = b"hello world async";
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(1); // Unix epoch + 1 second (1970-01-01T00:00:01Z)
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    builder
        .append_data(&mut header, "test_file_async.txt", &content[..])
        .unwrap();

    let tar_data = builder.into_inner().unwrap();

    // Wrap in bzip2 to create a .tar.bz2
    let mut bz2_data = Vec::new();
    let mut encoder = bzip2::write::BzEncoder::new(&mut bz2_data, bzip2::Compression::fast());
    std::io::Write::write_all(&mut encoder, &tar_data).unwrap();
    encoder.finish().unwrap();

    // Write to a temp file so we can use tokio::fs::File as AsyncRead
    tokio::fs::create_dir_all(&temp_dir).await.unwrap();
    let archive_path = temp_dir.join("pre_1980_mtime_test.tar.bz2");
    tokio::fs::write(&archive_path, &bz2_data).await.unwrap();

    // Extract using the async path
    let file = tokio_fs::File::open(&archive_path).await.unwrap();
    let result =
        rattler_package_streaming::tokio::async_read::extract_tar_bz2(file, &target_dir).await;
    assert!(
        result.is_ok(),
        "Async extraction should not fail due to mtime=1: {:?}",
        result.err()
    );

    // Verify the file was actually extracted
    let extracted_file = target_dir.join("test_file_async.txt");
    assert!(extracted_file.exists(), "Extracted file should exist");

    let extracted_content = tokio::fs::read(&extracted_file).await.unwrap();
    assert_eq!(extracted_content, content);
}

/// The sync and async paths share the extraction code but feed it
/// differently, so the trees they write must be byte-identical.
#[apply(conda_archives)]
#[tokio::test]
async fn sync_and_async_extract_conda_identical_trees(
    #[case] input: Url,
    #[case] sha256: &str,
    #[case] md5: &str,
) {
    let file_path = tools::download_and_cache_file_async(input, sha256)
        .await
        .unwrap();
    let stem = file_path.file_stem().unwrap().to_string_lossy().to_string();

    let sync_dest = fresh_dir(&format!("identical_sync_{stem}"));
    let sync_result =
        extract_conda_via_streaming(File::open(&file_path).unwrap(), &sync_dest).unwrap();

    let async_dest = fresh_dir(&format!("identical_async_{stem}"));
    let async_result = rattler_package_streaming::tokio::async_read::extract_conda(
        tokio_fs::File::open(&file_path).await.unwrap(),
        &async_dest,
    )
    .await
    .unwrap();

    let expected = (
        sha256.to_string(),
        md5.to_string(),
        std::fs::metadata(&file_path).unwrap().len(),
    );
    assert_eq!(digests(&sync_result), expected);
    assert_eq!(digests(&async_result), expected);
    let sync_tree = snapshot(&sync_dest);
    assert!(!sync_tree.is_empty());
    assert_eq!(sync_tree, snapshot(&async_dest));
}

#[apply(tar_bz2_archives)]
#[tokio::test]
async fn sync_and_async_extract_tar_bz2_identical_trees(
    #[case] input: Url,
    #[case] sha256: &str,
    #[case] md5: &str,
) {
    let file_path = tools::download_and_cache_file_async(input, sha256)
        .await
        .unwrap();
    let stem = file_path.file_stem().unwrap().to_string_lossy().to_string();

    let sync_dest = fresh_dir(&format!("identical_sync_{stem}"));
    let sync_result = extract_tar_bz2(File::open(&file_path).unwrap(), &sync_dest).unwrap();

    let async_dest = fresh_dir(&format!("identical_async_{stem}"));
    let async_result = rattler_package_streaming::tokio::async_read::extract_tar_bz2(
        tokio_fs::File::open(&file_path).await.unwrap(),
        &async_dest,
    )
    .await
    .unwrap();

    let expected = (
        sha256.to_string(),
        md5.to_string(),
        std::fs::metadata(&file_path).unwrap().len(),
    );
    assert_eq!(digests(&sync_result), expected);
    assert_eq!(digests(&async_result), expected);
    let sync_tree = snapshot(&sync_dest);
    assert!(!sync_tree.is_empty());
    assert_eq!(sync_tree, snapshot(&async_dest));
}

/// linux-64 bzip2 ships six symlinks in both archive formats. Windows skips
/// them and extracts the rest, unix creates them.
#[rstest]
#[case::tar_bz2(
    "https://conda.anaconda.org/conda-forge/linux-64/bzip2-1.0.8-h7f98852_4.tar.bz2",
    "cb521319804640ff2ad6a9f118d972ed76d86bea44e5626c09a13d38f562e1fa",
    "a1fd65c7ccbf10880423d82bca54eb54"
)]
#[case::conda(
    "https://conda.anaconda.org/conda-forge/linux-64/bzip2-1.0.8-hd590300_5.conda",
    "242c0c324507ee172c0e0dd2045814e746bb303d1eb78870d182ceb0abc726a8",
    "69b8b6202a07720f448be700e300ccf4"
)]
#[tokio::test]
async fn packages_with_symlinks_extract(
    #[case] input: Url,
    #[case] sha256: &str,
    #[case] md5: &str,
) {
    let file_path = tools::download_and_cache_file_async(input, sha256)
        .await
        .unwrap();
    let dest = fresh_dir(&format!(
        "symlinks_{}",
        file_path.file_name().unwrap().to_string_lossy()
    ));
    let reader = tokio_fs::File::open(&file_path).await.unwrap();
    let result = if file_path.extension().is_some_and(|ext| ext == "conda") {
        rattler_package_streaming::tokio::async_read::extract_conda(reader, &dest).await
    } else {
        rattler_package_streaming::tokio::async_read::extract_tar_bz2(reader, &dest).await
    }
    .unwrap();
    assert_eq!(hex::encode(result.sha256), sha256);
    assert_eq!(hex::encode(result.md5), md5);

    for file in [
        "info/index.json",
        "bin/bzgrep",
        "bin/bzmore",
        "bin/bzdiff",
        "lib/libbz2.so.1.0.8",
    ] {
        assert!(
            std::fs::symlink_metadata(dest.join(file))
                .unwrap()
                .is_file(),
            "{file} is not a regular file"
        );
    }
    for (link, target) in [
        ("bin/bzfgrep", "bzgrep"),
        ("bin/bzegrep", "bzgrep"),
        ("bin/bzless", "bzmore"),
        ("bin/bzcmp", "bzdiff"),
        ("lib/libbz2.so", "libbz2.so.1.0.8"),
        ("lib/libbz2.so.1.0", "libbz2.so.1.0.8"),
    ] {
        let on_disk = dest.join(link);
        if cfg!(windows) {
            assert!(
                std::fs::symlink_metadata(&on_disk).is_err(),
                "{link} should be skipped on windows"
            );
        } else {
            let meta = std::fs::symlink_metadata(&on_disk).unwrap();
            assert!(meta.file_type().is_symlink(), "{link} is not a symlink");
            assert_eq!(std::fs::read_link(&on_disk).unwrap(), Path::new(target));
        }
    }
}

#[test]
fn dotdot_entry_is_skipped_and_never_escapes() {
    let tar_bz2 = RawTar::default()
        .file("../escape_dotdot.txt", b"escaped")
        .file("nested/../../escape_dotdot2.txt", b"escaped")
        .file("ok.txt", b"fine")
        .finish_bz2();
    let dest = fresh_dir("dotdot");
    extract_tar_bz2(Cursor::new(tar_bz2), &dest).unwrap();

    let parent = dest.parent().unwrap();
    assert!(!parent.join("escape_dotdot.txt").exists());
    assert!(!parent.join("escape_dotdot2.txt").exists());
    assert_eq!(std::fs::read(dest.join("ok.txt")).unwrap(), b"fine");
    let tree = snapshot(&dest);
    assert_eq!(tree.len(), 1, "unexpected extra entries: {tree:?}");
}

#[test]
fn file_under_symlink_pointing_outside_is_not_written_outside() {
    let outside = fresh_dir("symlink_escape_outside");
    std::fs::create_dir_all(&outside).unwrap();
    let outside_abs = outside.canonicalize().unwrap();

    // A relative target that resolves to the parent of the destination and
    // an absolute target.
    let targets = [
        ("rel", "..".to_string()),
        ("abs", outside_abs.to_string_lossy().to_string()),
    ];
    for (label, target) in targets {
        if target.len() >= 100 {
            eprintln!("skipping {label}: target too long for a raw header");
            continue;
        }
        let payload = format!("pwned_{label}.txt");
        let tar_bz2 = RawTar::default()
            .symlink("escape", &target)
            .file(&format!("escape/{payload}"), b"pwned")
            .dir("escape/subdir/")
            .file(&format!("escape/subdir/{payload}"), b"pwned")
            .file("after.txt", b"after")
            .finish_bz2();
        let dest = fresh_dir(&format!("symlink_escape_{label}"));
        let result = extract_tar_bz2(Cursor::new(tar_bz2), &dest);

        let parent = dest.parent().unwrap();
        assert!(
            !parent.join(&payload).exists(),
            "{label}: file written outside destination via symlink"
        );
        assert!(
            !parent.join("subdir").join(&payload).exists(),
            "{label}: file written outside destination via symlink + dir"
        );
        assert!(
            !outside_abs.join(&payload).exists(),
            "{label}: file written into absolute symlink target"
        );
        assert!(
            !outside_abs.join("subdir").exists(),
            "{label}: directory created inside absolute symlink target"
        );

        if cfg!(windows) {
            // The symlink is skipped, so `escape` is a plain directory.
            assert!(result.is_ok(), "{label}: {result:?}");
            assert_eq!(
                std::fs::read(dest.join("escape").join(&payload)).unwrap(),
                b"pwned"
            );
        } else {
            assert!(
                result.is_err(),
                "{label}: writing through an escaping symlink must fail"
            );
            assert!(
                !dest.join("after.txt").exists(),
                "{label}: extraction continued after the failure"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn directory_entry_over_escaping_symlink_does_not_touch_outside_mtime() {
    let outside = fresh_dir("dir_over_symlink_outside");
    std::fs::create_dir_all(&outside).unwrap();
    filetime::set_file_mtime(
        &outside,
        filetime::FileTime::from_unix_time(1_500_000_000, 0),
    )
    .unwrap();
    let outside_abs = outside.canonicalize().unwrap();
    if outside_abs.to_string_lossy().len() >= 100 {
        eprintln!("skipping: target too long for a raw header");
        return;
    }

    let tar_bz2 = RawTar::default()
        .symlink("s", &outside_abs.to_string_lossy())
        .dir_mtime("s/", 1_600_000_000)
        .finish_bz2();
    let dest = fresh_dir("dir_over_symlink");
    let result = extract_tar_bz2(Cursor::new(tar_bz2), &dest);

    let mtime = filetime::FileTime::from_last_modification_time(
        &std::fs::symlink_metadata(&outside).unwrap(),
    );
    assert_eq!(
        mtime.unix_seconds(),
        1_500_000_000,
        "directory entry over a symlink changed the mtime outside the destination (result: {result:?})"
    );
}

#[test]
fn symlink_replacing_validated_directory_does_not_escape() {
    let tar_bz2 = RawTar::default()
        .file("d/a.txt", b"a")
        .symlink("d", "..")
        .file("d/b_escape.txt", b"b")
        .finish_bz2();
    let dest = fresh_dir("symlink_replaces_dir");
    let result = extract_tar_bz2(Cursor::new(tar_bz2), &dest);
    assert!(
        !dest.parent().unwrap().join("b_escape.txt").exists(),
        "file written outside destination (result: {result:?})"
    );
}

#[test]
fn hard_link_with_source_outside_destination_is_rejected() {
    let outside = fresh_dir("hardlink_outside");
    std::fs::create_dir_all(&outside).unwrap();
    let outside_file = outside.join("secret.txt");
    std::fs::write(&outside_file, b"secret").unwrap();
    let outside_abs = outside_file.canonicalize().unwrap();

    let mut targets = vec![("relative", "../hardlink_outside/secret.txt".to_string())];
    if outside_abs.to_string_lossy().len() < 100 {
        targets.push(("absolute", outside_abs.to_string_lossy().to_string()));
    }

    for (label, target) in targets {
        let tar_bz2 = RawTar::default()
            .hardlink("h.txt", &target)
            .file("after.txt", b"after")
            .finish_bz2();
        let dest = fresh_dir(&format!("hardlink_outside_{label}"));
        let result = extract_tar_bz2(Cursor::new(tar_bz2), &dest);
        assert!(
            result.is_err(),
            "{label}: hard link to a file outside the destination was accepted"
        );
        assert!(
            !dest.join("h.txt").exists(),
            "{label}: hard link was created"
        );
        assert_eq!(std::fs::read(&outside_file).unwrap(), b"secret");
    }
}

#[cfg(unix)]
#[test]
fn hard_link_to_symlink_escaping_destination_is_rejected() {
    use std::os::unix::fs::MetadataExt;

    let outside = fresh_dir("hardlink_via_symlink_outside");
    std::fs::create_dir_all(&outside).unwrap();
    let outside_file = outside.join("secret.txt");
    std::fs::write(&outside_file, b"secret").unwrap();
    let outside_abs = outside_file.canonicalize().unwrap();
    if outside_abs.to_string_lossy().len() >= 100 {
        eprintln!("skipping: target too long for a raw header");
        return;
    }

    let tar_bz2 = RawTar::default()
        .symlink("s.txt", &outside_abs.to_string_lossy())
        .hardlink("h.txt", "s.txt")
        .finish_bz2();
    let dest = fresh_dir("hardlink_via_symlink");
    let result = extract_tar_bz2(Cursor::new(tar_bz2), &dest);
    assert!(
        result.is_err(),
        "hard link through an escaping symlink was accepted"
    );
    assert!(!dest.join("h.txt").exists());
    assert_eq!(
        std::fs::metadata(&outside_file).unwrap().nlink(),
        1,
        "outside file gained a hard link"
    );
}

#[test]
fn hard_link_to_earlier_file_in_archive() {
    let payload = noise(3000, 7);
    let tar_bz2 = RawTar::default()
        .file("a.txt", &payload)
        .hardlink("b.txt", "a.txt")
        .file("sub/x.txt", b"x")
        .hardlink("sub/y.txt", "sub/x.txt")
        .hardlink("z.txt", "./sub/x.txt")
        .file("c.txt", b"c")
        .finish_bz2();
    let dest = fresh_dir("hardlink_ok");
    extract_tar_bz2(Cursor::new(tar_bz2), &dest).unwrap();

    assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), payload);
    assert_eq!(std::fs::read(dest.join("b.txt")).unwrap(), payload);
    assert_eq!(std::fs::read(dest.join("sub/y.txt")).unwrap(), b"x");
    assert_eq!(std::fs::read(dest.join("z.txt")).unwrap(), b"x");
    assert_eq!(std::fs::read(dest.join("c.txt")).unwrap(), b"c");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(dest.join("a.txt")).unwrap().nlink(), 2);
        assert_eq!(
            std::fs::metadata(dest.join("sub/x.txt")).unwrap().nlink(),
            3
        );
    }
}

#[test]
fn empty_files_directories_and_long_names() {
    let long_name = format!("{}/{}.txt", "d".repeat(120), "f".repeat(60));
    let tar_bz2 = RawTar::default()
        .file("empty.txt", b"")
        .dir("subdir/")
        .file("subdir/inner.txt", b"inner")
        .dir("emptydir/")
        .dir("nested/deeper/")
        .old_dir("olddir/")
        .file("olddir/inner.txt", b"old")
        .file("./dot/prefixed.txt", b"dot")
        .long_file(&long_name, b"long")
        .file("last.txt", b"last")
        .finish_bz2();
    let dest = fresh_dir("empty_and_dirs");
    extract_tar_bz2(Cursor::new(tar_bz2), &dest).unwrap();

    assert_eq!(std::fs::metadata(dest.join("empty.txt")).unwrap().len(), 0);
    assert!(dest.join("subdir").is_dir());
    assert_eq!(
        std::fs::read(dest.join("subdir/inner.txt")).unwrap(),
        b"inner"
    );
    assert!(dest.join("emptydir").is_dir());
    assert!(dest.join("nested/deeper").is_dir());
    assert!(dest.join("olddir").is_dir());
    assert_eq!(
        std::fs::read(dest.join("olddir/inner.txt")).unwrap(),
        b"old"
    );
    assert_eq!(
        std::fs::read(dest.join("dot/prefixed.txt")).unwrap(),
        b"dot"
    );
    assert_eq!(std::fs::read(dest.join(&long_name)).unwrap(), b"long");
    assert_eq!(std::fs::read(dest.join("last.txt")).unwrap(), b"last");
}

#[test]
fn lying_size_headers_do_not_panic() {
    let data = b"0123456789";

    // The header claims more bytes than the whole archive holds.
    let too_big = RawTar::default()
        .file_with_size("big.txt", data, 5000)
        .finish_bz2();
    let dest = fresh_dir("size_too_big");
    let result = extract_tar_bz2(Cursor::new(too_big), &dest);
    assert!(
        result.is_err(),
        "size larger than the stream must fail, got {:?} with file of {:?} bytes",
        result,
        std::fs::metadata(dest.join("big.txt")).map(|m| m.len())
    );

    // The header claims more bytes than written but still within the padded
    // block: the entry is the data plus zero padding.
    let within_block = RawTar::default()
        .file_with_size("padded.txt", data, 300)
        .file("after.txt", b"after")
        .finish_bz2();
    let dest = fresh_dir("size_within_block");
    extract_tar_bz2(Cursor::new(within_block), &dest).unwrap();
    let padded = std::fs::read(dest.join("padded.txt")).unwrap();
    assert_eq!(padded.len(), 300);
    assert_eq!(&padded[..10], data);
    assert!(padded[10..].iter().all(|b| *b == 0));
    assert_eq!(std::fs::read(dest.join("after.txt")).unwrap(), b"after");

    // The header claims fewer bytes than written: only the claimed prefix is
    // the file and the archive continues at the next block.
    let too_small = RawTar::default()
        .file_with_size("short.txt", data, 3)
        .file("after.txt", b"after")
        .finish_bz2();
    let dest = fresh_dir("size_too_small");
    extract_tar_bz2(Cursor::new(too_small), &dest).unwrap();
    assert_eq!(std::fs::read(dest.join("short.txt")).unwrap(), b"012");
    assert_eq!(std::fs::read(dest.join("after.txt")).unwrap(), b"after");
}

#[cfg(unix)]
#[test]
fn file_entry_replaces_existing_symlink_without_following_it() {
    let outside = fresh_dir("overwrite_symlink_outside");
    std::fs::create_dir_all(&outside).unwrap();
    let outside_file = outside.join("victim.txt");
    std::fs::write(&outside_file, b"original").unwrap();

    let dest = fresh_dir("overwrite_symlink");
    std::fs::create_dir_all(&dest).unwrap();
    std::os::unix::fs::symlink(&outside_file, dest.join("victim")).unwrap();

    let tar_bz2 = RawTar::default().file("victim", b"new").finish_bz2();
    extract_tar_bz2(Cursor::new(tar_bz2), &dest).unwrap();

    assert_eq!(
        std::fs::read(&outside_file).unwrap(),
        b"original",
        "write followed the pre-existing symlink"
    );
    let meta = std::fs::symlink_metadata(dest.join("victim")).unwrap();
    assert!(meta.is_file(), "destination is still a symlink");
    assert_eq!(std::fs::read(dest.join("victim")).unwrap(), b"new");
}

/// A cut that drops only trailing bytes of a `.conda` (the zip central
/// directory) is not detected while streaming: the tar reader stops at the
/// end-of-archive marker before the zip reader reaches its own end. Such a
/// cut still yields a complete tree and a sha256 that does not match the
/// package. A `.tar.bz2` is drained to the end of the bzip2 stream after the
/// tar reader stops, so every cut is an error.
#[rstest]
#[case::conda(
    "https://conda.anaconda.org/conda-forge/win-64/ruff-0.0.171-py310h298983d_0.conda",
    "25c755b97189ee066576b4ae3999d5e7ff4406d236b984742194e63941838dcd"
)]
#[case::tar_bz2(
    "https://conda.anaconda.org/conda-forge/win-64/conda-22.9.0-py38haa244fe_2.tar.bz2",
    "3c2c2e8e81bde5fb1ac4b014f51a62411feff004580c708c97a0ec2b7058cdc4"
)]
#[tokio::test]
async fn truncated_streams_return_errors_and_never_hang(#[case] input: Url, #[case] sha256: &str) {
    let file_path = tools::download_and_cache_file_async(input, sha256)
        .await
        .unwrap();
    let stem = file_path.file_stem().unwrap().to_string_lossy().to_string();
    let is_conda = file_path.extension().is_some_and(|ext| ext == "conda");
    let data = std::fs::read(&file_path).unwrap();
    let len = data.len();

    let reference_dest = fresh_dir(&format!("truncated_reference_{stem}"));
    if is_conda {
        extract_conda_via_streaming(Cursor::new(&data), &reference_dest).unwrap();
    } else {
        extract_tar_bz2(Cursor::new(&data), &reference_dest).unwrap();
    }
    let reference = snapshot(&reference_dest);

    let mut cutoffs = vec![
        0,
        1,
        CHUNK_SIZE - 1,
        CHUNK_SIZE,
        CHUNK_SIZE + 1,
        len / 2,
        len - 1,
    ];
    cutoffs.retain(|c| *c < len);
    cutoffs.sort_unstable();
    cutoffs.dedup();

    for cutoff in cutoffs {
        let dest = fresh_dir(&format!("truncated_{stem}_{cutoff}"));
        let truncated = &data[..cutoff];
        let outcome = if is_conda {
            tokio::time::timeout(
                TEST_TIMEOUT,
                rattler_package_streaming::tokio::async_read::extract_conda(truncated, &dest),
            )
            .await
        } else {
            tokio::time::timeout(
                TEST_TIMEOUT,
                rattler_package_streaming::tokio::async_read::extract_tar_bz2(truncated, &dest),
            )
            .await
        };
        match outcome.unwrap_or_else(|_| panic!("{stem} cut at {cutoff}/{len}: extraction hung")) {
            Err(err) => eprintln!("{stem} cut at {cutoff}/{len}: {err}"),
            Ok(result) => {
                assert!(
                    is_conda,
                    "{stem} cut at {cutoff}/{len}: truncated tar.bz2 extracted without error"
                );
                assert_ne!(
                    hex::encode(result.sha256),
                    sha256,
                    "{stem} cut at {cutoff}/{len}: truncated stream hashed like the full package"
                );
                assert_eq!(
                    snapshot(&dest),
                    reference,
                    "{stem} cut at {cutoff}/{len}: succeeded with an incomplete tree"
                );
            }
        }
    }
}

#[tokio::test]
async fn reader_error_is_reported_instead_of_truncation_error() {
    let conda = std::fs::read(
        tools::download_and_cache_file_async(
            "https://conda.anaconda.org/conda-forge/win-64/ruff-0.0.171-py310h298983d_0.conda"
                .parse()
                .unwrap(),
            "25c755b97189ee066576b4ae3999d5e7ff4406d236b984742194e63941838dcd",
        )
        .await
        .unwrap(),
    )
    .unwrap();

    for cutoff in [10usize, 5000, CHUNK_SIZE, conda.len() - 10] {
        let dest = fresh_dir(&format!("reader_error_{cutoff}"));
        let reader = FailingReader {
            data: &conda,
            pos: 0,
            cutoff,
        };
        let result = tokio::time::timeout(
            TEST_TIMEOUT,
            rattler_package_streaming::tokio::async_read::extract_conda(reader, &dest),
        )
        .await
        .expect("hung");
        match result {
            Err(ExtractError::IoError(err)) => assert_eq!(
                err.kind(),
                std::io::ErrorKind::Interrupted,
                "cut at {cutoff}: expected the reader's own error, got: {err}"
            ),
            other => panic!("cut at {cutoff}: expected IoError(Interrupted), got {other:?}"),
        }
    }
}

/// Dropping the extraction future mid-stream must stop the blocking worker,
/// otherwise the runtime cannot shut down.
#[test]
fn cancelled_extraction_does_not_hang_runtime_shutdown() {
    let panics = Arc::new(AtomicUsize::new(0));
    {
        let panics = panics.clone();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            panics.fetch_add(1, Ordering::SeqCst);
            previous(info);
        }));
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    let conda = runtime.block_on(async {
        std::fs::read(
            tools::download_and_cache_file_async(
                "https://conda.anaconda.org/conda-forge/win-64/ruff-0.0.171-py310h298983d_0.conda"
                    .parse()
                    .unwrap(),
                "25c755b97189ee066576b4ae3999d5e7ff4406d236b984742194e63941838dcd",
            )
            .await
            .unwrap(),
        )
        .unwrap()
    });

    let dest = fresh_dir("cancelled");
    let started = Instant::now();
    runtime.block_on(async {
        let reader = SlowReader::new(conda, 512, Duration::from_millis(5));
        let outcome = tokio::time::timeout(
            Duration::from_millis(300),
            rattler_package_streaming::tokio::async_read::extract_conda(reader, &dest),
        )
        .await;
        assert!(
            outcome.is_err(),
            "extraction finished before it could be cancelled; slow the reader down"
        );
    });
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cancelling took {:?}",
        started.elapsed()
    );

    let started = Instant::now();
    runtime.shutdown_timeout(Duration::from_secs(30));
    let shutdown = started.elapsed();
    assert!(
        shutdown < Duration::from_secs(10),
        "runtime shutdown took {shutdown:?}; a blocking worker did not exit after cancellation"
    );
    assert_eq!(
        panics.load(Ordering::SeqCst),
        0,
        "a panic occurred during cancelled extraction"
    );
}

/// Files around the chunk size the async path hands to the worker.
#[tokio::test]
async fn large_files_extract_intact() {
    let files = [
        ("minus_one_128k.bin", noise(CHUNK_SIZE - 1, 4)),
        ("exact_128k.bin", noise(CHUNK_SIZE, 2)),
        ("plus_one_128k.bin", noise(CHUNK_SIZE + 1, 3)),
        ("one_mib_plus.bin", noise(1024 * 1024 + 7, 6)),
    ];
    let mut tar = RawTar::default();
    for (name, data) in &files {
        tar.file(name, data);
    }
    let pkg_tar = tar.finish();
    let tar_bz2 = bz2(&pkg_tar);
    let conda = build_conda("big", &pkg_tar);

    let tar_bz2_dest = fresh_dir("large_files_tar_bz2");
    let tar_bz2_result = rattler_package_streaming::tokio::async_read::extract_tar_bz2(
        tar_bz2.as_slice(),
        &tar_bz2_dest,
    )
    .await
    .unwrap();
    let conda_dest = fresh_dir("large_files_conda");
    let conda_result =
        rattler_package_streaming::tokio::async_read::extract_conda(conda.as_slice(), &conda_dest)
            .await
            .unwrap();

    for (label, bytes, result, dest) in [
        ("tar_bz2", &tar_bz2, tar_bz2_result, tar_bz2_dest),
        ("conda", &conda, conda_result, conda_dest),
    ] {
        assert_eq!(
            digests(&result),
            (sha256_hex(bytes), md5_hex(bytes), bytes.len() as u64),
            "{label}"
        );
        for (name, data) in &files {
            let on_disk = std::fs::read(dest.join(name))
                .unwrap_or_else(|err| panic!("{label}: missing {name}: {err}"));
            assert_eq!(on_disk.len(), data.len(), "{label}: length of {name}");
            assert!(on_disk == *data, "{label}: contents of {name} differ");
        }
    }
}

/// `reqwest::tokio::extract` retries with buffering when streaming fails on
/// a package that uses data descriptors.
#[cfg(feature = "reqwest")]
#[tokio::test]
async fn data_descriptor_package_over_http_falls_back_to_buffering() {
    use reqwest::Client;
    use reqwest_middleware::ClientWithMiddleware;
    use tower_http::services::ServeDir;

    let package = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/resources/ca-certificates-2024.7.4-hbcca054_0.conda");
    let bytes = std::fs::read(&package).unwrap();

    // Streaming this package must still fail with the data descriptor error,
    // otherwise the fallback is not what this test exercises.
    let streaming_dest = fresh_dir("data_descriptor_streaming");
    let streaming = rattler_package_streaming::tokio::async_read::extract_conda(
        bytes.as_slice(),
        &streaming_dest,
    )
    .await;
    assert_matches::assert_matches!(
        streaming,
        Err(ExtractError::ZipError(
            zip::result::ZipError::UnsupportedArchive(
                "The file length is not available in the local header"
            )
        ))
    );

    let app = axum::Router::new().fallback_service(ServeDir::new(package.parent().unwrap()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let url = Url::parse(&format!(
        "http://{addr}/{}",
        package.file_name().unwrap().to_string_lossy()
    ))
    .unwrap();

    let dest = fresh_dir("data_descriptor_http");
    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        rattler_package_streaming::reqwest::tokio::extract(
            ClientWithMiddleware::from(Client::new()),
            url,
            &dest,
            None,
            None,
        ),
    )
    .await
    .expect("hung")
    .expect("buffering fallback failed");
    assert_eq!(
        hex::encode(result.sha256),
        "6a5d6d8a1a7552dbf8c617312ef951a77d2dac09f2aeaba661deebce603a7a97"
    );
    assert_eq!(hex::encode(result.md5), "a1d1adb5a5dc516dfb3dccc7b9b574a9");

    assert!(dest.join("info/index.json").is_file());
    assert!(dest.join("ssl/cacert.pem").is_file());
    let link = dest.join("ssl/cert.pem");
    if cfg!(windows) {
        assert!(
            std::fs::symlink_metadata(&link).is_err(),
            "symlink should be skipped on windows"
        );
    } else {
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("cacert.pem"));
    }
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
            return Err(std::io::Error::other("flaky"));
        }
        let max_read = buf.len().min(remaining);
        let bytes_read = self.reader.read(&mut buf[..max_read])?;
        self.total_read += bytes_read;
        Ok(bytes_read)
    }
}

/// Bytes per chunk the async path hands to the extraction worker.
const CHUNK_SIZE: usize = 128 * 1024;
const TEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Path of an empty directory below `CARGO_TARGET_TMPDIR`. Its parent exists
/// so tests can also place files next to a destination.
fn fresh_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("hostile")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
    dir
}

fn digests(result: &ExtractResult) -> (String, String, u64) {
    (
        hex::encode(result.sha256),
        hex::encode(result.md5),
        result.total_size,
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(rattler_digest::compute_bytes_digest::<Sha256>(bytes))
}

fn md5_hex(bytes: &[u8]) -> String {
    hex::encode(rattler_digest::compute_bytes_digest::<Md5>(bytes))
}

#[derive(Debug, PartialEq, Eq)]
enum Node {
    File {
        sha256: String,
        mtime: i64,
        #[cfg(unix)]
        mode: u32,
    },
    Dir,
    Symlink(PathBuf),
}

/// Every entry below `root` keyed by its relative path. Symlinks are not
/// followed.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Node> {
    let mut nodes = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false).min_depth(1) {
        let entry = entry.unwrap();
        let rel = entry.path().strip_prefix(root).unwrap().to_path_buf();
        let meta = std::fs::symlink_metadata(entry.path()).unwrap();
        let node = if meta.file_type().is_symlink() {
            Node::Symlink(std::fs::read_link(entry.path()).unwrap())
        } else if meta.is_dir() {
            Node::Dir
        } else {
            Node::File {
                sha256: sha256_hex(&std::fs::read(entry.path()).unwrap()),
                mtime: filetime::FileTime::from_last_modification_time(&meta).unix_seconds(),
                #[cfg(unix)]
                mode: {
                    use std::os::unix::fs::PermissionsExt;
                    meta.permissions().mode() & 0o777
                },
            }
        };
        nodes.insert(rel, node);
    }
    nodes
}

/// Deterministic pseudo-random bytes so bzip2 cannot collapse them.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn bz2(tar: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = bzip2::write::BzEncoder::new(&mut out, bzip2::Compression::fast());
    encoder.write_all(tar).unwrap();
    encoder.finish().unwrap();
    out
}

const RAW_MTIME: u64 = 1_700_000_000;

fn raw_header(kind: tar::EntryType, mode: u32, mtime: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_mode(mode);
    header.set_mtime(mtime);
    header
}

/// Builds tar archives from raw 512-byte headers so entries can carry names,
/// link targets and sizes that `tar::Builder` would refuse or fix up.
#[derive(Default)]
struct RawTar(Vec<u8>);

impl RawTar {
    /// Writes `name` and `link` straight into the header fields, then `data`
    /// padded to a whole block. `size` is what the header claims.
    fn push(
        &mut self,
        mut header: tar::Header,
        name: &str,
        link: Option<&str>,
        size: u64,
        data: &[u8],
    ) -> &mut Self {
        assert!(name.len() < 100, "raw name too long: {name}");
        let raw = header.as_old_mut();
        raw.name[..name.len()].copy_from_slice(name.as_bytes());
        if let Some(link) = link {
            assert!(link.len() < 100, "raw link name too long: {link}");
            raw.linkname[..link.len()].copy_from_slice(link.as_bytes());
        }
        header.set_size(size);
        header.set_cksum();
        self.0.extend_from_slice(header.as_bytes());
        self.0.extend_from_slice(data);
        let pad = (512 - data.len() % 512) % 512;
        self.0.extend(std::iter::repeat_n(0u8, pad));
        self
    }

    fn file(&mut self, name: &str, data: &[u8]) -> &mut Self {
        self.file_with_size(name, data, data.len() as u64)
    }

    fn file_with_size(&mut self, name: &str, data: &[u8], size: u64) -> &mut Self {
        let header = raw_header(tar::EntryType::Regular, 0o644, RAW_MTIME);
        self.push(header, name, None, size, data)
    }

    fn dir(&mut self, name: &str) -> &mut Self {
        let header = raw_header(tar::EntryType::Directory, 0o755, RAW_MTIME);
        self.push(header, name, None, 0, &[])
    }

    #[cfg(unix)]
    fn dir_mtime(&mut self, name: &str, mtime: u64) -> &mut Self {
        let header = raw_header(tar::EntryType::Directory, 0o755, mtime);
        self.push(header, name, None, 0, &[])
    }

    /// A pre-ustar header without magic. Such archives mark directories with
    /// a trailing slash on a regular file entry.
    fn old_dir(&mut self, name: &str) -> &mut Self {
        assert!(name.ends_with('/'));
        let mut header = tar::Header::new_old();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o755);
        header.set_mtime(RAW_MTIME);
        self.push(header, name, None, 0, &[])
    }

    fn symlink(&mut self, name: &str, target: &str) -> &mut Self {
        let header = raw_header(tar::EntryType::Symlink, 0o777, RAW_MTIME);
        self.push(header, name, Some(target), 0, &[])
    }

    fn hardlink(&mut self, name: &str, target: &str) -> &mut Self {
        let header = raw_header(tar::EntryType::Link, 0o644, RAW_MTIME);
        self.push(header, name, Some(target), 0, &[])
    }

    /// Appends an entry through `tar::Builder` so GNU long-name records are
    /// produced for paths over 100 bytes.
    fn long_file(&mut self, name: &str, data: &[u8]) -> &mut Self {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = raw_header(tar::EntryType::Regular, 0o644, RAW_MTIME);
        header.set_size(data.len() as u64);
        header.set_cksum();
        builder.append_data(&mut header, name, data).unwrap();
        let bytes = builder.into_inner().unwrap();
        // Drop the two end-of-archive blocks the builder appends.
        self.0.extend_from_slice(&bytes[..bytes.len() - 1024]);
        self
    }

    fn finish(&mut self) -> Vec<u8> {
        let mut out = std::mem::take(&mut self.0);
        out.extend(std::iter::repeat_n(0u8, 1024));
        out
    }

    fn finish_bz2(&mut self) -> Vec<u8> {
        bz2(&self.finish())
    }
}

/// Builds a minimal `.conda` archive around the given package tar.
fn build_conda(name: &str, pkg_tar: &[u8]) -> Vec<u8> {
    let index = format!(
        r#"{{"name":"{name}","version":"1.0","build":"0","build_number":0,"subdir":"noarch","depends":[]}}"#
    );
    let mut info = tar::Builder::new(Vec::new());
    let mut header = raw_header(tar::EntryType::Regular, 0o644, RAW_MTIME);
    header.set_size(index.len() as u64);
    header.set_cksum();
    info.append_data(&mut header, "info/index.json", index.as_bytes())
        .unwrap();
    let info_tar = info.into_inner().unwrap();

    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("metadata.json", options).unwrap();
    writer
        .write_all(br#"{"conda_pkg_format_version": 2}"#)
        .unwrap();
    writer
        .start_file(format!("info-{name}-1.0-0.tar.zst"), options)
        .unwrap();
    writer
        .write_all(&zstd::stream::encode_all(Cursor::new(info_tar), 3).unwrap())
        .unwrap();
    writer
        .start_file(format!("pkg-{name}-1.0-0.tar.zst"), options)
        .unwrap();
    writer
        .write_all(&zstd::stream::encode_all(Cursor::new(pkg_tar), 3).unwrap())
        .unwrap();
    writer.finish().unwrap().into_inner()
}

/// An `AsyncRead` that hands out `chunk` bytes every `delay`.
struct SlowReader {
    data: Vec<u8>,
    pos: usize,
    chunk: usize,
    delay: Duration,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl SlowReader {
    fn new(data: Vec<u8>, chunk: usize, delay: Duration) -> Self {
        Self {
            data,
            pos: 0,
            chunk,
            delay,
            sleep: None,
        }
    }
}

impl AsyncRead for SlowReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.pos >= self.data.len() {
            return Poll::Ready(Ok(()));
        }
        if self.sleep.is_none() {
            let delay = self.delay;
            self.sleep = Some(Box::pin(tokio::time::sleep(delay)));
        }
        match self.sleep.as_mut().unwrap().as_mut().poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(()) => self.sleep = None,
        }
        let n = self
            .chunk
            .min(buf.remaining())
            .min(self.data.len() - self.pos);
        let start = self.pos;
        buf.put_slice(&self.data[start..start + n]);
        self.pos += n;
        Poll::Ready(Ok(()))
    }
}

/// An `AsyncRead` that fails with an `Interrupted` error after `cutoff`
/// bytes.
struct FailingReader<'a> {
    data: &'a [u8],
    pos: usize,
    cutoff: usize,
}

impl AsyncRead for FailingReader<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.pos >= self.cutoff {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "reader gave up",
            )));
        }
        let n = buf
            .remaining()
            .min(self.cutoff - self.pos)
            .min(self.data.len() - self.pos)
            .min(1024);
        let start = self.pos;
        buf.put_slice(&self.data[start..start + n]);
        self.pos += n;
        Poll::Ready(Ok(()))
    }
}
