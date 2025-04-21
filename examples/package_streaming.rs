use rattler_package_streaming::read::PackageReader;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let package_path = Path::new("path/to/package.tar.bz2");
    let reader = PackageReader::from_path(package_path).await?;
    
    // Access package metadata
    println!("Package name: {}", reader.index().package_name());
    println!("Version: {}", reader.index().version());
    println!("Build string: {}", reader.index().build_string());
    
    // List files in the package
    for file in reader.index().files() {
        println!("File: {}", file);
    }
    
    Ok(())
} 