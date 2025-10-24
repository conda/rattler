use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[clap(
    name = "upgrade-lock-file",
    version,
    about = "Upgrade rattler lock files to the latest format version"
)]
struct Args {
    /// Path to the lock file to upgrade
    #[clap(value_name = "FILE")]
    input: PathBuf,

    /// Output path (defaults to overwriting the input file)
    #[clap(short, long, value_name = "FILE")]
    output: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Read the lock file
    let lock_file = rattler_lock::LockFile::from_path(&args.input)
        .with_context(|| format!("Failed to parse lock file: {}", args.input.display()))?;

    // Get the current version
    let current_version = lock_file.version();
    let latest_version = rattler_lock::FileFormatVersion::LATEST;

    println!("Current version: {current_version}, Latest version: {latest_version}",);

    if current_version == latest_version {
        println!("Lock file is already at the latest version (v{latest_version})",);
        if args.output.is_none() {
            return Ok(());
        }
    }

    // Determine output path
    let output = args.output.as_ref().unwrap_or(&args.input);

    // Serialize to the latest version (happens automatically)
    lock_file
        .to_path(output)
        .with_context(|| format!("Failed to write lock file: {}", output.display()))?;

    println!(
        "Successfully upgraded lock file from v{current_version} to v{latest_version}: {}",
        output.display()
    );

    Ok(())
}
