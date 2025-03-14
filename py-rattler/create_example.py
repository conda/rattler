#!/usr/bin/env python
import asyncio
import shutil
from pathlib import Path
from tempfile import gettempdir, TemporaryDirectory

from rattler import solve, install
from rattler.virtual_package import VirtualPackage


async def main(base_dir: Path, specs: list[str], cache_dir: Path):
    original_prefix = base_dir / "env-materializedxx"
    if original_prefix.exists():
        shutil.rmtree(original_prefix)
    dematerialized_prefix = base_dir / "env-dematerialized"
    if dematerialized_prefix.exists():
        shutil.rmtree(dematerialized_prefix)

    print("Solving environment:", specs)
    records = await solve(channels=["conda-forge"], specs=specs, virtual_packages=VirtualPackage.detect())
    print("Will install", len(records), "packages")

    print("Populating caches")
    with TemporaryDirectory() as tmpdir:
        await install(records, target_prefix=Path(tmpdir) / "env", is_overlay=False, show_progress=False, cache_dir=cache_dir)

    print("Creating conventional environment")
    await install(records, target_prefix=original_prefix, is_overlay=False, show_progress=True, cache_dir=cache_dir)

    print("Creating dematerialized environment")
    await install(records, target_prefix=dematerialized_prefix, is_overlay=True, show_progress=True, cache_dir=cache_dir)


def parse_args():
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument("--cache-dir", type=Path, default=Path(gettempdir()) / "rattler-cache")
    parser.add_argument("--base-dir", type=Path, required=True)
    parser.add_argument("specs", nargs="+")
    args = parser.parse_args()

    asyncio.run(main(args.base_dir, args.specs, args.cache_dir))


if __name__ == "__main__":
    parse_args()
