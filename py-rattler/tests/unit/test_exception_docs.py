"""Strict staleness checks for documented py-rattler exceptions (conda/rattler#1351).

Every API in ``COVERED_APIS`` documents the exceptions it raises with a
Google-style ``Raises:`` section. Five checks keep that documentation honest:

0. every registered API has a ``Raises:`` section under at least one parser,
   so a deleted block fails loudly instead of passing the rest vacuously;
A. every documented name resolves to a real exception (``rattler.exceptions``
   or a builtin);
B. every documented section still contains its pinned minimum set, so
   deletions fail loudly while additions pass;
C. every documented name is actually triggered by a ``pytest.raises`` negative
   test in this file (offline, deterministic), unless it is listed in
   ``documented_but_untriggerable`` with a reason and a Rust/Python call site;
D. every exception in ``rattler.exceptions.__all__`` has a ``docs/<snake>.md``
   page and an ``mkdocs.yml`` nav entry (deprecated aliases excluded).

New APIs must add a ``Raises:`` section *and* register in ``COVERED_APIS``.
"""

from __future__ import annotations

import builtins
import json
import re
import warnings
from collections.abc import Callable, Coroutine
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import griffe
import pytest
from griffe import DocstringSectionKind

import rattler.exceptions
from rattler import Channel, fetch_repo_data, solve
from rattler.networking.fetch_repo_data import FetchRepoDataOptions
from rattler.platform import Subdir

# Exceptions that may appear in `Raises:` sections without living in
# `rattler.exceptions`. Every entry here is raisable from the wrapped code.
BUILTIN_ALLOWLIST = frozenset(
    {
        "TypeError",
        "ValueError",
        "RuntimeError",
        "OSError",
        "IOError",
        "AttributeError",
        "ImportError",
        "NotImplementedError",
        "FileNotFoundError",
        "StopAsyncIteration",
    }
)

# `__all__` entries that intentionally have no docs page.
EXCLUDED_FROM_DOCS = frozenset({"ParsePlatformError"})


@dataclass(frozen=True)
class Untriggerable:
    """A documented exception with no deterministic offline trigger."""

    name: str
    reason: str
    call_site: str


# An async helper that raises the documented exception when awaited.
Trigger = Callable[[Path], Coroutine[Any, Any, None]]


@dataclass(frozen=True)
class APICoverage:
    """Coverage contract for one documented API."""

    pinned: frozenset[str]
    triggers: dict[str, Trigger] = field(default_factory=dict)
    documented_but_untriggerable: tuple[Untriggerable, ...] = ()


def _write_repodata(subdir_dir: Path, packages: dict[str, Any]) -> None:
    subdir_dir.mkdir(parents=True, exist_ok=True)
    doc = {
        "info": {"subdir": subdir_dir.name},
        "packages": packages,
        "packages.conda": {},
    }
    (subdir_dir / "repodata.json").write_text(json.dumps(doc))


def _package_entry(name: str, version: str = "1.0") -> dict[str, Any]:
    return {
        f"{name}-{version}-0.tar.bz2": {
            "build": "0",
            "build_number": 0,
            "depends": [],
            "name": name,
            "subdir": "noarch",
            "version": version,
        }
    }


def _local_channel(tmp_path: Path) -> Channel:
    """A `file://` channel with a single dummy package, fully offline."""
    channel_dir = tmp_path / "chan"
    _write_repodata(channel_dir / "noarch", _package_entry("mypkg"))
    return Channel(str(channel_dir))


async def _trigger_solve_unsat(tmp_path: Path) -> None:
    await solve([_local_channel(tmp_path)], ["definitely-not-a-real-pkg-xyz"], platforms=["noarch"])


async def _trigger_solve_missing_channel(tmp_path: Path) -> None:
    del tmp_path
    await solve([Channel("file:///nonexistent-dir-xyz-1351")], ["mypkg"], platforms=["noarch"])


async def _trigger_solve_bad_source(tmp_path: Path) -> None:
    del tmp_path
    await solve([123], ["mypkg"], platforms=["noarch"])  # type: ignore[list-item]


async def _trigger_solve_bad_spec(tmp_path: Path) -> None:
    await solve([_local_channel(tmp_path)], [":::"], platforms=["noarch"])


async def _trigger_solve_bad_source_str(tmp_path: Path) -> None:
    del tmp_path
    await solve(["::::"], ["mypkg"], platforms=["noarch"])


async def _trigger_solve_bad_policy(tmp_path: Path) -> None:
    await solve(
        [_local_channel(tmp_path)],
        ["mypkg"],
        platforms=["noarch"],
        timestamp_policy="bogus-policy",  # type: ignore[arg-type]
    )


async def _trigger_fetch_missing_channel(tmp_path: Path) -> None:
    await fetch_repo_data(
        channels=[Channel("file:///nonexistent-dir-xyz-1351")],
        platforms=[Subdir("noarch")],
        cache_path=tmp_path / "cache",
        callback=None,
    )


async def _trigger_fetch_cache_only_missing(tmp_path: Path) -> None:
    await fetch_repo_data(
        channels=[Channel("file:///nonexistent-dir-xyz-1351")],
        platforms=[Subdir("noarch")],
        cache_path=tmp_path / "empty-cache",
        callback=None,
        fetch_options=FetchRepoDataOptions(cache_action="use-cache-only"),
    )


COVERED_APIS: dict[str, APICoverage] = {
    "rattler.solver.solver.solve": APICoverage(
        pinned=frozenset(
            {
                "SolverError",
                "GatewayError",
                "InvalidChannelError",
                "InvalidMatchSpecError",
                "TypeError",
                "ValueError",
            }
        ),
        triggers={
            "SolverError": _trigger_solve_unsat,
            "GatewayError": _trigger_solve_missing_channel,
            "InvalidChannelError": _trigger_solve_bad_source_str,
            "InvalidMatchSpecError": _trigger_solve_bad_spec,
            "TypeError": _trigger_solve_bad_source,
            "ValueError": _trigger_solve_bad_policy,
        },
        documented_but_untriggerable=(
            Untriggerable(
                name="IoError",
                reason="only raised if the solver task is cancelled; no deterministic offline trigger",
                call_site="py-rattler/src/solver.rs:207,330",
            ),
        ),
    ),
    "rattler.networking.fetch_repo_data.fetch_repo_data": APICoverage(
        pinned=frozenset({"FetchRepoDataError"}),
        triggers={
            "FetchRepoDataError": _trigger_fetch_missing_channel,
        },
    ),
}


def _raises_names(dotted_path: str) -> set[str]:
    """Names from the ``Raises:`` section of ``dotted_path``, via griffe.

    Both parsers are tried explicitly (griffe does not auto-detect): Google
    first, then NumPy. The first parser that yields a ``Raises`` section wins,
    so a misparsed style can never pollute the result.
    """
    for parser in (griffe.Parser.google, griffe.Parser.numpy):
        obj = griffe.load(dotted_path, submodules=True, docstring_parser=parser)
        docstring = obj.docstring
        if docstring is None or docstring.parsed is None:
            continue
        for section in docstring.parsed:
            if section.kind == DocstringSectionKind.raises:
                names = {
                    str(item.annotation).strip().strip("`") for item in section.value if item.annotation is not None
                }
                if names:
                    return names
    return set()


def _has_raises_section(dotted_path: str) -> bool:
    """Whether ``dotted_path`` has a ``Raises:`` section under either parser."""
    for parser in (griffe.Parser.google, griffe.Parser.numpy):
        obj = griffe.load(dotted_path, submodules=True, docstring_parser=parser)
        docstring = obj.docstring
        if docstring is None or docstring.parsed is None:
            continue
        for section in docstring.parsed:
            if section.kind == DocstringSectionKind.raises:
                return True
    return False


def _exception_class(name: str) -> type[BaseException]:
    """Resolve a documented name to the exception class it names."""
    if name in BUILTIN_ALLOWLIST:
        resolved = getattr(builtins, name, None)
    else:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeprecationWarning)
            resolved = getattr(rattler.exceptions, name, None)
    assert isinstance(resolved, type) and issubclass(resolved, BaseException), (
        f"documented exception {name!r} does not resolve to an exception class"
    )
    return resolved


def test_covered_api_has_raises_section() -> None:
    """Every registered API has a `Raises:` section under at least one parser.

    Without this, a deleted `Raises:` block would make every other check pass
    vacuously on an empty ``documented`` set.
    """
    for dotted_path in COVERED_APIS:
        assert _has_raises_section(dotted_path), f"{dotted_path} has no Raises: section under either parser"


def test_covered_api_raises_resolve() -> None:
    """Check A: every documented name resolves to a real exception."""
    for dotted_path in COVERED_APIS:
        documented = _raises_names(dotted_path)
        assert documented, f"{dotted_path} documents no exceptions"
        for name in documented:
            _exception_class(name)


def test_covered_api_pinned_minimum() -> None:
    """Check B: the pinned minimum set is still documented (deletions fail)."""
    for dotted_path, coverage in COVERED_APIS.items():
        assert coverage.pinned, f"{dotted_path} registers an empty pinned minimum"
        documented = _raises_names(dotted_path)
        assert documented, f"{dotted_path} documents no exceptions"
        missing = coverage.pinned - documented
        assert not missing, f"{dotted_path} no longer documents its pinned minimum: {sorted(missing)}"


@pytest.mark.asyncio
async def test_covered_api_documented_names_exercised(tmp_path: Path) -> None:
    """Check C: every documented name is triggered, or explicitly allowlisted."""
    for dotted_path, coverage in COVERED_APIS.items():
        documented = _raises_names(dotted_path)
        allowlisted = {entry.name for entry in coverage.documented_but_untriggerable}
        assert len(coverage.documented_but_untriggerable) <= 2, (
            f"{dotted_path} allowlists more than 2 untriggerable exceptions"
        )
        for entry in coverage.documented_but_untriggerable:
            assert entry.reason, f"{dotted_path} allowlist entry {entry.name!r} needs a reason"
            assert entry.call_site, f"{dotted_path} allowlist entry {entry.name!r} needs a call-site citation"
        for name in sorted(documented):
            if name in coverage.triggers:
                with pytest.raises(_exception_class(name)):
                    await coverage.triggers[name](tmp_path)
            elif name in allowlisted:
                continue
            else:
                pytest.fail(
                    f"{dotted_path} documents {name!r} but no negative test triggers it "
                    "(add a trigger or an allowlist entry with reason and call site)"
                )


def _snake_case(name: str) -> str:
    snake = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", name)
    snake = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", snake)
    return snake.lower()


def _exception_nav_entries(nav_text: str) -> list[tuple[str, str]]:
    """`(name, page)` pairs from the `exceptions:` nav block, in file order."""
    lines = nav_text.splitlines()
    start = next(i for i, line in enumerate(lines) if line.strip() == "- exceptions:")
    entries = []
    for line in lines[start + 1 :]:
        match = re.fullmatch(r"\s+- ([A-Za-z_][A-Za-z0-9_]*): (\S+)", line)
        if match is None:
            break
        entries.append((match.group(1), match.group(2)))
    return entries


def test_all_exceptions_have_docs_page() -> None:
    """Check D: every exported exception has a docs page and a nav entry."""
    package_root = Path(__file__).resolve().parents[2]
    docs_dir = package_root / "docs"
    nav_text = (package_root / "mkdocs.yml").read_text()
    entries = _exception_nav_entries(nav_text)
    assert entries, "no entries found in the exceptions nav block"
    names = [name for name, _ in entries]
    assert len(names) == len(set(names)), f"duplicate names in the exceptions nav: {sorted(names)}"
    assert names == sorted(names), "exceptions nav entries are not sorted"
    expected = {name for name in rattler.exceptions.__all__ if name not in EXCLUDED_FROM_DOCS}
    assert set(names) == expected, (
        f"exceptions nav does not match rattler.exceptions.__all__: "
        f"missing={sorted(expected - set(names))} extra={sorted(set(names) - expected)}"
    )
    for name, page in entries:
        expected_page = f"{_snake_case(name)}.md"
        assert page == expected_page, f"nav entry for {name} points at {page}, expected {expected_page}"
        assert (docs_dir / page).is_file(), f"rattler.exceptions.{name} has no docs/{page}"


@pytest.mark.asyncio
async def test_solve_raises_solver_error(tmp_path: Path) -> None:
    """`solve()` reports unsatisfiable specs as `SolverError`."""
    with pytest.raises(rattler.exceptions.SolverError):
        await _trigger_solve_unsat(tmp_path)


@pytest.mark.asyncio
async def test_solve_raises_gateway_error(tmp_path: Path) -> None:
    """`solve()` reports missing channels as `GatewayError`."""
    with pytest.raises(rattler.exceptions.GatewayError):
        await _trigger_solve_missing_channel(tmp_path)


@pytest.mark.asyncio
async def test_solve_raises_invalid_match_spec_error(tmp_path: Path) -> None:
    """`solve()` reports an unparseable spec string as `InvalidMatchSpecError`."""
    with pytest.raises(rattler.exceptions.InvalidMatchSpecError):
        await _trigger_solve_bad_spec(tmp_path)


@pytest.mark.asyncio
async def test_solve_raises_invalid_channel_error(tmp_path: Path) -> None:
    """`solve()` reports an invalid channel string as `InvalidChannelError`."""
    with pytest.raises(rattler.exceptions.InvalidChannelError):
        await _trigger_solve_bad_source_str(tmp_path)


@pytest.mark.asyncio
async def test_solve_raises_type_error(tmp_path: Path) -> None:
    """`solve()` rejects sources outside the supported interface."""
    with pytest.raises(TypeError):
        await _trigger_solve_bad_source(tmp_path)


@pytest.mark.asyncio
async def test_solve_raises_value_error(tmp_path: Path) -> None:
    """`solve()` rejects an invalid option value (`timestamp_policy`)."""
    with pytest.raises(ValueError):
        await _trigger_solve_bad_policy(tmp_path)


@pytest.mark.asyncio
async def test_fetch_repo_data_raises_fetch_repo_data_error(tmp_path: Path) -> None:
    """`fetch_repo_data()` reports missing channels as `FetchRepoDataError`."""
    with pytest.raises(rattler.exceptions.FetchRepoDataError):
        await _trigger_fetch_missing_channel(tmp_path)


@pytest.mark.asyncio
async def test_fetch_repo_data_cache_only_missing_raises(tmp_path: Path) -> None:
    """`fetch_repo_data()` with `use-cache-only` fails on a missing channel."""
    with pytest.raises(rattler.exceptions.FetchRepoDataError):
        await _trigger_fetch_cache_only_missing(tmp_path)
