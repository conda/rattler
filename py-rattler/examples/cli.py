#!/usr/bin/env python3

import argparse
import asyncio
import sys
from pathlib import Path
from typing import List, Optional

from rattler.channel import Channel, ChannelConfig
from rattler.install import install
from rattler.lock import LockFile, LockChannel, LockPlatform
from rattler.match_spec import MatchSpec
from rattler.platform import Platform, PlatformLiteral
from rattler.solver import solve


async def create_environment(
    prefix: Path,
    dependencies: List[str],
    channel_strs: List[str],
    platform_str: Optional[PlatformLiteral],
    lockfile: Optional[Path],
) -> None:
    if prefix.exists():
        raise ValueError(f"Prefix path {prefix} already exists. Please specify a new path.")

    match_specs = [MatchSpec(dep) for dep in dependencies]
    channels = [Channel(channel, ChannelConfig()) for channel in channel_strs]
    selected_platform = Platform(platform_str) if platform_str else Platform.current()
    platforms = [Platform("noarch"), selected_platform]

    try:
        print("Solving dependencies...")
        records = await solve(channels, match_specs, platforms=platforms)

        print(f"Creating environment at {prefix} with dependencies: {dependencies}")
        await install(
            records=records,
            target_prefix=prefix,
        )

        if lockfile:
            lock_platform = LockPlatform(str(selected_platform))
            lock = LockFile([lock_platform])
            lock.set_channels(
                prefix.name,
                [LockChannel(chan.base_url) for chan in channels],
            )
            for record in records:
                lock.add_conda_package(prefix.name, lock_platform, record)
            lock.to_path(lockfile)
            print(f"Lockfile saved to {lockfile}")

        print(f"Environment successfully created at {prefix}")
    except Exception as e:
        print(f"Failed to create environment: {e}", file=sys.stderr)
        sys.exit(1)


"""
Example usage:
python3 -m examples.cli --prefix ~/Downloads/test python=3.12 flask  --lockfile ~/Downloads/test.lock --channel conda-forge
"""


def main() -> None:
    parser = argparse.ArgumentParser(description="Create a Conda environment from scratch using py-rattler.")
    parser.add_argument("--prefix", type=Path, required=True, help="Environment path.")
    parser.add_argument("dependencies", nargs="+", help="Dependencies (e.g., 'python=3.11').")
    parser.add_argument("--platform", help="Target platform (e.g., 'linux-64').")
    parser.add_argument("--channel", action="append", default=["conda-forge"], help="Channels to use.")
    parser.add_argument("--lockfile", type=Path, help="Save lock file to path.")
    args = parser.parse_args()
    asyncio.run(create_environment(args.prefix, args.dependencies, args.channel, args.platform, args.lockfile))


if __name__ == "__main__":
    main()
