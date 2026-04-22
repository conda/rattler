# Prefix Records

Classes for inspecting packages installed in a conda environment (`conda-meta/` directory).

## PrefixRecord

Record of a package installed in a conda prefix. Extends `RepoDataRecord` with installation-specific metadata.

### Constructor

```python
PrefixRecord(
    repodata_record: RepoDataRecord,
    paths_data: PrefixPaths,
    link: Link | None = None,
    package_tarball_full_path: os.PathLike[str] | None = None,
    extracted_package_dir: os.PathLike[str] | None = None,
    requested_spec: str | None = None,
    requested_specs: list[str] | None = None,
    files: list[os.PathLike[str]] | None = None,
)
```

### Static Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `from_path` | `PrefixRecord.from_path(path: os.PathLike[str]) -> PrefixRecord` | Parse from a `conda-meta/*.json` file |

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `write_to_path` | `write_to_path(path: os.PathLike[str], pretty: bool) -> None` | Write to a file |

### Properties (get/set)

Inherits all properties from `RepoDataRecord` and `PackageRecord`, plus:

| Property | Type | Description |
|----------|------|-------------|
| `package_tarball_full_path` | `Path \| None` | Path to the package archive file |
| `extracted_package_dir` | `Path \| None` | Path to the extracted package directory |
| `files` | `list[Path]` | Sorted list of all files belonging to this package |
| `paths_data` | `PrefixPaths` | Information about how files were linked |
| `requested_spec` | `str \| None` | Original spec used for installation (deprecated) |
| `requested_specs` | `list[str]` | Original specs used for installation |

**Example:**

```python
from rattler import PrefixRecord
from pathlib import Path

# Read all installed packages from an environment
conda_meta = Path("/opt/envs/myenv/conda-meta")
for json_file in conda_meta.glob("*.json"):
    record = PrefixRecord.from_path(json_file)
    print(f"{record.name} {record.version}")
    print(f"  files: {len(record.files)}")
```

---

## PrefixPaths

Collection of path entries for an installed package.

### Constructor

```python
PrefixPaths(paths_version: int = 1)
```

### Properties (get/set)

| Property | Type | Description |
|----------|------|-------------|
| `paths_version` | `int` | Version of the paths format |
| `paths` | `list[PrefixPathsEntry]` | All file entries |

---

## PrefixPathsEntry

A single file entry in an installed package. Implements `os.PathLike`.

### Constructor

```python
PrefixPathsEntry(
    relative_path: os.PathLike[str],
    path_type: PrefixPathType,
    prefix_placeholder: str | None = None,
    file_mode: FileMode | None = None,
    sha256: bytes | None = None,
    sha256_in_prefix: bytes | None = None,
    size_in_bytes: int | None = None,
    original_path: os.PathLike[str] | None = None,
)
```

### Properties (get/set)

| Property | Type | Description |
|----------|------|-------------|
| `relative_path` | `os.PathLike[str]` | Relative path from package root |
| `no_link` | `bool` | Whether the file should not be linked |
| `path_type` | `PrefixPathType` | How the file was installed |
| `prefix_placeholder` | `str \| None` | Placeholder prefix in the file |
| `file_mode` | `FileMode` | Binary or text file |
| `sha256` | `bytes` | SHA256 hash of the original file |
| `sha256_in_prefix` | `bytes` | SHA256 hash after prefix replacement |
| `size_in_bytes` | `int` | File size in bytes |

---

## PrefixPathType

How a file was installed into the prefix. Extends `PathType` with Python-specific types.

```python
PrefixPathType(path_type: Literal[
    "hardlink", "softlink", "directory",
    "pyc_file", "windows_python_entry_point_script",
    "windows_python_entry_point_exe", "unix_python_entry_point",
])
```

| Property | Type | Description |
|----------|------|-------------|
| `hardlink` | `bool` | File was hard linked |
| `softlink` | `bool` | File was soft linked |
| `directory` | `bool` | Directory was created |
| `pyc_file` | `bool` | Compiled Python `.pyc` file |
| `windows_python_entry_point_script` | `bool` | Windows Python entry point script |
| `windows_python_entry_point_exe` | `bool` | Windows Python entry point executable |
| `unix_python_entry_point` | `bool` | Unix Python entry point script |

---

## Link

Link information for how a package was linked.

```python
Link(path: os.PathLike[str], type: LinkType | None)
```

---

## LinkType

Enum for file link types.

| Value | Description |
|-------|-------------|
| `LinkType.HARDLINK` | Hard link |
| `LinkType.SOFTLINK` | Symbolic link |
| `LinkType.COPY` | File copy |
| `LinkType.DIRECTORY` | Directory |
