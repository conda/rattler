use rattler_conda_types::package::ArchiveType;
use rattler_package_streaming::read::{extract_conda_via_streaming, extract_tar_bz2};
use rattler_package_streaming::write::{
    write_conda_package, write_tar_bz2_package, CompressionLevel,
};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn find_all_archives() -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|d| d.path())
}

fn find_all_package_files(path: &Path) -> Vec<PathBuf> {
    WalkDir::new(path)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .map(walkdir::DirEntry::into_path)
        .filter(|p| p.is_file())
        .collect::<Vec<_>>()
}

enum Decoder {
    TarBz2,
    Zst,
}

impl Decoder {
    fn open<'a, T: Read>(&'a self, f: &'a mut T) -> tar::Archive<Box<dyn std::io::Read + '_>> {
        match self {
            Decoder::TarBz2 => {
                let d = bzip2::read::BzDecoder::new(f);
                tar::Archive::new(Box::new(d))
            }
            Decoder::Zst => {
                let d = zstd::stream::read::Decoder::new(f).unwrap();
                tar::Archive::new(Box::new(d))
            }
        }
    }
}

enum FilterFiles {
    None,
    NoInfo,
    NoLicense,
}

/// Compare two tar archives by comparing the files, the file sizes and the file metadata (uid, gid, mode, mtime)
/// However, currently we skip some checks as we are not yet properly setting some of the values
/// Also note that there are some conda packagaging weirdnesses going on (like the info/licenses/... files in the pkg archive)
fn compare_two_tar_archives<T: Read>(
    f1: &mut T,
    f2: &mut T,
    decoder: Decoder,
    filter: FilterFiles,
) {
    let mut archive1 = decoder.open(f1);
    let mut archive2 = decoder.open(f2);

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

    // remove info/ files from map1 because conda-build adds the
    // info/licenses/... files to the pkg archive (not the info archive) for .conda packages
    match filter {
        FilterFiles::NoInfo => {
            let info = PathBuf::from("info");
            map1 = map1
                .into_iter()
                .filter(|(k, _)| !k.starts_with(&info))
                .collect::<HashMap<_, _>>();
        }
        FilterFiles::NoLicense => {
            let info_licenses = PathBuf::from("info/licenses");
            map2 = map2
                .into_iter()
                .filter(|(k, _)| !k.starts_with(&info_licenses))
                .collect::<HashMap<_, _>>();
        }
        FilterFiles::None => {}
    }
    assert_eq!(map1.len(), map2.len());

    for (path, header1) in map1 {
        let header2 = map2.get(&path).unwrap();
        assert_eq!(header1.size().unwrap(), header2.size().unwrap());
        // println!("Comparing {:?}", path);
        // println!(
        //     "mode: {:o} {:o}",
        //     header1.mode().unwrap(),
        //     header2.mode().unwrap()
        // );
        // assert_eq!(header1.mode().unwrap(), header2.mode().unwrap());
        // assert_eq!(header1.uid().unwrap(), header2.uid().unwrap());
        // assert_eq!(header1.gid().unwrap(), header2.gid().unwrap());
    }
}

fn compare_two_conda_archives(p1: &Path, p2: &Path) {
    println!("Comparing {p1:?} and {p2:?}");
    let mut archive1 = File::open(p1).unwrap();
    let mut archive2 = File::open(p2).unwrap();
    // open outer zip file
    let mut zip1 = zip::ZipArchive::new(&mut archive1).unwrap();
    let mut zip2 = zip::ZipArchive::new(&mut archive2).unwrap();

    // find metadata.json file in outer zip file
    let metadata1 = zip1.by_name("metadata.json").unwrap();
    let metadata2 = zip2.by_name("metadata.json").unwrap();

    // read metadata.json file
    let metadata_bytes1 = metadata1.bytes().collect::<Result<Vec<u8>, _>>().unwrap();
    let _mstr1 = std::str::from_utf8(&metadata_bytes1).unwrap();
    let metadata_bytes2 = metadata2.bytes().collect::<Result<Vec<u8>, _>>().unwrap();
    let _mstr2 = std::str::from_utf8(&metadata_bytes2).unwrap();

    // compare metadata.json files

    // TODO we should use the Python formatter!
    // assert_eq!(mstr1, mstr2);

    let filename = p1.file_stem().unwrap().to_string_lossy();
    let pkg_name = format!("pkg-{filename}.tar.zst");
    let info_name = format!("info-{filename}.tar.zst");

    {
        let mut pkg1 = zip1.by_name(&pkg_name).unwrap();
        let mut pkg2 = zip2.by_name(&pkg_name).unwrap();

        compare_two_tar_archives(&mut pkg1, &mut pkg2, Decoder::Zst, FilterFiles::NoInfo);
    }

    let mut info1 = zip1.by_name(&info_name).unwrap();
    let mut info2 = zip2.by_name(&info_name).unwrap();

    compare_two_tar_archives(&mut info1, &mut info2, Decoder::Zst, FilterFiles::NoLicense);
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
            &file_path.file_stem().unwrap().to_string_lossy()
        ));

        let writer = File::create(&new_archive).unwrap();
        let paths = find_all_package_files(&target_dir);
        write_tar_bz2_package(
            writer,
            &target_dir,
            &paths,
            CompressionLevel::Default,
            None,
            None,
        )
        .unwrap();

        // compare the two archives
        let mut f1 = File::open(&file_path).unwrap();
        let mut f2 = File::open(&new_archive).unwrap();

        compare_two_tar_archives(&mut f1, &mut f2, Decoder::TarBz2, FilterFiles::None);
    }
}

#[test]
fn test_rewrite_conda() {
    let temp_dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    println!("Target dir: {}", temp_dir.display());

    for file_path in find_all_archives().filter(|path| {
        ArchiveType::try_from(path) == Some(ArchiveType::Conda)
            && path.file_name().unwrap() != "stir-5.0.2-py38h9224444_7.conda"
    }) {
        println!("Name: {}", file_path.display());

        let name = file_path.file_stem().unwrap().to_string_lossy();
        let target_dir = temp_dir.join(file_path.file_stem().unwrap());
        extract_conda_via_streaming(File::open(&file_path).unwrap(), &target_dir).unwrap();

        let new_archive = temp_dir.join(format!(
            "{}-new.conda",
            &file_path.file_stem().unwrap().to_string_lossy()
        ));

        let writer = File::create(&new_archive).unwrap();
        let paths = find_all_package_files(&target_dir);
        write_conda_package(
            writer,
            &target_dir,
            &paths,
            CompressionLevel::Default,
            None,
            &name,
            None,
            None,
        )
        .unwrap();

        // compare the two archives
        compare_two_conda_archives(&file_path, &new_archive);
    }
}
