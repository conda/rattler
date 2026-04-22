# MatchSpec and NamelessMatchSpec

## MatchSpec

A query language for conda packages. Composed of package name, version, build, channel, and other attributes. Supports wildcards, glob patterns, and version ranges.

### Constructor

```python
MatchSpec(
    spec: str,
    strict: bool = False,
    exact_names_only: bool = True,
    experimental_extras: bool = False,
    experimental_conditionals: bool = False,
)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `spec` | `str` | required | MatchSpec string (e.g., `"numpy >=1.26"`) |
| `strict` | `bool` | `False` | Reject ambiguous version specs |
| `exact_names_only` | `bool` | `True` | When `False`, names can be glob patterns or regex |
| `experimental_extras` | `bool` | `False` | Enable extras syntax (`pkg[extras=[foo,bar]]`) |
| `experimental_conditionals` | `bool` | `False` | Enable conditional syntax (`pkg[when="python >=3.6"]`) |

### Spec Syntax

```python
# Simple name + version
MatchSpec("numpy >=1.26")
MatchSpec("python ==3.12.0")
MatchSpec("scipy 1.11.*")

# Channel-qualified
MatchSpec("conda-forge::numpy")
MatchSpec("conda-forge/linux-64::numpy >=1.26")

# Bracket notation
MatchSpec("numpy[version='>=1.26,<2', build='py312*']")
MatchSpec("package[channel=conda-forge, subdir=linux-64]")

# From URL
MatchSpec.from_url("https://conda.anaconda.org/conda-forge/linux-64/numpy-1.26.4-py312h.conda")

# From NamelessMatchSpec + name
MatchSpec.from_nameless(nameless_spec, "numpy")
```

### Properties

All properties are read-only.

| Property | Type | Description |
|----------|------|-------------|
| `name` | `PackageNameMatcher` | Package name (exact, glob, or regex matcher) |
| `version` | `str \| None` | Version spec (e.g., `">=1.2.3"`, `"1.2.*"`) |
| `build` | `str \| None` | Build string (e.g., `"py312_0"`, `"py*"`) |
| `build_number` | `str \| None` | Build number |
| `file_name` | `str \| None` | Specific filename to match |
| `channel` | `Channel \| None` | Channel constraint |
| `subdir` | `str \| None` | Subdirectory constraint |
| `namespace` | `str \| None` | Package namespace |
| `extras` | `list[str] \| None` | Optional dependency extras (experimental) |
| `condition` | `str \| None` | Conditional expression (experimental) |
| `md5` | `bytes \| None` | MD5 hash constraint |
| `sha256` | `bytes \| None` | SHA256 hash constraint |

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `matches` | `matches(record: PackageRecord) -> bool` | Returns `True` if the record satisfies this spec |
| `from_nameless` | `from_nameless(spec: NamelessMatchSpec, name: str) -> MatchSpec` | Construct from a NamelessMatchSpec and a name |
| `from_url` | `from_url(url: str) -> MatchSpec` | Construct from a package URL |

---

## NamelessMatchSpec

Like `MatchSpec` but without the package name. Useful when the name is already known (e.g., in version pinning files).

### Constructor

```python
NamelessMatchSpec(
    spec: str,
    strict: bool = False,
    experimental_extras: bool = False,
    experimental_conditionals: bool = False,
)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `spec` | `str` | required | Version/build spec without name (e.g., `">=1.26"`, `"3.4.1 *cuda"`) |
| `strict` | `bool` | `False` | Reject ambiguous version specs |
| `experimental_extras` | `bool` | `False` | Enable extras syntax |
| `experimental_conditionals` | `bool` | `False` | Enable conditional syntax |

### Properties

Same as `MatchSpec` except no `name` property.

| Property | Type | Description |
|----------|------|-------------|
| `version` | `str \| None` | Version spec |
| `build` | `str \| None` | Build string |
| `build_number` | `str \| None` | Build number |
| `file_name` | `str \| None` | Filename |
| `channel` | `Channel \| None` | Channel |
| `subdir` | `str \| None` | Subdirectory |
| `namespace` | `str \| None` | Namespace |
| `extras` | `list[str] \| None` | Extras (experimental) |
| `condition` | `str \| None` | Condition (experimental) |
| `md5` | `bytes \| None` | MD5 hash |
| `sha256` | `bytes \| None` | SHA256 hash |

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `matches` | `matches(package_record: PackageRecord) -> bool` | Returns `True` if the record satisfies this spec |
| `from_match_spec` | `from_match_spec(spec: MatchSpec) -> NamelessMatchSpec` | Extract the nameless portion from a MatchSpec |

---

## PackageNameMatcher

Returned by `MatchSpec.name`. Supports exact names, glob patterns, and regex.

```python
from rattler import MatchSpec

# Exact match
spec = MatchSpec("numpy", exact_names_only=True)
spec.name.normalized  # "numpy"
spec.name.as_package_name()  # PackageName("numpy")

# Glob match (requires exact_names_only=False)
spec = MatchSpec("jupyter-*", exact_names_only=False)
spec.name.normalized  # "jupyter-*"
spec.name.as_package_name()  # None (not an exact name)

# Regex match
spec = MatchSpec("^jupyter-.*$", exact_names_only=False)
```

| Property/Method | Return Type | Description |
|-----------------|-------------|-------------|
| `normalized` | `str` | Normalized string (package name for exact, pattern for glob/regex) |
| `as_package_name()` | `PackageName \| None` | Convert to `PackageName` if exact match, else `None` |
