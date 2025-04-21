use rattler::install_into_prefix;
use rattler_conda_types::{Channel, PackageRecord};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure channels
    let channels = vec![
        Channel::from_str("conda-forge")?,
        Channel::from_str("defaults")?,
    ];

    // Specify packages to install
    let packages = vec!["python=3.9", "numpy>=1.20"];
    
    // Install into prefix
    install_into_prefix(
        &packages,
        &channels,
        &PathBuf::from(".prefix"),
        None,
    ).await?;
    
    Ok(())
} 