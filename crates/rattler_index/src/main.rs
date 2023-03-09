use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
};

use fxhash::FxHashMap;
use md5::Md5;
use rattler_conda_types::repo_data::ChannelInfo;
use rattler_conda_types::{
    package::{IndexJson, PackageFile},
    PackageRecord, RepoData,
};
use rattler_digest::compute_file_digest;
use rattler_package_streaming::{read, seek};

use sha2::Sha256;

fn package_record_from_index_json<T: Read>(
    file: &Path,
    index_json_reader: &mut T,
) -> Result<PackageRecord, std::io::Error> {
    let index = IndexJson::from_reader(index_json_reader).unwrap();

    let sha256_result = compute_file_digest::<Sha256>(file).unwrap();
    let md5_result = compute_file_digest::<Md5>(file).unwrap();
    let size = std::fs::metadata(file).unwrap().len();

    let package_record = PackageRecord {
        name: index.name,
        version: index.version,
        build: index.build,
        build_number: index.build_number,
        subdir: index.subdir.unwrap_or_else(|| "unknown".to_string()),
        md5: Some(hex::encode(md5_result)),
        sha256: Some(hex::encode(sha256_result)),
        size: Some(size),
        arch: index.arch,
        platform: index.platform,
        depends: index.depends,
        constrains: index.constrains,
        track_features: index.track_features,
        features: index.features,
        noarch: index.noarch,
        license: index.license,
        license_family: index.license_family,
        timestamp: index.timestamp,
    };
    Ok(package_record)
}

fn package_record_from_tar_bz2(file: &Path) -> Result<PackageRecord, std::io::Error> {
    let reader = std::fs::File::open(file).unwrap();
    let mut archive = read::stream_tar_bz2(reader);

    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap();
        let path = path.to_str().unwrap();
        if path == "info/index.json" {
            return package_record_from_index_json(file, &mut entry);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "No index.json found",
    ))
}

fn package_record_from_conda(file: &Path) -> Result<PackageRecord, std::io::Error> {
    let reader = std::fs::File::open(file).unwrap();
    let mut archive = seek::stream_conda_info(reader).expect("Could not open conda file");

    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap();
        let path = path.to_str().unwrap();
        if path == "info/index.json" {
            return package_record_from_index_json(file, &mut entry);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "No index.json found",
    ))
}

fn main() {
    println!("Hello, world!");

    // find all tar.bz2 files in the current directory
    // for each file, create a PackageRecord
    // create a RepoData with the PackageRecords
    // print the RepoData as JSON
    let tar_bz2_glob = PathBuf::from("/Users/wolfv/micromamba/pkgs")
        .join("*.tar.bz2")
        .to_str()
        .unwrap()
        .to_string();

    let mut packages_subdir = HashMap::<String, FxHashMap<String, PackageRecord>>::default();

    let mut i = 0;
    for entry in glob::glob(&tar_bz2_glob).unwrap() {
        i += 1;
        if i > 10 {
            break;
        }
        match entry {
            Ok(path) => {
                println!("{:?}", path);
                let package_record = package_record_from_tar_bz2(&path).unwrap();
                println!("{:?}", package_record);
                let ix = path.file_name().unwrap().to_string_lossy().to_string();
                packages_subdir
                    .entry(package_record.subdir.clone())
                    .or_default()
                    .insert(ix, package_record);
            }
            Err(e) => println!("{:?}", e),
        }
    }

    let conda_glob = PathBuf::from("/Users/wolfv/micromamba/pkgs")
        .join("*.conda")
        .to_str()
        .unwrap()
        .to_string();

    let mut conda_packages_subdir = HashMap::<String, FxHashMap<String, PackageRecord>>::default();

    let mut i = 0;
    for entry in glob::glob(&conda_glob).unwrap() {
        i += 1;
        if i > 10 {
            break;
        }
        match entry {
            Ok(path) => {
                println!("{:?}", path);
                let package_record = package_record_from_conda(&path).unwrap();
                println!("{:?}", package_record);
                let ix = path.file_name().unwrap().to_string_lossy().to_string();
                conda_packages_subdir
                    .entry(package_record.subdir.clone())
                    .or_default()
                    .insert(ix, package_record);
            }
            Err(e) => println!("{:?}", e),
        }
    }

    // merge the two subdir keys
    let subdirs = packages_subdir
        .keys()
        .cloned()
        .chain(conda_packages_subdir.keys().cloned())
        .collect::<HashSet<_>>();

    for subdir in subdirs.into_iter() {
        let mut removed = HashSet::default();
        ["test", "a", "b", "xxx", "bla"].map(|s| removed.insert(s.to_string()));

        let repodata = RepoData {
            version: Some(1),
            info: Some(ChannelInfo {
                subdir: subdir.clone(),
            }),
            packages: packages_subdir.remove(&subdir).unwrap_or_default(),
            conda_packages: conda_packages_subdir.remove(&subdir).unwrap_or_default(),
            removed,
        };

        print!("{}", serde_json::to_string_pretty(&repodata).unwrap());
    }
}
