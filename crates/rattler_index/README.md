# rattler_index

`rattler_index` creates or updates conda channel indexes by writing
`repodata.json`, optional compressed repodata, and optional sharded repodata for
packages stored on a local filesystem, in S3, or in Azure Blob Storage.

S3 support requires the `s3` feature; Azure Blob Storage support requires the
`azure` feature.

## CLI Usage

Index a local channel:

```shell
rattler-index --config ./rattler-config.toml fs ./channel
```

Index an S3 channel:

```shell
rattler-index --config ./rattler-config.toml s3 s3://my-bucket/my-channel
```

Index an Azure Blob Storage channel:

```shell
rattler-index --config ./rattler-config.toml az \
    az://my-storage-account.blob.core.windows.net/my-container/my-channel \
    --azure-cli
```

Indexing writes to the container through opendal, which only accepts a storage
account key or a shared access signature (SAS) token — it cannot use an `az
login` AAD bearer token directly. Supply one of:

- `--azure-cli`: mint a short-lived user-delegation SAS from the current `az
  login` session automatically. Requires the Azure CLI (`az`) on `PATH` and a
  prior `az login`. The minted SAS is scoped to the target container, granted
  only the permissions indexing needs, and expires after 30 minutes (a SAS
  cannot be individually revoked, so it is kept short-lived on purpose).
- `--account-key` / `AZURE_STORAGE_KEY`: a storage account key.
- `--sas-token` / `AZURE_STORAGE_SAS_TOKEN`: a SAS token you supply yourself.

To mint a SAS manually instead of using `--azure-cli` (for example, to reuse it
across several commands), generate one from your `az login` session and pass it
via `--sas-token`:

```shell
export AZURE_STORAGE_SAS_TOKEN=$(az storage container generate-sas \
    --account-name my-storage-account --name my-container \
    --permissions rwlc --expiry 2026-01-01T00:00Z \
    --auth-mode login --as-user --https-only -o tsv)
rattler-index --config ./rattler-config.toml az \
    az://my-storage-account.blob.core.windows.net/my-container/my-channel
```

The `--config` flag points at the same TOML configuration file used by pixi. It
configures S3 and Azure credentials, concurrency, and per-channel index options
under the `[index-config]` section.

When `--config` is omitted, `rattler-index` falls back to its built-in defaults
(`write-zst = true`, `write-shards = true`, no advertised repodata revisions,
`from-index-json` revision assignment, no channel metadata).

## Remote storage credentials

S3 endpoint and region settings live under `[s3-options.<bucket>]`, keyed by
bucket name:

```toml
[s3-options.my-bucket]
endpoint-url = "https://my-bucket.s3.amazonaws.com"
region = "eu-central-1"
force-path-style = false
```

Azure Blob Storage needs no such block: the storage account and blob endpoint
are read directly from the channel URL
(`az://<account>.blob.core.windows.net/<container>/<channel>`), so the account,
container, and endpoint (including sovereign clouds) are fully determined by the
URL you pass. The `az://` scheme is required and is rewritten to `https://`
internally; a bare `https://` URL is rejected. The host must be a dotted
`<account>.blob.<suffix>` domain, so IP-literal / single-label hosts (and hence
the Azurite emulator) are not supported. Credentials are never stored in the
config — they are resolved at runtime from `--account-key` / `--sas-token`, an
`az login` session (`--azure-cli`), or the `DefaultCredentialProvider` chain.

## Per-channel index configuration

Index options live in `[index-config]` and follow the same shape as
[`[repodata-config]`](https://pixi.prefix.dev/latest/reference/pixi_configuration/#repodata-config):
a flat default block with optional per-channel overrides keyed by URL or
absolute path. The most specific entry wins; less-specific entries supply
fallback values, and `[index-config]` itself is the final fallback.

```toml
# Defaults applied when no per-channel entry matches
[index-config]
write-zst = true
write-shards = true
repodata-revisions = ["v3"]
package-revision-assignment = "from-index-json"

# Per-host: applies to every channel under this bucket
[index-config."s3://my-bucket"]
base-url = "../packages/"

# Per-channel: most specific entry wins, then falls back to the host entry,
# then to [index-config]
[index-config."s3://my-bucket/staging"]
write-shards = false
package-revision-assignment = "latest"

[index-config."s3://my-bucket/staging".channel-relations]
base = "../conda-forge"

# Local directory keys are absolute paths
[index-config."/srv/conda/internal"]
base-url = "../packages/"
```

Matching rules:

- Keys are compared against the canonical channel target — the full URL for
  remote channels (`s3://...`) or the canonicalized absolute path for local
  channels.
- Match is on path-component boundaries: `s3://my-bucket` matches
  `s3://my-bucket/staging` but not `s3://my-bucket-other/staging`.
- All matching keys are layered onto `[index-config]` in shortest-to-longest
  order, so the most specific entry overrides earlier values field by field.

### Supported fields

| Field | Type | Description |
| --- | --- | --- |
| `write-zst` | boolean | Writes `repodata.json.zst`. Defaults to `true`. |
| `write-shards` | boolean | Writes `repodata_shards.msgpack.zst` and shard files. Defaults to `true`. |
| `repodata-revisions` | array | Repodata revisions to enable. Each entry is a string (`"v3"`, `"legacy"`). The indexer fills revision package counts and timestamps while writing repodata. |
| `package-revision-assignment` | string | Controls which `repodata-revisions` bucket a freshly indexed package lands in. `from-index-json` (default) reads the revision from each package's `info/index.json`, so legacy packages stay in the legacy maps and v3-tagged packages go to the v3 bucket. `latest` is an opt-in override that forces every package into the newest configured revision — useful for migrating a whole channel onto v3 in one shot. A future revision-assignment mode will pick based on a package's timestamp so repodata can be deterministically recreated. |
| `base-url` | string | Writes `info.base_url` in generated `repodata.json` and sharded repodata metadata. May be relative or absolute. |
| `channel-relations.base` | string | A single channel reference with higher priority than this channel, written to `info.channel_relations.base`. |
| `channel-relations.overrides` | string | A single channel reference with lower priority than this channel, written to `info.channel_relations.overrides`. |

`channel-relations.base` and `channel-relations.overrides` follow
[CEP-42 channel relationship metadata](https://github.com/conda/ceps/blob/main/cep-0042.md)
and must not point to the same channel.

### Common configurations

Advertise v3 repodata for all channels:

```toml
[index-config]
repodata-revisions = ["v3"]
```

Advertise v3 repodata and assign every package to the newest configured
revision for one specific channel:

```toml
[index-config."s3://my-bucket/staging"]
repodata-revisions = ["v3"]
package-revision-assignment = "latest"
```

Declare CEP-42 channel relationships:

```toml
[index-config."s3://my-bucket/my-channel".channel-relations]
base = "../conda-forge"
overrides = "../fallback"
```

The generated `repodata.json` then contains the metadata under `info`, for
example:

```json
{
  "info": {
    "base_url": "../packages/",
    "repodata_revisions": [
      {
        "revision": 3,
        "n_packages": 42
      }
    ],
    "channel_relations": {
      "base": "../conda-forge",
      "overrides": "../fallback"
    }
  }
}
```

The same channel metadata is also written into the sharded repodata index when
`write-shards` is enabled.
