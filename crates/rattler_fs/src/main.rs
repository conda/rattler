use clap::{value_parser, Arg, Command};
use fuser::{BackgroundSession, Config, MountOption};
use std::{collections::HashMap, io::stdin, path::{Path, PathBuf}, sync::Arc};

use rattler_lock::{CondaBinaryData, LockFile, LockedPackageRef, DEFAULT_ENVIRONMENT_NAME};
use rattler_conda_types::{Platform, package::{PathsEntry, PathsJson, PrefixPlaceholder} };
use rattler_cache::{default_cache_dir, package_cache::{CacheMetadata, PackageCache}};
use rattler_networking::{LazyClient};

mod virtual_fs;
use virtual_fs::VirtualFS;

mod fuse_directory;
use fuse_directory::{FuseMetadata};
mod prefix_replacement;

// prefix_placeholder == PathsV2

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let fs_type = "fuse";

    let (pixi_lock, mountpoint) = handle_input_arguments();

    let lockfile = LockFile::from_path(&pixi_lock)?;
    let environment_name = DEFAULT_ENVIRONMENT_NAME; 
    let default_environment = lockfile.environment(&environment_name).unwrap();
    let current_platform = Platform::current();
    let package_refs = default_environment.packages(current_platform).unwrap();

    let mut env_paths: Vec<FuseMetadata> = vec![];
    let top_dir = FuseMetadata::new_directory(PathBuf::from("."), 0);
    env_paths.push(top_dir); 

    // hash map of absolute path to index
    let mut directory_indices: HashMap<PathBuf, usize> = HashMap::new(); 
    directory_indices.insert(PathBuf::from("."), 0); // added
    // let package_ref = package_refs.next().unwrap(); // in the end profuct should loop over all the packages in the ref
    
    for package_ref in package_refs {
        let (paths_json, cachelock) = get_paths_json(package_ref).await?;
        path_parse(paths_json, cachelock,  &mut env_paths, &mut directory_indices);
        println!("Parsed {} metadata entries", env_paths.len());
    }
    
    println!("mountpoint: {mountpoint:#?}");
    let fs_session = connect_to_remote_fs(fs_type, &mountpoint, &mut env_paths).unwrap();

    println!("Press enter to unmount current session");
    let mut buffer = String::new();

    stdin().read_line(&mut buffer).unwrap();
    fs_session.join();

    Ok(())
    // unmount automatically before mounting if needed
}

async fn get_paths_json(package_ref: LockedPackageRef<'_>) -> anyhow::Result<(PathsJson, CacheMetadata)> {
    let package_data = package_ref.as_binary_conda().unwrap();   
    let cache_dir = default_cache_dir()?.join("pkgs"); // FIX should not be hardcoded -- 
    
    println!("{:#?}", cache_dir);
    let cache = PackageCache::new(cache_dir);
    let (paths_json, cachelock) = solve_package(cache, package_data).await;
    Ok((paths_json, cachelock))
}

fn path_parse (paths_json: PathsJson, cachelock: CacheMetadata, env_paths: &mut Vec<FuseMetadata>, directory_indices: &mut HashMap<PathBuf, usize>){

    paths_json.paths.iter().for_each(|path| {
        let cachepath : Arc<Path> = cachelock.path().into();
        let parent_directory = path.relative_path.parent().unwrap_or(Path::new("."));
        // let parent_components = parent_directory.components();
        let mut parent_index = 0;

        parent_directory.components().for_each(
            |component|{
                let current_path = env_paths[parent_index]
                    .as_directory()
                    .expect("First element is always the root directory")
                    .prefix_path
                    .join(component);

                parent_index = match directory_indices.get(&current_path) { 
                    Some(&index) => index,
                    None => {
                        let new_dir = FuseMetadata::new_directory(current_path.clone(), parent_index);
                        let child_index = env_paths.len(); // TODO: Is there a better way of knowing the index of the new item?

                        env_paths.push(new_dir);
                        env_paths[parent_index]
                            .as_directory_mut()
                            .expect("Parent is a directory")
                            .children.push(child_index);

                        directory_indices.insert(current_path, child_index);
                        child_index
                    }
                };
        });

        let file_name = path.relative_path.file_name().expect("Files always have names");

        // maybe hide as prefix function
        let prefix_placeholder = collect_prefix_placeholder(&path);

        // let file_permissions = path.permissions();
        let file_index = env_paths.len();
        env_paths.push(FuseMetadata::new_file(
            file_name.into(), 
            parent_index,
            cachepath.clone(),
            path.path_type.clone(), 
            prefix_placeholder
        ));

        // TODO: Is there a better way of knowing the index of the new item?
        env_paths[parent_index]
            .as_directory_mut()
            .expect("Parents are always directories")
            .children.push(file_index);
    });
    // println!("add the env_paths: {:#?}", &env_paths);

}

fn collect_prefix_placeholder(paths_entry: &PathsEntry) -> Option<PrefixPlaceholder> {
    match &paths_entry.prefix_placeholder {
        Some(prefix_placeholder) => Some(PrefixPlaceholder::new(prefix_placeholder.file_mode, prefix_placeholder.placeholder.clone())),
        None => None
    }
}

fn handle_input_arguments() -> (PathBuf, PathBuf) { // return the pixi_lock & the mount_point
    let matches = Command::new("mount")
        .arg(
            Arg::new( "PIXI_LOCK")
                .required(true)
                .index(1)
                .value_parser(value_parser!(PathBuf))
                .help("Pixi lock file to mount"),
            )
        .arg(
            Arg::new("MOUNT_POINT")
                .required(true)
                .index(2)
                .value_parser(value_parser!(PathBuf))
                .help("Act as a client, and mount FUSE at given path"),
            )
        .get_matches();

    env_logger::init();
    
    let pixi_lock = matches.get_one::<PathBuf>("PIXI_LOCK").unwrap().to_path_buf();
    let mountpoint = matches.get_one::<PathBuf>("MOUNT_POINT").unwrap().to_path_buf();
    
    (pixi_lock, mountpoint.canonicalize().unwrap())
}

fn connect_to_remote_fs(fs_type: &str, mountpoint: &Path, metadata: &mut Vec<FuseMetadata> ) -> Option<BackgroundSession, >{ // when implementing other mounts, need to create own struct with impl like unmount
    match fs_type {
        "fuse" => {
            let options = 
            vec![
                MountOption::RO, 
                MountOption::FSName("conda-packages".to_string()),
                MountOption::AutoUnmount,
                MountOption::AllowOther
            ];
            Some(fuser::spawn_mount2(VirtualFS::new(metadata, mountpoint), mountpoint, &Config(options)).unwrap())
        }
        _ => {
            println!("Invalid FS Type");
            None
        }
    }
}

async fn solve_package(cache: PackageCache, package_data: &CondaBinaryData) -> (PathsJson, CacheMetadata){
    let package_record = &package_data.package_record;
    let package_url = package_data.location.as_url().unwrap().clone();
    let client = LazyClient::default();
    // our actual crate would do get_or_fetch but then from a remote thing meaning it will retrieve the packages and have a unique client (adding packages to CVMFS if they don't exist yet)
    let cache_lock = cache.get_or_fetch_from_url(package_record, package_url, client, None).await.unwrap(); 
    let paths_json = PathsJson::from_package_directory_with_deprecated_fallback(cache_lock.path()).unwrap();

    println!("solving {:#?} packages from {:#?}", paths_json.paths.len(), cache_lock.path());
    (paths_json, cache_lock)
}


// #[cfg(test)]
// mod tests {
//     use super::*;
//     use rattler_conda_types::package::{PathType, PathsEntry};

//     #[test]
//     fn test_path_parse_creates_directory_tree() {
//         // Fake PathsJson with two files
//         let fake_paths = PathsJson {
//             paths: vec![
//                 PathsEntry {
//                     relative_path: PathBuf::from("bin/python"),
//                     path_type: PathType::HardLink,
//                     prefix_placeholder: None,
//                     no_link: false,
//                     sha256: None,
//                     size_in_bytes: None,
//                 },
//                 PathsEntry {
//                     relative_path: PathBuf::from("lib/site.py"),
//                     path_type: PathType::HardLink,
//                     prefix_placeholder: None,
//                     no_link: false,
//                     sha256: None,
//                     size_in_bytes: None,
//                 },
//             ],
//             paths_version: 01,
//         };

//         let env_paths = path_parse(fake_paths);
//         // Root directory always exists
//         assert_eq!(env_paths[0].as_directory().unwrap().prefix_path, PathBuf::from("/"));

//         // There should be directories for /bin and /lib and two files
//         let all_paths: Vec<PathBuf> = env_paths.iter().map(|m| {
//             match m {
//                 FuseMetadata::Directory(dir) => dir.prefix_path.clone(),
//                 FuseMetadata::File(file) => PathBuf::from(&file.basename),
//             }
//         }).collect();

//         assert!(all_paths.contains(&PathBuf::from("/bin")));
//         assert!(all_paths.contains(&PathBuf::from("/lib")));
//         assert!(all_paths.iter().any(|p| p.ends_with("python")));
//         assert!(all_paths.iter().any(|p| p.ends_with("site.py")));
//     }
// }