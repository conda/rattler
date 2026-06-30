use anyhow::{Context, Result};
use clap::{Arg, ArgAction, Command};
use rattler_cache::{PACKAGE_CACHE_DIR, default_cache_dir};
use std::path::PathBuf;

use rattler_vfs::{MountBackend, mount_environment};

#[compio::main]
async fn main() -> Result<()> {
    let args = handle_input_arguments()?;

    println!("Mounting environment...");
    println!("  lock: {:?}", args.pixi_lock);
    println!("  cache: {:?}", args.cache_origin);
    println!("  mount: {:?}", args.mount_dir);
    println!("  backend: {:?}", args.mount_type);

    let session = mount_environment(
        args.pixi_lock,
        args.cache_origin,
        args.mount_dir.clone(),
        args.mount_type,
        args.environment_name,
        args.download,
    )
    .await?;

    println!("Environment mounted at {}", args.mount_dir.display());

    println!("Press Ctrl+C to unmount.");

    compio::signal::ctrl_c().await?;

    println!("Unmounting...");

    session.unmount()?;

    println!("Done.");

    Ok(())
}

pub struct MountArgs {
    pub pixi_lock: PathBuf,
    pub cache_origin: PathBuf,
    pub mount_dir: PathBuf,
    pub mount_type: MountBackend,
    pub environment_name: String,
    pub download: bool,
}

fn handle_input_arguments() -> anyhow::Result<MountArgs> {
    let matches = Command::new("mount")
        .arg(
            Arg::new("PIXI_LOCK")
                .long("PIXI_LOCK")
                .required(true)
                .value_parser(clap::value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("CACHE_ORIGIN")
                .long("CACHE_ORIGIN")
                .value_parser(clap::value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("MOUNT_DIR")
                .long("MOUNT_DIR")
                .value_parser(clap::value_parser!(PathBuf)),
        )
        .arg(
            Arg::new("MOUNT_TYPE")
                .long("MOUNT_TYPE")
                .default_value("nfs"),
        )
        .arg(
            Arg::new("ENVIRONMENT")
                .long("ENVIRONMENT")
                .default_value("default"),
        )
        .arg(
            Arg::new("DOWNLOAD")
                .long("DOWNLOAD")
                .help("Download any packages that are not already in the cache via rattler")
                .action(ArgAction::SetTrue),
        )
        .get_matches();

    let pixi_lock = matches.get_one::<PathBuf>("PIXI_LOCK").unwrap().clone();

    let cache_origin = matches
        .get_one::<PathBuf>("CACHE_ORIGIN")
        .cloned()
        .unwrap_or_else(|| {
            default_cache_dir().map_or_else(
                |_| PathBuf::from(PACKAGE_CACHE_DIR),
                |cache_dir| cache_dir.join(PACKAGE_CACHE_DIR),
            )
        });

    let mount_type = MountBackend::from(
        matches
            .get_one::<String>("MOUNT_TYPE")
            .map_or("nfs", String::as_str),
    );

    let environment_name = matches.get_one::<String>("ENVIRONMENT").unwrap().clone();

    let download = matches.get_flag("DOWNLOAD");

    let mount_dir = matches
        .get_one::<PathBuf>("MOUNT_DIR")
        .cloned()
        .unwrap_or_else(|| {
            pixi_lock
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join(".pixi")
                .join("envs")
                .join(&environment_name)
        });

    std::fs::create_dir_all(&mount_dir)
        .with_context(|| format!("failed to create mount directory {}", mount_dir.display()))?;

    let mount_dir = mount_dir.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize mount directory {}",
            mount_dir.display()
        )
    })?;

    Ok(MountArgs {
        pixi_lock,
        cache_origin,
        mount_dir,
        mount_type,
        environment_name,
        download,
    })
}
