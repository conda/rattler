from rattler import PackageName, Channel, Environment
from typing import NoReturn

def main() -> None:
    # Create a new channel
    channel = Channel("conda-forge")  # Using constructor instead of from_url

    # Get package information
    pkg_name = PackageName.from_str("numpy")  # Using PackageName instead of Package
    print(f"Package name: {pkg_name}")
    
    # Create environment and install packages
    env = Environment()  # Using constructor instead of create
    env.create("my-env")  # Create the environment
    env.install(["python=3.9", "numpy>=1.20"])
    env.export_requirements("requirements.txt")

if __name__ == "__main__":
    main() 