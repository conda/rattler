# rattler_lookup

Read and write the *lookup index* of a conda channel: static, content-addressed
[Apache Parquet](https://parquet.apache.org/) files in `<subdir>/lookup/` that
map the files shipped by a channel's packages to the packages containing them.
A lookup needs a handful of HTTP range requests and never downloads the index.

The index of a subdir consists of `manifest.json` (the only file that changes)
and *layers*. Every layer has a packages file (the filenames of the artifacts
it indexes) and one lookup table per *kind*, `paths` and `reversed-paths`,
whose sorted, unique keys map to row numbers in the packages file. The
repodata points to the manifest with `info.lookup_url`.

## Querying

```rust,no_run
use rattler_lookup::{discovery, Kind, Location, Query, SubdirIndex};

# async fn example() -> Result<(), rattler_lookup::LookupError> {
let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
let channel = Location::parse("https://conda.anaconda.org/conda-forge/");

// `lookup_url` from the repodata tells where the manifest of a subdir is.
let manifest = discovery::discover_manifest(&channel, "linux-64", &client)
    .await?
    .expect("the channel publishes a lookup index");
let mut index = SubdirIndex::open(manifest, &Kind::ALL, &client).await?;

// A path, or a pattern such as `**/zlib.h`, `**/libssl.so*` or
// `site-packages/polars/*` (the literal start of the pattern is scanned).
let matches = index.query(&Query::parse("**/include/zlib.h")?, Some(1000)).await?;
for (path, filenames) in &matches.paths {
    println!("{path}: {}", filenames.len());
}
# Ok(())
# }
```

`rattler whoprovides` is a command line interface to this, and `rattler-index
--write-lookup` writes the index of a channel.

## Writing

`write_layer` writes the files of a layer for a set of artifacts and their
paths; the returned `Layer` goes into the manifest. `bulk` reads complete layer
files back, e.g. to merge layers into a new base layer.
