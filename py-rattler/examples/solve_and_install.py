import asyncio
import tempfile

from rattler import solve, install, VirtualPackage


async def main() -> None:
    # Start by solving the environment.
    #
    # Solving is the process of going from specifications of package and their
    # version requirements to a list of concrete packages.
    print("started solving the environment")
    solved_records = await solve(
        # Channels to use for solving
        channels=["conda-forge"],
        # The specs to solve for
        specs=["python ~=3.12.0", "pip", "requests 2.31.0"],
        # Virtual packages define the specifications of the environment
        virtual_packages=VirtualPackage.detect(),
    )
    print("solved required dependencies")

    # Install the packages into a new environment (or updates it if it already
    # existed).
    env_path = tempfile.mkdtemp()
    await install(
        records=solved_records,
        target_prefix=env_path,
    )

    print(f"created environment: {env_path}")


if __name__ == "__main__":
    asyncio.run(main())
