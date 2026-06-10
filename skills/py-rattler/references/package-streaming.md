# Package Streaming

Download and extract conda package archives (`.conda` and `.tar.bz2`).

## Local Extraction

### extract()

```python
def extract(path: os.PathLike[str], dest: os.PathLike[str]) -> tuple[bytes, bytes]
```

Extract a `.conda` package archive to a destination directory.

| Parameter | Type | Description |
|-----------|------|-------------|
| `path` | `os.PathLike[str]` | Path to the `.conda` archive |
| `dest` | `os.PathLike[str]` | Destination directory |

**Returns:** `tuple[bytes, bytes]`

### extract_tar_bz2()

```python
def extract_tar_bz2(path: os.PathLike[str], dest: os.PathLike[str]) -> tuple[bytes, bytes]
```

Extract a `.tar.bz2` package archive to a destination directory.

| Parameter | Type | Description |
|-----------|------|-------------|
| `path` | `os.PathLike[str]` | Path to the `.tar.bz2` archive |
| `dest` | `os.PathLike[str]` | Destination directory |

**Returns:** `tuple[bytes, bytes]`

---

## Remote Download

All download functions are **async**.

### download_to_path()

```python
async def download_to_path(
    client: Client,
    url: str,
    dest: os.PathLike[str],
) -> None
```

Stream a package archive from URL to a file on disk (non-buffered).

### download_bytes()

```python
async def download_bytes(client: Client, url: str) -> bytes
```

Download a package archive from URL into memory (full response buffered).

### download_to_writer()

```python
async def download_to_writer(
    client: Client,
    url: str,
    writer: object,
) -> None
```

Stream a package archive from URL to a Python writer object. The writer must have a synchronous `write(bytes)` method (e.g., `io.BytesIO`, file object).

### download_and_extract()

```python
async def download_and_extract(
    client: Client,
    url: str,
    dest: os.PathLike[str],
    expected_sha: bytes | None = None,
) -> tuple[bytes, bytes]
```

Download a package from URL and extract it to a destination directory.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `client` | `Client` | required | HTTP client |
| `url` | `str` | required | Package URL |
| `dest` | `os.PathLike[str]` | required | Destination directory |
| `expected_sha` | `bytes \| None` | `None` | Expected SHA256 for verification |

### fetch_raw_package_file_from_url()

```python
async def fetch_raw_package_file_from_url(
    client: Client,
    url: str,
    path: str,
) -> bytes
```

Fetch a single file from inside a remote `.conda` package using sparse HTTP range requests. Does **not** download the entire archive.

| Parameter | Type | Description |
|-----------|------|-------------|
| `client` | `Client` | HTTP client |
| `url` | `str` | URL of the `.conda` package |
| `path` | `str` | File path inside the package (e.g., `"info/index.json"`) |

**Returns:** Raw file bytes.

**Example:**

```python
from rattler import Client
from rattler.package_streaming import fetch_raw_package_file_from_url
import json

client = Client.default_client()
content = await fetch_raw_package_file_from_url(
    client,
    "https://conda.anaconda.org/conda-forge/linux-64/numpy-1.26.4-py312h.conda",
    "info/index.json",
)
metadata = json.loads(content)
print(metadata["name"], metadata["version"])
```
