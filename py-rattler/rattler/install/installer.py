from __future__ import annotations
import os
from typing import List, Optional

from rattler.networking.client import Client
from rattler.platform.platform import Platform
from rattler.prefix.prefix_record import PrefixRecord
from rattler.repo_data.record import RepoDataRecord

from rattler.rattler import py_install


async def install(
    records: List[RepoDataRecord],
    target_prefix: str | os.PathLike[str],
    cache_dir: Optional[os.PathLike[str]] = None,
    installed_packages: Optional[List[PrefixRecord]] = None,
    platform: Optional[Platform] = None,
    execute_link_scripts: bool = False,
    show_progress: bool = True,
    client: Optional[Client] = None,
) -> None:
    """
    Create an environment by downloading and linking the `dependencies` in
    the `target_prefix` directory.

    !!! warning

        When `execute_link_scripts` is set to `True` the post-link and pre-unlink scripts of
        packages will be executed. These scripts are not sandboxed and can be used to execute
        arbitrary code. It is therefor discouraged to enable executing link scripts.

    Example
    -------
    ```python
    >>> import asyncio
    >>> from rattler import solve, install
    >>> from tempfile import TemporaryDirectory
    >>> temp_dir = TemporaryDirectory()
    >>>
    >>> async def main():
    ...     # Solve an environment with python 3.9 for the current platform
    ...     records = await solve(channels=["conda-forge"], specs=["python 3.9.*"])
    ...
    ...     # Link the environment in a temporary directory (you can pass any kind of path here).
    ...     await install(records, target_prefix=temp_dir.name)
    ...
    ...     # That's it! The environment is now created.
    ...     # You will find Python under `f"{temp_dir.name}/bin/python"` or `f"{temp_dir.name}/python.exe"` on Windows.
    >>> asyncio.run(main())

    ```

    Arguments:
        records: A list of solved `RepoDataRecord`s.
        target_prefix: Path to the directory where the environment should
                be created.
        cache_dir: Path to directory where the dependencies will be
                downloaded and cached.
        installed_packages: A list of `PrefixRecord`s which are
                already installed in the `target_prefix`. This can be obtained by loading
                `PrefixRecord`s from `{target_prefix}/conda-meta/`.
                If `None` is specified then the `target_prefix` will be scanned for installed
                packages.
        platform: Target platform to create and link the
                environment. Defaults to current platform.
        execute_link_scripts: whether to execute the post-link and pre-unlink scripts
                that may be part of a package. Defaults to False.
        show_progress: If set to `True` a progress bar will be shown on the CLI.
        client: An authenticated client to use for downloading packages. If not specified a default
                client will be used.
    """

    await py_install(
        records=records,
        target_prefix=str(target_prefix),
        cache_dir=cache_dir,
        installed_packages=installed_packages,
        platform=platform._inner if platform is not None else None,
        client=client._client if client is not None else None,
        execute_link_scripts=execute_link_scripts,
        show_progress=show_progress,
    )
