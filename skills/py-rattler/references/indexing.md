# Channel Indexing

Create conda channel repodata indexes for local filesystems or S3-hosted channels. Both functions are **async**.

## index_fs()

Index packages in a local channel directory.

```python
async def index_fs(
    channel_directory: os.PathLike[str],
    target_platform: Platform | None = None,
    repodata_patch: str | None = None,
    write_zst: bool = True,
    write_shards: bool = True,
    force: bool = False,
    max_parallel: int | None = None,
) -> None
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `channel_directory` | `os.PathLike[str]` | required | Root directory of the channel (contains platform subdirectories) |
| `target_platform` | `Platform \| None` | `None` | Specific platform to index (`None` = all platforms found) |
| `repodata_patch` | `str \| None` | `None` | Conda package name containing repodata patches |
| `write_zst` | `bool` | `True` | Write `repodata.json.zst` |
| `write_shards` | `bool` | `True` | Write sharded repodata |
| `force` | `bool` | `False` | Re-index all subdirectories even if unchanged |
| `max_parallel` | `int \| None` | `None` | Max packages to process in-memory simultaneously |

**Example:**

```python
from rattler import index_fs, Platform

# Index all platforms
await index_fs("/path/to/my-channel")

# Index only linux-64
await index_fs(
    "/path/to/my-channel",
    target_platform=Platform("linux-64"),
    force=True,
)
```

---

## index_s3()

Index packages in an S3-hosted channel.

```python
async def index_s3(
    channel_url: str,
    credentials: S3Credentials | None = None,
    target_platform: Platform | None = None,
    repodata_patch: str | None = None,
    write_zst: bool = True,
    write_shards: bool = True,
    force: bool = False,
    max_parallel: int | None = None,
    precondition_checks: bool = True,
) -> None
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `channel_url` | `str` | required | S3 URL (e.g., `"s3://my-bucket/my-channel"`) |
| `credentials` | `S3Credentials \| None` | `None` | S3 credentials (uses environment/SDK defaults if `None`) |
| `target_platform` | `Platform \| None` | `None` | Specific platform to index |
| `repodata_patch` | `str \| None` | `None` | Conda package name for repodata patches |
| `write_zst` | `bool` | `True` | Write `repodata.json.zst` |
| `write_shards` | `bool` | `True` | Write sharded repodata |
| `force` | `bool` | `False` | Re-index all subdirectories |
| `max_parallel` | `int \| None` | `None` | Max packages to process in parallel |
| `precondition_checks` | `bool` | `True` | Prevent data corruption with multi-process indexing |

---

## S3Credentials

```python
@dataclass
class S3Credentials:
    endpoint_url: str
    region: str
    access_key_id: str | None = None
    secret_access_key: str | None = None
    session_token: str | None = None
    addressing_style: Literal["path", "virtual-host"] = "virtual-host"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `endpoint_url` | `str` | required | S3 endpoint URL |
| `region` | `str` | required | AWS region |
| `access_key_id` | `str \| None` | `None` | AWS access key ID |
| `secret_access_key` | `str \| None` | `None` | AWS secret access key |
| `session_token` | `str \| None` | `None` | Session token for temporary credentials |
| `addressing_style` | `Literal["path", "virtual-host"]` | `"virtual-host"` | S3 bucket addressing style |

**Example:**

```python
from rattler import index_s3
from rattler.index import S3Credentials

await index_s3(
    "s3://my-conda-channel/",
    credentials=S3Credentials(
        endpoint_url="https://s3.amazonaws.com",
        region="us-east-1",
        access_key_id="AKIA...",
        secret_access_key="...",
    ),
)
```
