use rattler_package_streaming::{
    read::{extract_conda, extract_tar_bz2, parallel_extract_tar_bz2},
    ArchiveType,
};
use std::fs::File;
use std::path::{Path, PathBuf};

fn find_all_archives() -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("data"))
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

#[test]
fn test_extract_tar_parallel() {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());

    for file_path in
        find_all_archives().filter(|path| ArchiveType::try_from(path) == Some(ArchiveType::TarBz2))
    {
        println!("Name: {}", file_path.display());

        let target_dir = temp_dir.join(format!(
            "{}-parallel",
            file_path.file_stem().unwrap().to_string_lossy().as_ref()
        ));
        parallel_extract_tar_bz2(
            File::open(&file_path).unwrap(),
            &target_dir,
            1024 * 1024 * 10,
        )
        .unwrap();
    }
}
