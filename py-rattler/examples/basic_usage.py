from rattler import PackageName, Channel, Environment
from typing import List

def main() -> None:
    # Create a new channel
    channel = Channel("conda-forge")

    # Get package information
    pkg_name = PackageName("numpy")  # Using constructor directly
    print(f"Package name: {pkg_name}")
    
    # Define environment parameters
    name = "my-env"
    requirements: List[str] = ["python=3.9", "numpy>=1.20"]
    channels = [channel]
    
    # Create and configure environment
    env = Environment(
        name=name,
        requirements=requirements,
        channels=channels
    )

if __name__ == "__main__":
    main() 