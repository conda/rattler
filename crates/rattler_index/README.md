# rattler_index

`rattler_index` creates or updates conda channel indexes by writing
`repodata.json`, optional compressed repodata, and optional sharded repodata for
packages stored on a local filesystem or in S3.

## CLI Usage

Index a local channel:

```shell
rattler-index --channel-options ./channel-options.toml fs ./channel
```

Index an S3 channel:

```shell
rattler-index --channel-options ./channel-options.toml s3 s3://my-bucket/my-channel
```

The `--channel-options` file is TOML. Values from explicit CLI flags override
values from the TOML file. When a key is omitted from the TOML file and not
provided on the CLI, the normal `rattler-index` defaults are used.

`--channel-options` is separate from `--config`: the former describes metadata
and indexing behavior for the channel being written, while the latter loads the
normal rattler/pixi configuration for settings such as S3 access and
concurrency.

## Channel Options TOML

Only add the fields you need:

```toml
write-zst = true
write-shards = true
repodata-revisions = ["v3"]
package-revision-assignment = "latest"
base-url = "../packages/"

[channel-relations]
base = "../conda-forge"
overrides = "../fallback"
```

Supported fields:

| Field | Type | Description |
| --- | --- | --- |
| `write-zst` | boolean | Writes `repodata.json.zst`. Defaults to `true` if unset. |
| `write-shards` | boolean | Writes `repodata_shards.msgpack.zst` and shard files. Defaults to `true` if unset. |
| `repodata-revisions` | array | Repodata revisions to enable, for example `["v3"]` or `[3]`. The indexer fills revision package counts and timestamps while writing repodata. |
| `package-revision-assignment` | string | `from-index-json` assigns packages from their package metadata; `latest` assigns packages to the newest configured revision. Defaults to `from-index-json`. |
| `base-url` | string | Writes `info.base_url` in generated `repodata.json` and sharded repodata metadata. It may be relative or absolute and is written as provided. |
| `channel-relations.base` | string | A single channel reference with higher priority than this channel, written to `info.channel_relations.base`. |
| `channel-relations.overrides` | string | A single channel reference with lower priority than this channel, written to `info.channel_relations.overrides`. |

`channel-relations.base` and `channel-relations.overrides` are scalar channel
references, not arrays. They follow
[CEP-42 channel relationship metadata](https://github.com/conda/ceps/blob/main/cep-0042.md)
and must not point to the same channel.

TOML keys should use kebab-case. Snake-case aliases are accepted for
compatibility with Rust field names, but new files should prefer kebab-case.

## Common Configurations

Advertise v3 repodata and keep package revision assignment driven by each
package's `info/index.json`:

```toml
repodata-revisions = ["v3"]
```

Advertise v3 repodata and assign every package to the newest configured
revision:

```toml
repodata-revisions = ["v3"]
package-revision-assignment = "latest"
```

Declare package URLs relative to the subdir repodata location:

```toml
base-url = "../packages/"
```

Declare CEP-42 channel relationships:

```toml
[channel-relations]
base = "../conda-forge"
overrides = "../fallback"
```

The generated `repodata.json` contains the metadata under `info`, for example:

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
