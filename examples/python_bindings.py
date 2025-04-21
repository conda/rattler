from rattler import Package, Channel, Environment

def main():
    # Create a new channel
    channel = Channel.from_url("https://conda.anaconda.org/conda-forge")

    # Get package information
    pkg = Package.from_url(channel, "numpy-1.20.0-py39h6944008_0.tar.bz2")
    print(f"Package name: {pkg.name}")
    print(f"Version: {pkg.version}")
    print(f"Build string: {pkg.build_string}")

    # Working with environments
    env = Environment.create("my-env")
    env.install(["python=3.9", "numpy>=1.20"])
    env.export_requirements("requirements.txt")

if __name__ == "__main__":
    main() 