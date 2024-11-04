#!/usr/bin/env -S pixi exec --spec py-rattler --spec typer -- python

import asyncio
from pathlib import Path
from typing import get_args

from rattler import install as rattler_install
from rattler import LockFile, Platform
from rattler.platform.platform import PlatformLiteral
from rattler.networking import Client, MirrorMiddleware, AuthenticationMiddleware
import typer


app = typer.Typer()


async def _install(
    lock_file_path: Path,
    environment_name: str,
    platform: Platform,
    target_prefix: Path,
) -> None:
    lock_file = LockFile.from_path(lock_file_path)
    environment = lock_file.environment(environment_name)
    if environment is None:
        raise ValueError(f"Environment {environment_name} not found in lock file {lock_file_path}")
    records = environment.conda_repodata_records_for_platform(platform)
    if not records:
        raise ValueError(f"No records found for platform {platform} in lock file {lock_file_path}")
    await rattler_install(
        records=records,
        target_prefix=target_prefix,
        client=Client(
            middlewares=[
                MirrorMiddleware({"https://conda.anaconda.org/conda-forge": ["https://repo.prefix.dev/conda-forge"]}),
                AuthenticationMiddleware(),
            ]
        ),
    )


@app.command()
def install(
    lock_file_path: Path = Path("pixi.lock").absolute(),
    environment_name: str = "default",
    platform: str = str(Platform.current()),
    target_prefix: Path = Path("env").absolute(),
) -> None:
    """
    Installs a pixi.lock file to a custom prefix.
    """
    if platform not in get_args(PlatformLiteral):
        raise ValueError(f"Invalid platform {platform}. Must be one of {get_args(PlatformLiteral)}")
    asyncio.run(
        _install(
            lock_file_path=lock_file_path,
            environment_name=environment_name,
            platform=Platform(platform),  # type: ignore[arg-type]
            target_prefix=target_prefix,
        )
    )


if __name__ == "__main__":
    app()
