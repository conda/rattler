# Networking

## Client

HTTP client with composable middleware for authentication, retries, mirrors, and cloud storage.

### Constructor

```python
Client(
    middlewares: list[...] | None = None,
    headers: dict[str, str] | None = None,
    timeout: int | None = None,
)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `middlewares` | `list \| None` | `None` | List of middleware instances (applied in order) |
| `headers` | `dict[str, str] \| None` | `None` | Default headers for all requests |
| `timeout` | `int \| None` | `None` | Request timeout in seconds |

### Static Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `default_client` | `Client.default_client(max_retries: int = 3) -> Client` | Client with standard middleware stack: retry, authentication, OCI, GCS, S3 |
| `authenticated_client` | `Client.authenticated_client() -> Client` | Same as `default_client()` (full middleware stack) |

**Example:**

```python
from rattler import Client

# Quick default
client = Client.default_client(max_retries=3)

# Custom with headers and timeout
client = Client(
    headers={"X-Custom": "value"},
    timeout=30,
)
```

---

## Middleware

Middleware is composable. Pass a list to `Client(middlewares=[...])`. Order matters — middleware is applied in the order given.

### RetryMiddleware

Retries transient HTTP errors with exponential back-off.

```python
RetryMiddleware(max_retries: int = 3)
```

### AuthenticationMiddleware

Reads credentials from the conda keychain/authentication storage.

```python
AuthenticationMiddleware()
```

### MirrorMiddleware

Replaces channel URLs with mirror URLs.

```python
MirrorMiddleware(mirrors: dict[str, list[str]])
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `mirrors` | `dict[str, list[str]]` | Map of original URL to list of mirror URLs |

**Example:**

```python
MirrorMiddleware({
    "https://conda.anaconda.org/conda-forge": [
        "https://repo.prefix.dev/conda-forge",
        "https://mirror.example.com/conda-forge",
    ],
})
```

### OciMiddleware

Handles `oci://` URLs for Docker/OCI registry-hosted channels.

```python
OciMiddleware()
```

### S3Middleware

Handles `s3://` URLs with AWS authentication.

```python
S3Middleware(config: dict[str, S3Config] | None = None)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `config` | `dict[str, S3Config] \| None` | `None` | Per-bucket configuration. If `None`, uses AWS SDK defaults |

### S3Config

Per-bucket S3 configuration.

```python
S3Config(
    endpoint_url: str | None = None,
    region: str | None = None,
    force_path_style: bool | None = None,
)
```

### GCSMiddleware

Handles `gcs://` URLs with Google Cloud Storage authentication.

```python
GCSMiddleware()
```

### AddHeadersMiddleware

Adds custom headers to requests based on a callback.

```python
AddHeadersMiddleware(callback: Callable[[str, str], dict[str, str] | None])
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `callback` | `Callable[[str, str], dict[str, str] \| None]` | Function receiving `(host, path)`, returns headers dict or `None` |

**Example:**

```python
def add_auth(host: str, path: str) -> dict[str, str] | None:
    if host == "private.example.com":
        return {"Authorization": "Bearer token123"}
    return None

AddHeadersMiddleware(add_auth)
```

---

## Composing a Custom Client

```python
from rattler import Client
from rattler.networking import (
    RetryMiddleware, AuthenticationMiddleware, MirrorMiddleware,
    OciMiddleware, S3Middleware, GCSMiddleware, AddHeadersMiddleware,
)

client = Client(middlewares=[
    RetryMiddleware(max_retries=3),
    AuthenticationMiddleware(),
    MirrorMiddleware({
        "https://conda.anaconda.org/conda-forge": [
            "https://repo.prefix.dev/conda-forge"
        ],
    }),
    S3Middleware(),
    GCSMiddleware(),
    OciMiddleware(),
])
```

---

## fetch_repo_data()

Low-level function to fetch repodata directly (without Gateway).

```python
async def fetch_repo_data(
    *,
    channels: list[Channel],
    platforms: list[Platform],
    cache_path: str | os.PathLike[str],
    callback: Callable[[int, int], None] | None,
    client: Client | None = None,
    fetch_options: FetchRepoDataOptions | None = None,
) -> list[SparseRepoData]
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `channels` | `list[Channel]` | required | Channels to fetch from |
| `platforms` | `list[Platform]` | required | Platforms to fetch for |
| `cache_path` | `str \| os.PathLike[str]` | required | Cache directory for downloaded data |
| `callback` | `Callable[[int, int], None] \| None` | required | Progress callback `(current, total)` |
| `client` | `Client \| None` | `None` | HTTP client (default if `None`) |
| `fetch_options` | `FetchRepoDataOptions \| None` | `None` | Fetch configuration |

**Returns:** `list[SparseRepoData]`

---

## FetchRepoDataOptions

```python
@dataclass
class FetchRepoDataOptions:
    cache_action: CacheAction = "cache-or-fetch"
    variant: Variant = "after-patches"
    zstd_enabled: bool = True
    bz2_enabled: bool = True
```

**CacheAction values:** `"cache-or-fetch"`, `"use-cache-only"`, `"force-cache-only"`, `"no-cache"`

**Variant values:** `"after-patches"`, `"from-packages"`, `"current"`
