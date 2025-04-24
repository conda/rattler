from rattler import PackageName, Channel, Platform

def main() -> None:  # type: ignore[no-untyped-def]
    """Basic example of using rattler."""
    # Create a package name
    pkg_name = PackageName("numpy")
    print(f"Package name: {pkg_name}")
    
    # Get current platform
    platform = Platform.current()
    print(f"Current platform: {platform}")
    
    # Create a channel
    channel = Channel("conda-forge")
    print(f"Using channel: {channel}")

if __name__ == "__main__":
    main() 