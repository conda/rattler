# rattler_index

`rattler_index` creates or updates conda channel indexes by writing
`repodata.json`, optional compressed repodata, and optional sharded repodata for
packages stored on a local filesystem or in S3.

## CLI Usage

Index a local channel:

```shell
rattler-index --config ./rattler-config.toml fs ./channel
```

Index an S3 channel:

```shell
rattler-index --config ./rattler-config.toml s3 s3://my-bucket/my-channel
```

The `--config` flag points at the same TOML configuration file used by pixi. It
configures S3 credentials, concurrency, and per-channel index options under the
`[index-config]` section.

When `--config` is omitted, `rattler-index` falls back to its built-in defaults
(`write-zst = true`, `write-shards = true`, no advertised repodata revisions,
`from-index-json` revision assignment, no channel metadata).

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
| `repodata-revisions` | array | Repodata revisions to enable, e.g. `["v3"]`. Each entry may be the string form (`"v3"`, `"legacy"`) or an integer (`3`). The indexer fills revision package counts and timestamps while writing repodata. |
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
