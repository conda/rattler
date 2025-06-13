use crate::filesystem::RattlerFS;
use clap::{Arg, ArgAction, Command};
use fuser::MountOption;
use rattler_cache::default_cache_dir;
use rattler_lock::DEFAULT_ENVIRONMENT_NAME;
use std::path::PathBuf;
use std::vec;
use tree::EnvTree;

mod filesystem;
mod patching;
mod tree;
mod tree_objects;

#[tokio::main]
async fn main() {
    let matches = Command::new("rattler_fuse_test")
        .arg(
            Arg::new("lock_file")
                .required(true)
                .index(1)
                .help("Path to the pixi.lock file"),
        )
        .arg(
            Arg::new("mount_point")
                .required(true)
                .index(2)
                .help("Act as a client, and mount FUSE at given path"),
        )
        .arg(
            Arg::new("env_name")
                .long("env-name")
                .default_value(DEFAULT_ENVIRONMENT_NAME)
                .help("Name of the environment to mount"),
        )
        .arg(
            Arg::new("print-tree")
                .long("print-tree")
                .action(ArgAction::SetTrue)
                .help("Print the tree before mounting"),
        )
        .get_matches();

    let lock_file_path = PathBuf::from(matches.get_one::<String>("lock_file").unwrap());
    let cache_dir = default_cache_dir()
        .unwrap()
        .join(rattler_cache::PACKAGE_CACHE_DIR);
    let env_name = matches.get_one::<String>("env_name").unwrap();
    let target_dir = PathBuf::from(matches.get_one::<String>("mount_point").unwrap());
    let tree = EnvTree::from_lock_file(&lock_file_path, env_name, &target_dir, &cache_dir)
        .await
        .unwrap();
    if matches.get_flag("print-tree") {
        tree.print_tree();
    }

    println!(
        "Mounting {} from {} at {} with cache dir {}",
        env_name,
        lock_file_path.display(),
        target_dir.display(),
        cache_dir.display()
    );
    let options = vec![
        MountOption::RO,
        MountOption::FSName("rattlerfs".to_string()),
        MountOption::AllowOther,
        MountOption::AutoUnmount,
    ];
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    fuser::mount2(RattlerFS::new(tree, uid, gid), &target_dir, &options).unwrap();
}
