from __future__ import annotations

from typing import Dict, Optional

try:
    from rich.console import Group
    from rich.live import Live
    from rich.progress import (
        BarColumn,
        DownloadColumn,
        MofNCompleteColumn,
        Progress,
        SpinnerColumn,
        TaskID,
        TextColumn,
        TimeRemainingColumn,
        TransferSpeedColumn,
    )

    _RICH_AVAILABLE = True
except ImportError:
    _RICH_AVAILABLE = False

from rattler.install.installer import InstallerReporter


class RichInstallerReporter(InstallerReporter):
    """
    An :class:`InstallerReporter` that renders live progress bars using
    `rich <https://github.com/Textualize/rich>`_.

    Requires the ``rich`` package::

        pip install rich
        # or, using the extras declared by py-rattler:
        pip install py-rattler[rich]

    The display consists of three stacked progress panels:

    * **Overall** — a single progress bar tracking completed / total packages with ETA.
    * **Downloads** — one transient bar per active download with bytes, speed, and ETA.
    * **Links / Unlinks** — one transient spinner row per active link or unlink operation.

    Example
    -------
    ```python
    from rattler import solve, install
    from rattler.install import RichInstallerReporter
    import asyncio

    async def main():
        records = await solve(["conda-forge"], ["python 3.11.*"])
        await install(records, "/tmp/myenv", reporter=RichInstallerReporter())

    asyncio.run(main())
    ```
    """

    def __init__(self) -> None:
        if not _RICH_AVAILABLE:
            raise ImportError(
                "The 'rich' package is required to use RichInstallerReporter.\n"
                "Install it with:  pip install rich\n"
                "or:               pip install py-rattler[rich]"
            )

        self._overall_progress: Progress = Progress(
            SpinnerColumn(),
            TextColumn("[bold green]{task.description}"),
            BarColumn(),
            MofNCompleteColumn(),
            TimeRemainingColumn(),
        )

        self._download_progress: Progress = Progress(
            SpinnerColumn(),
            TextColumn("[cyan]{task.description:<40}"),
            BarColumn(),
            DownloadColumn(),
            TransferSpeedColumn(),
            TimeRemainingColumn(),
        )

        self._link_progress: Progress = Progress(
            SpinnerColumn(),
            TextColumn("{task.description}"),
        )

        self._live: Live = Live(
            Group(
                self._overall_progress,
                self._download_progress,
                self._link_progress,
            ),
            refresh_per_second=10,
        )

        self._overall_task: Optional[TaskID] = None
        self._total_operations: int = 0
        self._completed_operations: int = 0
        self._cache_entry_names: Dict[int, str] = {}
        self._download_tasks: Dict[int, TaskID] = {}
        self._link_tasks: Dict[int, TaskID] = {}

    def on_transaction_start(self, total_operations: int) -> None:
        self._total_operations = total_operations
        self._live.start()
        self._overall_task = self._overall_progress.add_task(
            f"Installing {total_operations} package{'s' if total_operations != 1 else ''}",
            total=total_operations,
        )

    def on_transaction_complete(self) -> None:
        if self._overall_task is not None:
            self._overall_progress.update(
                self._overall_task,
                description="[bold green]✓ Installation complete",
                completed=self._total_operations,
            )
        self._live.stop()

    def on_populate_cache_start(self, operation: int, package_name: str) -> int:
        self._cache_entry_names[operation] = package_name
        return operation

    def on_populate_cache_complete(self, cache_entry: int) -> None:
        self._cache_entry_names.pop(cache_entry, None)

    def on_download_start(self, cache_entry: int) -> int:
        package_name = self._cache_entry_names.get(cache_entry, "unknown")
        task_id = self._download_progress.add_task(
            f"Downloading {package_name}",
            total=None,
            start=True,
        )
        download_idx = int(task_id)
        self._download_tasks[download_idx] = task_id
        return download_idx

    def on_download_progress(self, download_idx: int, progress: int, total: Optional[int]) -> None:
        task_id = self._download_tasks.get(download_idx)
        if task_id is not None:
            self._download_progress.update(task_id, completed=progress, total=total)

    def on_download_completed(self, download_idx: int) -> None:
        task_id = self._download_tasks.pop(download_idx, None)
        if task_id is not None:
            self._download_progress.remove_task(task_id)

    def on_link_start(self, operation: int, package_name: str) -> int:
        task_id = self._link_progress.add_task(
            f"[blue]Linking   [bold]{package_name}",
            total=None,
        )
        link_idx = int(task_id)
        self._link_tasks[link_idx] = task_id
        return link_idx

    def on_link_complete(self, index: int) -> None:
        task_id = self._link_tasks.pop(index, None)
        if task_id is not None:
            self._link_progress.remove_task(task_id)
        self._completed_operations += 1
        if self._overall_task is not None:
            self._overall_progress.update(self._overall_task, advance=1)

    def on_unlink_start(self, operation: int, package_name: str) -> int:
        task_id = self._link_progress.add_task(
            f"[yellow]Removing  [bold]{package_name}",
            total=None,
        )
        unlink_idx = int(task_id)
        self._link_tasks[unlink_idx] = task_id
        return unlink_idx

    def on_unlink_complete(self, index: int) -> None:
        task_id = self._link_tasks.pop(index, None)
        if task_id is not None:
            self._link_progress.remove_task(task_id)
        self._completed_operations += 1
        if self._overall_task is not None:
            self._overall_progress.update(self._overall_task, advance=1)
