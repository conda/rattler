#!/usr/bin/env python3

import argparse
import asyncio
import sys
from pathlib import Path
from typing import List

from rattler.channel import Channel, ChannelConfig
from rattler.install import install
from rattler.match_spec import MatchSpec
from rattler.solver import solve


async def create_environment(prefix: Path, dependencies: List[str]) -> None:
    if prefix.exists():
        raise ValueError(f"Prefix path {prefix} already exists. Please specify a new path.")

    match_specs = [MatchSpec(dep) for dep in dependencies]
    channels = [Channel("conda-forge", ChannelConfig())]

    try:
        print("Solving dependencies...")
        records = await solve(channels, match_specs)

        print(f"Creating environment at {prefix} with dependencies: {dependencies}")
        await install(
            records=records,
            target_prefix=prefix,
        )
        print(f"Environment successfully created at {prefix}")
    except Exception as e:
        print(f"Failed to create environment: {e}", file=sys.stderr)
        sys.exit(1)

def main():
    parser = argparse.ArgumentParser(
        description="Create a Conda environment from scratch using py-rattler."
    )
    parser.add_argument(
        "--prefix",
        type=Path,
        required=True,
        help="Path where the environment will be created (must not exist)."
    )
    parser.add_argument(
        "dependencies",
        nargs="+",
        help="List of dependencies as MatchSpec strings (e.g., 'numpy>=1.20', 'python=3.11')."
    )

    args = parser.parse_args()
    asyncio.run(create_environment(args.prefix, args.dependencies))

if __name__ == "__main__":
    main()
