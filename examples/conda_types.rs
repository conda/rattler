use rattler_conda_types::{PackageName, VersionSpec, Platform};

fn main() {
    // Working with package names
    let pkg_name = PackageName::new("numpy").unwrap();
    let is_valid = pkg_name.is_valid(); // true
    println!("Package name is valid: {}", is_valid);

    // Version specifications
    let version_spec = VersionSpec::parse(">=1.20,<2.0").unwrap();
    let version_matches = version_spec.matches(&"1.21.0".parse().unwrap()); // true
    println!("Version matches: {}", version_matches);

    // Platform handling
    let platform = Platform::current(); // Gets current platform
    println!("Current platform: {}", platform);
} 