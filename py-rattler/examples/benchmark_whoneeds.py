"""Reproduce consecutive all-platform conda-forge who-needs queries.

Run with:
    pixi run python scripts/reproduce_whoneeds.py

Pass ``--pause`` to keep the process open after each query for inspection in
tools such as btop.
"""

from __future__ import annotations

import argparse
import asyncio
import gc
import os
import resource
import sys
from time import perf_counter
from typing import cast

from rattler.networking import Client
from rattler.networking.fetch_repo_data import CacheAction
from rattler.platform import Platform
from rattler.platform.platform import PlatformName
from rattler.repo_data import Dependent, Gateway, SourceConfig

CONDA_FORGE_PLATFORMS = [
    Platform(PlatformName.LINUX_64),
    Platform(PlatformName.LINUX_AARCH64),
    Platform(PlatformName.LINUX_ARMV7L),
    Platform(PlatformName.LINUX_PPC64LE),
    Platform(PlatformName.LINUX_RISCV64),
    Platform(PlatformName.LINUX_S390X),
    Platform(PlatformName.OSX_64),
    Platform(PlatformName.OSX_ARM64),
    Platform(PlatformName.WIN_32),
    Platform(PlatformName.WIN_64),
    Platform(PlatformName.WIN_ARM64),
    Platform(PlatformName.NOARCH),
]
TARGETS = ("python", "polars")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run consecutive python and polars who-needs queries.")
    parser.add_argument(
        "--pause",
        action="store_true",
        help="wait for Enter after each query so process memory can be inspected",
    )
    return parser.parse_args()


def format_bytes(size: int) -> str:
    value = float(size)
    for unit in ("B", "KiB", "MiB", "GiB"):
        if value < 1024 or unit == "GiB":
            return f"{value:.1f} {unit}"
        value /= 1024
    raise AssertionError("unreachable")


def peak_rss_bytes() -> int:
    peak_rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    # macOS reports bytes; Linux and other Unix platforms report KiB.
    return int(peak_rss if sys.platform == "darwin" else peak_rss * 1024)


async def main() -> None:
    pause = cast(bool, parse_args().pause)
    print(f"PID: {os.getpid()}")
    print("Channel: conda-forge")
    print("Platforms: " + ", ".join(str(platform) for platform in CONDA_FORGE_PLATFORMS))

    gateway = Gateway(
        default_config=SourceConfig(
            cache_action=CacheAction.CACHE_OR_FETCH,
        ),
        client=Client.default_client(user_agent="pixi-browse-whoneeds-reproducer"),
        show_progress=False,
    )

    current_result: list[Dependent] = []
    for target in TARGETS:
        if current_result:
            print(f"\nKeeping {len(current_result):,} results from the previous view while querying {target!r}...")
        else:
            print(f"\nQuerying {target!r}...")

        started = perf_counter()
        next_result = await gateway.who_needs(
            sources=["conda-forge"],
            platforms=CONDA_FORGE_PLATFORMS,
            target=target,
        )
        elapsed = perf_counter() - started

        # Replacing the current result mirrors the UI after the new query finishes.
        current_result = next_result
        del next_result
        gc.collect()

        package_count = len({dependent.record.name.normalized for dependent in current_result})
        print(
            f"Finished {target!r} in {elapsed:.3f}s: {len(current_result):,} records across {package_count:,} packages"
        )
        print(f"Peak RSS: {format_bytes(peak_rss_bytes())}")

        if pause:
            input("Press Enter to continue...")

    if pause:
        input("Final result is still retained; press Enter to exit...")


if __name__ == "__main__":
    asyncio.run(main())
