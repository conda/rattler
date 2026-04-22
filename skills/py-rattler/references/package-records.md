# Package Records

## PackageRecord

A single record in conda repodata. Refers to a single binary distribution of a package.

### Constructor

```python
PackageRecord(
    name: str | PackageName,
    version: str | VersionWithSource,
    build: str,
    build_number: int,
    subdir: str | Platform,
    arch: str | None = None,
    platform: str | None = None,
    noarch: NoArchType | NoArchLiteral | None = None,
    depends: list[str] | None = None,
    constrains: list[str] | None = None,
    sha256: bytes | None = None,
    md5: bytes | None = None,
    size: int | None = None,
    features: list[str] | None = None,
    legacy_bz2_md5: bytes | None = None,
    legacy_bz2_size: int | None = None,
    license: str | None = None,
    license_family: str | None = None,
    python_site_packages_path: str | None = None,
)
```

### Static Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `from_index_json` | `from_index_json(path, size=None, sha256=None, md5=None) -> PackageRecord` | Build from an `index.json` file |
| `sort_topologically` | `sort_topologically(records: list[PackageRecord]) -> list[PackageRecord]` | Sort records in dependency order (deterministic) |
| `to_graph` | `to_graph(records: list[PackageRecord]) -> nx.DiGraph` | Convert to a directed acyclic graph (requires networkx). Skips virtual packages |
| `validate` | `validate(records: list[PackageRecord]) -> None` | Validate that records are consistent w.r.t. `depends` and `constrains`. Raises `ValidatePackageRecordsError` |

### Instance Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `matches` | `matches(spec: MatchSpec) -> bool` | Check if this record satisfies the given MatchSpec |
| `to_json` | `to_json() -> str` | Serialize to JSON string |

### Properties (get/set)

| Property | Type | Description |
|----------|------|-------------|
| `name` | `PackageName` | Package name |
| `version` | `VersionWithSource` | Package version |
| `build` | `str` | Build string |
| `build_number` | `int` | Build number |
| `subdir` | `str` | Subdirectory (e.g., `"linux-64"`) |
| `arch` | `str \| None` | Architecture |
| `platform` | `str \| None` | OS platform |
| `noarch` | `NoArchType` | NoArch type (`"python"`, `"generic"`, or `None`) |
| `depends` | `list[str]` | Package dependencies (MatchSpec strings) |
| `constrains` | `list[str]` | Additional constraints on packages |
| `sha256` | `bytes \| None` | SHA256 hash of package archive |
| `md5` | `bytes \| None` | MD5 hash of package archive |
| `size` | `int \| None` | Size of package archive in bytes |
| `timestamp` | `datetime.datetime \| None` | Creation timestamp |
| `license` | `str \| None` | SPDX license identifier |
| `license_family` | `str \| None` | License family |
| `features` | `str \| None` | Deprecated feature spec |
| `track_features` | `list[str]` | Track features (for downweighting) |
| `legacy_bz2_md5` | `bytes \| None` | Deprecated legacy hash |
| `legacy_bz2_size` | `int \| None` | Deprecated legacy size |
| `python_site_packages_path` | `str \| None` | Path to site-packages (Python packages) |

### Comparison and Hashing

Supports `==`, `!=`, `<`, `<=`, `>`, `>=`, and `hash()`. Ordering is by name, then track features, then version, then build number, then timestamp.

```python
str(record)   # "name=version=build"
```

---

## RepoDataRecord

Extends `PackageRecord` with URL and channel information. Inherits all properties and methods from `PackageRecord`.

### Constructor

```python
RepoDataRecord(
    package_record: PackageRecord,
    file_name: str,
    url: str,
    channel: str,
)
```

### Additional Properties (get/set)

| Property | Type | Description |
|----------|------|-------------|
| `url` | `str` | Canonical URL to download this package |
| `channel` | `str` | Channel URL or name |
| `file_name` | `str` | Filename of the package archive |

---

## NoArchType

Specifies the noarch type of a package.

### Constructor

```python
NoArchType(noarch: Literal["python", "generic", True] | None = None)
```

- `None` → no noarch (platform-specific)
- `"python"` → pure Python package (noarch: python)
- `"generic"` or `True` → generic noarch package

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `generic` | `bool` | `True` if noarch is "generic" |
| `python` | `bool` | `True` if noarch is "python" |
| `none` | `bool` | `True` if noarch is not set |

Supports `==`, `!=`, and `hash()`.

---

## PatchInstructions

Instructions for patching repodata. Used with `RepoData.apply_patches()`.
