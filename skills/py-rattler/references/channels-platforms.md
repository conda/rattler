# Channels and Platforms

## Channel

Represents a conda channel (a source of packages).

### Constructor

```python
Channel(name: str, channel_configuration: ChannelConfig | None = None)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | `str` | required | Channel name or URL (e.g., `"conda-forge"`, `"https://repo.prefix.dev/conda-forge"`) |
| `channel_configuration` | `ChannelConfig \| None` | `None` | Custom channel configuration |

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `name` | `str \| None` | Channel name |
| `base_url` | `str` | Base URL of the channel |

### Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `to_lock_channel` | `to_lock_channel() -> LockChannel` | Convert to a `LockChannel` for lock files |

**Example:**

```python
from rattler import Channel, ChannelConfig

ch = Channel("conda-forge")
ch = Channel("https://repo.prefix.dev/conda-forge")
print(ch.name)      # "conda-forge"
print(ch.base_url)  # "https://conda.anaconda.org/conda-forge/"

# With custom alias
config = ChannelConfig(channel_alias="https://repo.prefix.dev/")
ch = Channel("my-channel", config)
```

---

## ChannelConfig

Configuration for channel URL resolution.

### Constructor

```python
ChannelConfig(
    channel_alias: str = "https://conda.anaconda.org/",
    root_dir: str = os.getcwd(),
)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `channel_alias` | `str` | `"https://conda.anaconda.org/"` | Base URL for resolving channel names |
| `root_dir` | `str` | `os.getcwd()` | Root directory for resolving local channels |

---

## ChannelPriority

Enum controlling how multi-channel package searches work. Used in `solve()`.

| Value | Description |
|-------|-------------|
| `ChannelPriority.Strict` | Use the first channel where a package is found exclusively |
| `ChannelPriority.Disabled` | Search all channels, prefer highest version regardless of channel |

---

## Platform

Represents a target platform. Uses a singleton pattern: same platform string returns the same instance.

### Constructor

```python
Platform(value: PlatformLiteral | str)
```

### PlatformLiteral

```python
PlatformLiteral = Literal[
    "noarch", "unknown",
    "linux-32", "linux-64", "linux-aarch64", "linux-armv6l", "linux-armv7l",
    "linux-loongarch64", "linux-ppc64le", "linux-ppc64", "linux-ppc",
    "linux-s390x", "linux-riscv32", "linux-riscv64",
    "freebsd-64",
    "osx-64", "osx-arm64",
    "win-32", "win-64", "win-arm64",
    "emscripten-wasm32", "wasi-wasm32", "zos-z",
]
```

### Class Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `current` | `Platform.current() -> Platform` | Detect the current platform |
| `all` | `Platform.all() -> Iterator[Platform]` | Iterate over all 23 supported platforms |

### Properties

| Property | Type | Description |
|----------|------|-------------|
| `is_linux` | `bool` | `True` if platform is Linux |
| `is_osx` | `bool` | `True` if platform is macOS |
| `is_windows` | `bool` | `True` if platform is Windows |
| `is_unix` | `bool` | `True` if platform is Unix (Linux or macOS) |
| `arch` | `Arch \| None` | Architecture (e.g., `x86_64`, `aarch64`) |
| `only_platform` | `str \| None` | Platform name without architecture |

**Example:**

```python
from rattler import Platform

p = Platform.current()
print(p)           # e.g., "osx-arm64"
print(p.is_osx)   # True
print(p.arch)      # Arch("aarch64")

for platform in Platform.all():
    print(platform)
```
