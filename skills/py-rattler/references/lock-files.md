# Lock Files

Read and write lock files for reproducible conda and PyPI environments across platforms.

## LockFile

Multi-environment lock file (e.g., `pixi.lock`).

### Constructor

```python
LockFile(envs: dict[str, Environment])
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `envs` | `dict[str, Environment]` | Map of environment names to `Environment` objects |

### Static Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `from_path` | `LockFile.from_path(path: os.PathLike[str]) -> LockFile` | Parse a lock file from disk |

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `to_path` | `to_path(path: os.PathLike[str]) -> None` | Write lock file to disk |
| `environments` | `environments() -> list[tuple[str, Environment]]` | All environments as `(name, env)` tuples |
| `environment` | `environment(name: str) -> Environment \| None` | Get a specific environment by name |
| `default_environment` | `default_environment() -> Environment \| None` | Get the default environment |

**Example:**

```python
from rattler import LockFile, Platform

lock = LockFile.from_path("pixi.lock")

# List environments
for name, env in lock.environments():
    print(name)

# Get default environment
env = lock.default_environment()

# Get records for a platform
records = env.conda_repodata_records_for_platform(Platform("linux-64"))
for r in records:
    print(f"{r.name} {r.version}")

# Write back
lock.to_path("pixi.lock")
```

---

## Environment

A single environment within a lock file. Contains locked packages for one or more platforms.

### Constructor

```python
Environment(
    name: str,
    requirements: dict[Platform, list[RepoDataRecord]],
    channels: list[Channel | LockChannel],
)
```

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `platforms` | `platforms() -> list[Platform]` | All platforms with locked packages |
| `channels` | `channels() -> list[LockChannel]` | Channels used (order is significant) |
| `packages` | `packages(platform: Platform) -> list[LockedPackage] \| None` | All locked packages (conda + PyPI) for a platform |
| `packages_by_platform` | `packages_by_platform() -> dict[Platform, list[LockedPackage]]` | All packages grouped by platform |
| `conda_repodata_records` | `conda_repodata_records() -> dict[Platform, list[RepoDataRecord]]` | Conda records for all platforms |
| `conda_repodata_records_for_platform` | `conda_repodata_records_for_platform(platform: Platform) -> list[RepoDataRecord] \| None` | Conda records for a specific platform |
| `pypi_packages` | `pypi_packages() -> dict[Platform, list[PypiLockedPackage]]` | PyPI packages for all platforms |
| `pypi_packages_for_platform` | `pypi_packages_for_platform(platform: Platform) -> list[PypiLockedPackage] \| None` | PyPI packages for a specific platform |

---

## LockChannel

A channel reference within a lock file.

### Constructor

```python
LockChannel(url: str)
```

`str(lock_channel)` returns the URL.

---

## LockedPackage (ABC)

Base class for any package in a lock file. Subclasses: `CondaLockedBinaryPackage`, `CondaLockedSourcePackage`, `PypiLockedPackage`.

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `name` | `str` | Package name |
| `location` | `str` | Package URL/location |
| `hashes` | `PackageHashes \| None` | File hashes |

---

## CondaLockedPackage (ABC)

Base for locked conda packages. Inherits from `LockedPackage`.

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `package_record` | `PackageRecord` | Full package metadata |
| `version` | `Version` | Package version |

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `satisfies` | `satisfies(spec: MatchSpec \| str) -> bool` | Check if this locked package satisfies a MatchSpec |

---

## CondaLockedBinaryPackage

A locked conda binary package. Inherits from `CondaLockedPackage`.

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `repo_data_record` | `repo_data_record() -> RepoDataRecord` | Returns the full `RepoDataRecord` including location info |

---

## CondaLockedSourcePackage

A locked conda source package. Inherits from `CondaLockedPackage`.

---

## PypiLockedPackage

A locked PyPI package. Inherits from `LockedPackage`.

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `version` | `str` | Package version string |
| `requires_dist` | `list[str]` | Dependencies on other packages |
| `requires_python` | `str \| None` | Python version requirement |
| `is_editable` | `bool` | Whether installed in editable mode |
| `extras` | `set[str]` | Enabled extras |

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `satisfies` | `satisfies(spec: str) -> bool` | Check if this package satisfies a requirement spec |

---

## PackageHashes

File hashes for a locked package.

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `md5` | `bytes \| None` | MD5 hash |
| `sha256` | `bytes \| None` | SHA256 hash |
