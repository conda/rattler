# Version and VersionSpec

## Version

Represents a conda package version. Supports epoch, segments, local versions, comparison, and bumping.

Version strings can contain alphanumeric characters (A-Za-z0-9) separated by dots and underscores. An optional epoch (`N!`) can precede the version string. Comparison is case-insensitive.

### Constructor

```python
Version(version: str)
```

**Examples:**

```python
v = Version("1.2.3")
v = Version("2!1.0.0")        # epoch 2
v = Version("1.0.0+local")    # local segment
v = Version("1.0.0dev1")      # dev version
```

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `epoch` | `int \| None` | Epoch number, or `None` if not defined |
| `has_local` | `bool` | `True` if a local segment is defined (part after `+`) |
| `is_dev` | `bool` | `True` if version contains a "dev" component |
| `segment_count` | `int` | Number of segments (excludes epoch and local) |

### Segment Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `segments` | `segments() -> list[list[str \| int]]` | Returns all non-local segments |
| `local_segments` | `local_segments() -> list[list[str \| int]]` | Returns only local segments |
| `as_major_minor` | `as_major_minor() -> tuple[int, int] \| None` | Returns `(major, minor)` or `None` if < 2 segments |
| `pop_segments` | `pop_segments(n: int = 1) -> Version` | Remove `n` trailing segments. Raises `InvalidVersionError` if result is invalid |
| `with_segments` | `with_segments(start: int, stop: int) -> Version` | Return version with segments `[start, stop)`. Raises `InvalidVersionError` if invalid |
| `extend_to_length` | `extend_to_length(length: int) -> Version` | Pad with zeros to reach `length` segments |

### Bumping Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `bump_major` | `bump_major() -> Version` | Bump the major (first) segment |
| `bump_minor` | `bump_minor() -> Version` | Bump the minor (second) segment |
| `bump_patch` | `bump_patch() -> Version` | Bump the patch (third) segment |
| `bump_last` | `bump_last() -> Version` | Bump the last segment |
| `bump_segment` | `bump_segment(index: int) -> Version` | Bump a specific segment by index |
| `with_alpha` | `with_alpha() -> Version` | Append alpha character to last segment if not already present |

### Modification Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `strip_local` | `strip_local() -> Version` | Return version without local segment |
| `remove_local` | `remove_local() -> Version` | Same as `strip_local` |

### Comparison Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `starts_with` | `starts_with(other: Version) -> bool` | Check if version and local segment start the same as `other` |
| `compatible_with` | `compatible_with(other: Version) -> bool` | Check compatibility (minor changes compatible, major changes break) |

### Operators

Supports `==`, `!=`, `<`, `<=`, `>`, `>=`, and `hash()`.

```python
Version("1.2.3") > Version("1.2.2")   # True
Version("1.2.3") == Version("1.2.3")  # True
{Version("1.0")}                       # hashable
```

---

## VersionSpec

A version constraint specification. Supports simple constraints (`>=1.2.3`), compound constraints (`>=1.2.3,<2.0.0`), and union constraints (`>=1.2.3|<1.0.0`).

### Constructor

```python
VersionSpec(spec: str, strict: bool = False)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `spec` | `str` | required | Version constraint string |
| `strict` | `bool` | `False` | Use strict parsing mode |

**Examples:**

```python
VersionSpec(">=1.2.3")
VersionSpec(">=1.2.3,<2.0.0")   # AND
VersionSpec(">=2.0|<1.0")       # OR
VersionSpec("1.2.*")            # wildcard
VersionSpec("~=1.2.3")          # compatible release
```

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `matches` | `matches(version: Version) -> bool` | Returns `True` if the version satisfies this spec |

### Operators

Supports `==`, `!=`, and `hash()`.

---

## VersionWithSource

A subclass of `Version` that preserves the original source string. Useful when you want to retain the exact input representation after parsing (e.g., `"1.01"` parses as `Version("1.1")` but `VersionWithSource("1.01")` keeps `"1.01"` as its string form).

### Constructor

```python
VersionWithSource(version: str | Version)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `version` | `str \| Version` | Source string (preserved as-is) or existing `Version` (source becomes `str(version)`) |

### Behavior

- `__str__()` returns the original source string, not the parsed normalized form.
- `__repr__()` returns e.g. `VersionWithSource(version="1.1", source="1.01")`.
- Inherits all comparison, segment, bumping, and modification methods from `Version`.
- Comparison and hashing are based on the parsed `Version`, not the source string. So `VersionWithSource("1.01") == VersionWithSource("1.1")` is `True`.

**Example:**

```python
from rattler import VersionWithSource

v1 = VersionWithSource("1.01")
v2 = VersionWithSource("1.1")
str(v1)          # "1.01"
str(v2)          # "1.1"
v1 == v2         # True — compared by parsed version
```

`VersionWithSource` is the type returned by `PackageRecord.version` and `IndexJson.version`.
