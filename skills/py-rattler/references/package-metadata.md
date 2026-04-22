# Package Metadata

Classes for reading package metadata files from conda archives (`info/` directory).

All metadata classes support loading from multiple sources:

| Method | Description |
|--------|-------------|
| `from_path(path)` | Parse from a file on disk |
| `from_package_directory(path)` | Parse from an extracted package directory |
| `from_package_archive(path)` | Parse directly from a `.conda` or `.tar.bz2` archive |
| `from_str(string)` | Parse from a JSON string |
| `await from_remote_url(client, url)` | Fetch from a remote package archive URL (async, sparse range request) |
| `package_path()` | Returns the relative path inside the archive (e.g., `info/index.json`) |

---

## PackageName

Represents a normalized conda package name.

### Constructor

```python
PackageName(source: str)
```

### Static Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `unchecked` | `PackageName.unchecked(normalized: str) -> PackageName` | Construct without validation (use only if input is known valid) |
| `from_matchspec_str` | `PackageName.from_matchspec_str(spec: str) -> PackageName` | Parse name from a MatchSpec string (splits on whitespace/version chars) |
| `from_matchspec_str_unchecked` | `PackageName.from_matchspec_str_unchecked(spec: str) -> PackageName` | Parse name from MatchSpec string without validation (preserves case) |

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `source` | `str` | Original string used to create this name |
| `normalized` | `str` | Normalized, valid conda package name |

Supports `==`, `!=`, and `hash()`. Can compare with strings.

---

## IndexJson

Contents of `info/index.json` — core package metadata.

### Properties (get/set)

| Property | Type | Description |
|----------|------|-------------|
| `name` | `PackageName` | Package name |
| `version` | `Version` | Package version |
| `build` | `str` | Build string |
| `build_number` | `int` | Build number |
| `depends` | `list[str]` | Package dependencies |
| `constrains` | `list[str]` | Package constraints |
| `arch` | `str \| None` | Architecture |
| `platform` | `str \| None` | OS platform |
| `subdir` | `str \| None` | Subdirectory |
| `license` | `str \| None` | License |
| `license_family` | `str \| None` | License family |
| `features` | `str \| None` | Deprecated features |
| `track_features` | `list[str]` | Track features |
| `timestamp` | `datetime.datetime \| None` | Creation timestamp |

**Example:**

```python
from rattler import IndexJson

idx = IndexJson.from_path("/path/to/info/index.json")
print(f"{idx.name} {idx.version} {idx.build}")
print(f"depends: {idx.depends}")

# From remote package (async, uses range request)
idx = await IndexJson.from_remote_url(client, "https://conda.anaconda.org/.../numpy-1.26.4-py312h.conda")
```

---

## AboutJson

Contents of `info/about.json` — descriptive metadata.

### Properties (get/set)

| Property | Type | Description |
|----------|------|-------------|
| `channels` | `list[str]` | Channels used during build |
| `description` | `str \| None` | Full package description |
| `dev_url` | `list[str]` | URLs to development pages |
| `doc_url` | `list[str]` | URLs to documentation |
| `home` | `list[str]` | Homepage URLs |
| `extra` | `dict[str, Any]` | JSON-serializable extra metadata |
| `license` | `str \| None` | License identifier |
| `license_family` | `str \| None` | License family |
| `source_url` | `str \| None` | URL to source code |
| `summary` | `str \| None` | Short summary description |

---

## RunExportsJson

Contents of `info/run_exports.json` — runtime dependencies that downstream packages must include.

### Constructor

```python
RunExportsJson(
    weak: list[str] | None = None,
    strong: list[str] | None = None,
    noarch: list[str] | None = None,
    weak_constrains: list[str] | None = None,
    strong_constrains: list[str] | None = None,
)
```

### Properties (get/set)

| Property | Type | Description |
|----------|------|-------------|
| `weak` | `list[str]` | Weak run exports (host → run dependency) |
| `strong` | `list[str]` | Strong run exports (build → host and run) |
| `noarch` | `list[str]` | Run exports only applied to noarch packages |
| `weak_constrains` | `list[str]` | Weak constrains (host → build or run → host) |
| `strong_constrains` | `list[str]` | Strong constrains (build → host and run) |

---

## PathsJson

Contents of `info/paths.json` — file manifest for the package.

### Properties (get/set)

| Property | Type | Description |
|----------|------|-------------|
| `paths` | `list[PathsEntry]` | All file entries in the package |
| `paths_version` | `int` | Version of the paths.json format |

### Additional Static Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `from_deprecated_package_directory` | `from_deprecated_package_directory(path) -> PathsJson` | Construct from older packages without paths.json |
| `from_package_directory_with_deprecated_fallback` | `from_package_directory_with_deprecated_fallback(path) -> PathsJson` | Try paths.json, fall back to deprecated format |

---

## PathsEntry

A single file entry in `paths.json`.

### Constructor

```python
PathsEntry(
    relative_path: str,
    no_link: bool,
    path_type: PathType,
    prefix_placeholder: PrefixPlaceholder | None,
    sha256: bytes | None,
    size_in_bytes: int | None,
)
```

### Properties (get/set)

| Property | Type | Description |
|----------|------|-------------|
| `relative_path` | `Path` | Relative path from package root |
| `no_link` | `bool` | Whether the file should be linked when installing |
| `path_type` | `PathType` | How to include the file |
| `prefix_placeholder` | `PrefixPlaceholder \| None` | Placeholder prefix for path rewriting |
| `sha256` | `bytes \| None` | SHA256 hash (paths.json v1 only) |
| `size_in_bytes` | `int \| None` | File size in bytes |

---

## PathType

How a file entry is included in the package.

```python
PathType(path_type: Literal["hardlink", "softlink", "directory"])
```

| Property | Type | Description |
|----------|------|-------------|
| `hardlink` | `bool` | Default: file is hard linked |
| `softlink` | `bool` | File is soft linked |
| `directory` | `bool` | Explicitly create empty directory |

---

## PrefixPlaceholder

Describes a placeholder in a file that must be replaced during installation.

```python
PrefixPlaceholder(file_mode: FileMode, placeholder: str)
```

| Property | Type | Description |
|----------|------|-------------|
| `file_mode` | `FileMode` | Binary or text file |
| `placeholder` | `str` | Placeholder prefix string (path where package was built) |

---

## FileMode

Whether a file is binary or text (affects prefix replacement strategy).

```python
FileMode(file_mode: Literal["binary", "text"])
```

| Property | Type | Description |
|----------|------|-------------|
| `binary` | `bool` | File is binary |
| `text` | `bool` | File is text |
| `unknown` | `bool` | File mode is unspecified |
