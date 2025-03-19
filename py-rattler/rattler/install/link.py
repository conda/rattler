from __future__ import annotations
import os
from typing import List, Optional, Union

from rattler.platform.platform import Platform
from rattler.prefix.prefix_record import PrefixRecord
from rattler.rattler import py_link_package

async def link_package(
    package_dir: str | os.PathLike[str],
    target_dir: str | os.PathLike[str],
    python_info_version: Optional[str] = None,
    python_info_implementation: Optional[str] = None,
    platform: Optional[Platform] = None,
    io_concurrency_limit: Optional[int] = None,
    prefix_records: Optional[List[PrefixRecord]] = None,
    execute_link_scripts: bool = False,
) -> bool:
    """
    Links a package into a prefix at a low-level.
    
    This function provides direct access to the underlying link_package API which 
    links package contents from an extracted package directory to a target prefix.
    
    Arguments:
        package_dir: Path to the extracted package directory
        target_dir: Path to the environment prefix
        python_info_version: Optional Python version for noarch packages (e.g. "3.9.0")
        python_info_implementation: Optional Python implementation for noarch packages (e.g. "cpython")
        platform: Target platform for linking
        io_concurrency_limit: Optional limit for concurrent IO operations
        prefix_records: Optional list of prefix records in the environment
        execute_link_scripts: Whether to execute pre/post link scripts
        
    Returns:
        True if the package was successfully linked
    """
    return await py_link_package(
        package_dir=str(package_dir),
        target_dir=str(target_dir),
        python_info_version=python_info_version,
        python_info_implementation=python_info_implementation,
        platform=platform._inner if platform is not None else None,
        io_concurrency_limit=io_concurrency_limit,
        prefix_records=prefix_records,
        execute_link_scripts=execute_link_scripts,
    )