# SparseRepoData

::: rattler.repo_data.sparse

## Load repodata from memory

Use `SparseRepoData.from_bytes` when the repodata JSON is already in memory:

```python
from rattler import Channel, SparseRepoData

channel = Channel("https://example.org/channel")
data = b'{"packages": {}, "packages.conda": {}}'
with SparseRepoData.from_bytes(channel, "linux-64", data) as repo:
    names = repo.package_names()
```

The constructor copies the uncompressed UTF-8 JSON bytes into owned native storage.
The caller can release the original bytes after construction. The resulting object
supports the same loading, solving and close operations as a file-backed instance.
