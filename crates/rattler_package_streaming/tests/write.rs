use rattler_conda_types::package::ArchiveType;
use rattler_package_streaming::read::extract_tar_bz2;
use rattler_package_streaming::write::write_tar_bz2_package;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn find_all_archives() -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|d| d.path())
}

fn find_all_package_files(path: &Path) -> Vec<PathBuf> {
    WalkDir::new(&path)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|p| p.is_file())
        .collect::<Vec<_>>()
}

fn compare_two_tar_bz2_archives(p1: &Path, p2: &Path) {
    println!("Comparing {:?} and {:?}", p1, p2);
    let mut archive1 = tar::Archive::new(bzip2::read::BzDecoder::new(File::open(p1).unwrap()));

    let mut archive2 = tar::Archive::new(bzip2::read::BzDecoder::new(File::open(p2).unwrap()));

    let entries1 = archive1.entries().unwrap();
    let entries2 = archive2.entries().unwrap();

    // create a map with entry.path as key and entry.header as value
    let mut map1 = HashMap::new();
    let mut map2 = HashMap::new();

    for entry in entries1 {
        let entry = entry.unwrap();
        let path = entry.path().unwrap().into_owned();
        map1.insert(path, entry.header().clone());
    }

    for entry in entries2 {
        let entry = entry.unwrap();
        let path = entry.path().unwrap().into_owned();
        map2.insert(path, entry.header().clone());
    }

    assert_eq!(map1.len(), map2.len());

    for (path, header1) in map1 {
        let header2 = map2.get(&path).unwrap();
        println!("Comparing {:?}", path);
        assert_eq!(header1.size().unwrap(), header2.size().unwrap());
        assert_eq!(header1.mode().unwrap(), header2.mode().unwrap());
        assert_eq!(header1.uid().unwrap(), header2.uid().unwrap());
        assert_eq!(header1.gid().unwrap(), header2.gid().unwrap());
    }
}

#[test]
fn test_rewrite_tar_bz2() {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());

    for file_path in
        find_all_archives().filter(|path| ArchiveType::try_from(path) == Some(ArchiveType::TarBz2))
    {
        println!("Name: {}", file_path.display());

        let target_dir = temp_dir.join(file_path.file_stem().unwrap());
        extract_tar_bz2(File::open(&file_path).unwrap(), &target_dir).unwrap();

        let new_archive = temp_dir.join(format!(
            "{}-new.tar.bz2",
            file_path.file_stem().unwrap().to_string_lossy().as_ref()
        ));

        let writer = File::create(&new_archive).unwrap();
        let paths = find_all_package_files(&target_dir);
        write_tar_bz2_package(writer, &target_dir, &paths, 9).unwrap();

        // compare the two archives
        // Note this is currently failing
        compare_two_tar_bz2_archives(&file_path, &new_archive);
    }
}
