# Solving and Installing

## solve()

Resolve package dependencies and return the records that should be present in the environment.

```python
async def solve(
    sources: Sequence[Channel | str | RepoDataSource],
    specs: Sequence[MatchSpec | str],
    gateway: Gateway = Gateway(),
    platforms: Sequence[Platform | PlatformLiteral] | None = None,
    locked_packages: Sequence[RepoDataRecord] | None = None,
    pinned_packages: Sequence[RepoDataRecord] | None = None,
    virtual_packages: Sequence[GenericVirtualPackage | VirtualPackage] | None = None,
    timeout: datetime.timedelta | None = None,
    channel_priority: ChannelPriority = ChannelPriority.Strict,
    exclude_newer: datetime.datetime | None = None,
    strategy: SolveStrategy = "highest",
    constraints: Sequence[MatchSpec | str] | None = None,
) -> list[RepoDataRecord]
```

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `sources` | `Sequence[Channel \| str \| RepoDataSource]` | required | Channels (by name, URL, or `Channel` object) or custom `RepoDataSource` implementations |
| `specs` | `Sequence[MatchSpec \| str]` | required | Package specifications to solve |
| `gateway` | `Gateway` | `Gateway()` | Gateway for acquiring repodata |
| `platforms` | `Sequence[Platform \| PlatformLiteral] \| None` | `None` | Target platforms. If `None`, uses current platform + noarch |
| `locked_packages` | `Sequence[RepoDataRecord] \| None` | `None` | Previously selected records. Solver prefers these to reduce updates |
| `pinned_packages` | `Sequence[RepoDataRecord] \| None` | `None` | Records that **cannot** be changed. Solver always selects these exact variants |
| `virtual_packages` | `Sequence[GenericVirtualPackage \| VirtualPackage] \| None` | `None` | Virtual packages considered active (e.g., `__linux`, `__cuda`) |
| `timeout` | `datetime.timedelta \| None` | `None` | Maximum time the solver is allowed to run |
| `channel_priority` | `ChannelPriority` | `Strict` | `Strict`: use first channel where a package is found. `Disabled`: search all channels |
| `exclude_newer` | `datetime.datetime \| None` | `None` | Ignore packages published after this datetime |
| `strategy` | `SolveStrategy` | `"highest"` | Version selection strategy (see below) |
| `constraints` | `Sequence[MatchSpec \| str] \| None` | `None` | Additional constraints. Packages in constraints are not necessarily installed but if present must satisfy these |

**Returns:** `list[RepoDataRecord]` — the resolved set of packages.

**SolveStrategy values:**

| Value | Description |
|-------|-------------|
| `"highest"` | Select the highest compatible version of all packages |
| `"lowest"` | Select the lowest compatible version of all packages |
| `"lowest-direct"` | Lowest for direct dependencies, highest for transitive |

**Example:**

```python
import asyncio
import datetime
from rattler import solve, ChannelPriority, Platform, VirtualPackage

async def main():
    records = await solve(
        sources=["conda-forge"],
        specs=["python 3.12.*", "numpy >=1.26"],
        platforms=[Platform("linux-64")],
        virtual_packages=VirtualPackage.detect(),
        channel_priority=ChannelPriority.Strict,
        strategy="highest",
        timeout=datetime.timedelta(seconds=60),
    )
    for r in records:
        print(f"{r.name} {r.version}")

asyncio.run(main())
```

---

## solve_with_sparse_repodata()

Like `solve()` but takes manually-provided `SparseRepoData` instead of using a `Gateway`.

```python
async def solve_with_sparse_repodata(
    specs: Sequence[MatchSpec | str],
    sparse_repodata: Sequence[SparseRepoData],
    locked_packages: Sequence[RepoDataRecord] | None = None,
    pinned_packages: Sequence[RepoDataRecord] | None = None,
    virtual_packages: Sequence[GenericVirtualPackage | VirtualPackage] | None = None,
    timeout: datetime.timedelta | None = None,
    channel_priority: ChannelPriority = ChannelPriority.Strict,
    exclude_newer: datetime.datetime | None = None,
    strategy: SolveStrategy = "highest",
    constraints: Sequence[MatchSpec | str] | None = None,
    package_format_selection: PackageFormatSelection = PackageFormatSelection.PREFER_CONDA,
) -> list[RepoDataRecord]
```

**Additional parameters vs `solve()`:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `sparse_repodata` | `Sequence[SparseRepoData]` | required | Manually provided repodata to query |
| `package_format_selection` | `PackageFormatSelection` | `PREFER_CONDA` | Which package formats to select |

---

## install()

Download and link packages into a target environment.

```python
async def install(
    records: list[RepoDataRecord],
    target_prefix: str | os.PathLike[str],
    cache_dir: os.PathLike[str] | None = None,
    installed_packages: list[PrefixRecord] | None = None,
    reinstall_packages: set[str] | None = None,
    ignored_packages: set[str] | None = None,
    platform: Platform | None = None,
    execute_link_scripts: bool = False,
    show_progress: bool = True,
    client: Client | None = None,
    requested_specs: list[MatchSpec] | None = None,
) -> None
```

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `records` | `list[RepoDataRecord]` | required | Solved records from `solve()` |
| `target_prefix` | `str \| os.PathLike[str]` | required | Directory where the environment is created |
| `cache_dir` | `os.PathLike[str] \| None` | `None` | Download cache directory. Auto-detected if `None` |
| `installed_packages` | `list[PrefixRecord] \| None` | `None` | Already-installed packages. Auto-scanned from `target_prefix/conda-meta/` if `None` |
| `reinstall_packages` | `set[str] \| None` | `None` | Package names to force-reinstall |
| `ignored_packages` | `set[str] \| None` | `None` | Package names to leave untouched (not removed, installed, or updated) |
| `platform` | `Platform \| None` | `None` | Target platform. Defaults to current platform |
| `execute_link_scripts` | `bool` | `False` | Run post-link/pre-unlink scripts. **WARNING**: not sandboxed, can execute arbitrary code |
| `show_progress` | `bool` | `True` | Show CLI progress bar |
| `client` | `Client \| None` | `None` | HTTP client for downloading. Default client used if `None` |
| `requested_specs` | `list[MatchSpec] \| None` | `None` | Original specs to record in `conda-meta/*.json` |

**Returns:** `None`

**Example:**

```python
from rattler import solve, install, Client

records = await solve(sources=["conda-forge"], specs=["python 3.12.*"])
client = Client.default_client()
await install(
    records,
    target_prefix="/opt/envs/myenv",
    client=client,
    show_progress=True,
    requested_specs=[MatchSpec("python 3.12.*")],
)
```
