# Gateway and Repodata

## Gateway

Thread-safe manager for efficiently acquiring repodata. Handles remote fetching, local caching, and request coalescing.

### Constructor

```python
Gateway(
    cache_dir: os.PathLike[str] | None = None,
    default_config: SourceConfig | None = None,
    per_channel_config: dict[str, SourceConfig] | None = None,
    max_concurrent_requests: int = 100,
    client: Client | None = None,
    show_progress: bool = False,
)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `cache_dir` | `os.PathLike[str] \| None` | `None` | Cache directory (auto-detected if `None`) |
| `default_config` | `SourceConfig \| None` | `None` | Default source config (uses `SourceConfig()` if `None`) |
| `per_channel_config` | `dict[str, SourceConfig] \| None` | `None` | Per-URL config (longest matching prefix wins) |
| `max_concurrent_requests` | `int` | `100` | Maximum concurrent HTTP requests |
| `client` | `Client \| None` | `None` | HTTP client (default if `None`) |
| `show_progress` | `bool` | `False` | Show CLI progress bars |

### Methods

#### query()

```python
async def query(
    sources: Iterable[Channel | str | RepoDataSource],
    platforms: Iterable[Platform | PlatformLiteral],
    specs: Iterable[MatchSpec | PackageName | str],
    recursive: bool = True,
) -> list[list[RepoDataRecord]]
```

Query repodata from channels and custom sources.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `sources` | `Iterable[Channel \| str \| RepoDataSource]` | required | Channels or custom sources |
| `platforms` | `Iterable[Platform \| PlatformLiteral]` | required | Platforms to query |
| `specs` | `Iterable[MatchSpec \| PackageName \| str]` | required | Specs to search for |
| `recursive` | `bool` | `True` | Recursively fetch dependencies |

**Returns:** `list[list[RepoDataRecord]]` — outer list per source, inner list of matching records.

#### names()

```python
async def names(
    sources: Iterable[Channel | str | RepoDataSource],
    platforms: Iterable[Platform | PlatformLiteral],
) -> list[PackageName]
```

List all package names available in the given sources and platforms.

#### clear_repodata_cache()

```python
def clear_repodata_cache(
    channel: Channel | str,
    subdirs: Iterable[Platform | PlatformLiteral] | None = None,
    clear_disk: bool = False,
) -> None
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `channel` | `Channel \| str` | required | Channel to clear cache for |
| `subdirs` | `Iterable[...] \| None` | `None` | Specific subdirs to clear (`None` = all) |
| `clear_disk` | `bool` | `False` | Also clear on-disk cache (default: only in-memory) |

**Example:**

```python
from rattler import Gateway, SourceConfig, Platform

gateway = Gateway(
    cache_dir="/tmp/rattler-cache",
    default_config=SourceConfig(
        zstd_enabled=True,
        sharded_enabled=True,
    ),
    max_concurrent_requests=50,
)

# Query for specific packages
results = await gateway.query(
    sources=["conda-forge"],
    platforms=[Platform("linux-64"), Platform("noarch")],
    specs=["python", "numpy"],
)
for source_results in results:
    for record in source_results:
        print(f"{record.name} {record.version}")

# List all package names
names = await gateway.names(
    sources=["conda-forge"],
    platforms=[Platform("linux-64")],
)
```

---

## SourceConfig

Per-channel configuration for the Gateway.

```python
@dataclass
class SourceConfig:
    zstd_enabled: bool = True
    bz2_enabled: bool = True
    sharded_enabled: bool = True
    cache_action: CacheAction = "cache-or-fetch"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `zstd_enabled` | `bool` | `True` | Enable ZSTD compression for repodata |
| `bz2_enabled` | `bool` | `True` | Enable BZ2 compression for repodata |
| `sharded_enabled` | `bool` | `True` | Enable sharded repodata format |
| `cache_action` | `CacheAction` | `"cache-or-fetch"` | Cache behavior |

**CacheAction values:** `"cache-or-fetch"`, `"use-cache-only"`, `"force-cache-only"`, `"no-cache"`

---

## RepoDataSource (Protocol)

Implement this protocol to provide packages from custom backends (databases, APIs, in-memory caches). Custom sources are NOT cached by the Gateway.

```python
class RepoDataSource(Protocol):
    async def fetch_package_records(
        self, platform: Platform, name: PackageName
    ) -> list[RepoDataRecord]:
        """Fetch records for a specific package name and platform."""
        ...

    def package_names(self, platform: Platform) -> list[str]:
        """Return all available package names for the given platform."""
        ...
```

**Example:**

```python
from rattler.repo_data import RepoDataSource
from rattler import solve, PackageName, Platform, RepoDataRecord

class DatabaseSource(RepoDataSource):
    def __init__(self, db):
        self.db = db

    async def fetch_package_records(self, platform, name):
        rows = self.db.query(platform=str(platform), name=str(name))
        return [row_to_record(r) for r in rows]

    def package_names(self, platform):
        return self.db.list_names(platform=str(platform))

records = await solve([DatabaseSource(db)], specs=["my-package"])
```

---

## RepoData

In-memory representation of a `repodata.json` file.

### Constructor

```python
RepoData(path: str | os.PathLike[str])
```

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `apply_patches` | `apply_patches(instructions: PatchInstructions) -> None` | Apply a patch to the repodata |
| `into_repo_data` | `into_repo_data(channel: Channel) -> list[RepoDataRecord]` | Convert to a list of `RepoDataRecord` given the source channel |

---

## SparseRepoData

Memory-efficient, lazy-loading representation of repodata. Only loads records on demand.

### Constructor

```python
SparseRepoData(channel: Channel, subdir: str, path: os.PathLike[str] | str)
```

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `subdir` | `str` | Subdirectory this repodata was loaded from |

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `package_names` | `package_names(package_format_selection=PREFER_CONDA) -> list[str]` | All package names in the repodata |
| `record_count` | `record_count(package_format_selection=PREFER_CONDA) -> int` | Total number of packages |
| `load_records` | `load_records(package_name: str \| PackageName, ...) -> list[RepoDataRecord]` | Load records for a specific package |
| `load_all_records` | `load_all_records(package_format_selection=PREFER_CONDA) -> list[RepoDataRecord]` | Load all records |
| `load_matching_records` | `load_matching_records(specs: Iterable[MatchSpec], ...) -> list[RepoDataRecord]` | Load records matching any of the given specs |
| `load_records_recursive` | `load_records_recursive(repo_data: list[SparseRepoData], package_names: list[PackageName], ...) -> list[list[RepoDataRecord]]` | **(static)** Load records for packages and their dependencies |
| `close` | `close() -> None` | Release mapped resources |

Supports context manager (`with SparseRepoData(...) as srd:`).

---

## PackageFormatSelection

Enum controlling which package formats to use.

| Value | Description |
|-------|-------------|
| `ONLY_TAR_BZ2` | Only `.tar.bz2` packages |
| `ONLY_CONDA` | Only `.conda` packages |
| `PREFER_CONDA` | Prefer `.conda` over `.tar.bz2` |
| `PREFER_CONDA_WITH_WHL` | Prefer `.conda`, also include `.whl` if available |
| `BOTH` | Use both `.tar.bz2` and `.conda` |
