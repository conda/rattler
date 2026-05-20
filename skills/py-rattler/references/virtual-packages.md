# Virtual Packages

Virtual packages represent system capabilities that the package manager cannot install but can use for dependency resolution. They are prefixed with double underscores (e.g., `__linux`, `__cuda`, `__glibc`).

## VirtualPackage

Detected virtual package from the current system.

### Static Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `detect` | `VirtualPackage.detect(overrides: VirtualPackageOverrides = VirtualPackageOverrides()) -> list[VirtualPackage]` | Detect virtual packages for the current system with optional overrides |
| `current` | `VirtualPackage.current() -> list[VirtualPackage]` | **Deprecated** (use `detect` instead) |

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `into_generic` | `into_generic() -> GenericVirtualPackage` | Convert to a `GenericVirtualPackage` |

**Example:**

```python
from rattler import VirtualPackage

vpkgs = VirtualPackage.detect()
for vp in vpkgs:
    print(vp)  # e.g., "__osx=15.0=0", "__archspec=1=m1"
```

---

## GenericVirtualPackage

A manually constructed virtual package with explicit name, version, and build string.

### Constructor

```python
GenericVirtualPackage(
    name: PackageName,
    version: Version,
    build_string: str,
)
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `name` | `PackageName` | Package name (e.g., `PackageName("__cuda")`) |
| `version` | `Version` | Version (e.g., `Version("12.0")`) |
| `build_string` | `str` | Build string (typically `"0"`) |

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `name` | `PackageName` | Package name |
| `version` | `Version` | Package version |
| `build_string` | `str` | Build identifier |

**Example:**

```python
from rattler import GenericVirtualPackage, PackageName, Version

cuda = GenericVirtualPackage(
    PackageName("__cuda"),
    Version("12.0"),
    "0",
)
```

---

## VirtualPackageOverrides

Override automatic virtual package detection. Useful for cross-platform solving.

### Constructor

```python
VirtualPackageOverrides(
    osx: Override | None = None,
    libc: Override | None = None,
    cuda: Override | None = None,
    archspec: Override | None = None,
)
```

All parameters default to `None` (no override; use auto-detected value).

### Class Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `from_env` | `VirtualPackageOverrides.from_env() -> VirtualPackageOverrides` | Read overrides from environment variables |

### Properties (get/set)

| Property | Type | Description |
|----------|------|-------------|
| `osx` | `Override \| None` | macOS version override |
| `libc` | `Override \| None` | glibc version override |
| `cuda` | `Override \| None` | CUDA version override |
| `archspec` | `Override \| None` | CPU architecture override |

**Example:**

```python
from rattler import VirtualPackage, VirtualPackageOverrides, Override

# Solve for Linux with specific glibc and CUDA
overrides = VirtualPackageOverrides(
    libc=Override.string("2.17"),
    cuda=Override.string("12.0"),
)
vpkgs = VirtualPackage.detect(overrides)

# Or read from environment
overrides = VirtualPackageOverrides.from_env()
```

---

## Override

Specifies how to override a virtual package value.

### Factory Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `default_env_var` | `Override.default_env_var() -> Override` | Use the default environment variable for this virtual package (overrides auto-detection if set) |
| `env_var` | `Override.env_var(env_var: str) -> Override` | Read override value from a specific environment variable |
| `string` | `Override.string(override: str) -> Override` | Use a literal string value directly |

**Example:**

```python
from rattler import Override

# Use a specific version string
Override.string("14.0")

# Read from the default environment variable
Override.default_env_var()

# Read from a custom environment variable
Override.env_var("MY_CUDA_VERSION")
```
