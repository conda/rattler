use std::{
    collections::{HashMap, HashSet},
    hash::BuildHasherDefault,
    path::{Path, PathBuf},
};

use md5::Md5;
use rattler_conda_types::repo_data::ChannelInfo;
use rattler_conda_types::{
    package::{IndexJson, PackageFile},
    PackageRecord, RepoData,
};
use rattler_digest::compute_file_digest;
use rattler_package_streaming::read;

use sha2::Sha256;

fn package_record_from_tar_bz2(file: &Path) -> Result<PackageRecord, std::io::Error> {
    let reader = std::fs::File::open(file).unwrap();
    let mut archive = read::stream_tar_bz2(reader);

    let sha256_result = compute_file_digest::<Sha256>(file).unwrap();
    let md5_result = compute_file_digest::<Md5>(file).unwrap();
    let size = std::fs::metadata(file).unwrap().len();

    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap();
        let path = path.to_str().unwrap();
        if path == "info/index.json" {
            let index = IndexJson::from_reader(&mut entry).unwrap();

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
                track_features: vec![],
                features: None,
                // track_features: index.track_features,
                // features: index.features,
                noarch: index.noarch,
                license: index.license,
                license_family: index.license_family,
                timestamp: index.timestamp,
            };
            return Ok(package_record);
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

    let mut packages =
        HashMap::<String, PackageRecord, BuildHasherDefault<fxhash::FxHasher>>::default();

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
                packages.insert(ix, package_record);
            }
            Err(e) => println!("{:?}", e),
        }
    }

    let mut removed = HashSet::default();
    ["test", "a", "b", "xxx", "bla"].map(|s| removed.insert(s.to_string()));

    let repodata = RepoData {
        version: Some(1),
        info: Some(ChannelInfo {
            subdir: "noarch".to_string(),
        }),
        packages,
        conda_packages: HashMap::default(),
        removed,
    };

    print!("{}", serde_json::to_string_pretty(&repodata).unwrap());
}
