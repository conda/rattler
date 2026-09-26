//! Writes the *lookup index* of a subdir (which artifacts contain a file)
//! next to its repodata, in `<subdir>/lookup/`, using [`rattler_lookup`].
//!
//! Every run adds a *layer* with the artifacts that are not indexed yet and
//! lists the artifacts that disappeared from the subdir in the manifest's
//! `removed`. Once an index has [`crate::MAX_LOOKUP_LAYERS`] layers (or when indexing is
//! forced), all layers are merged into a new base layer. Layer files are
//! content-addressed and immutable; files of superseded layers are left in
//! place so that clients with a cached manifest can finish their lookups.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Read,
};

use opendal::Operator;
use rattler_conda_types::{Platform, package::DistArchiveIdentifier};
use rattler_lookup::{
    Kind, Location, Manifest, WriteOptions, bulk,
    manifest::{LOOKUP_DIR, manifest_path},
    write_layer, write_layer_sorted,
};

use crate::{
    CACHE_CONTROL_IMMUTABLE, CACHE_CONTROL_REPODATA, LookupStats, RepodataFileMetadata,
    error::RepodataError,
};

/// The kinds of tables the index provides.
const KINDS: [Kind; 2] = [Kind::Paths, Kind::ReversedPaths];

/// Chunks in which layer files are uploaded.
const UPLOAD_CHUNK: usize = 8 << 20;

/// The existing index of a subdir: its manifest and the artifacts its layers
/// list.
pub(crate) struct ExistingLookup {
    pub manifest: Manifest,
    /// The filenames of every layer, in package id order.
    pub layer_packages: Vec<Vec<String>>,
}

impl ExistingLookup {
    /// The filenames of all artifacts in any layer.
    pub fn indexed(&self) -> HashSet<&str> {
        self.layer_packages
            .iter()
            .flatten()
            .map(String::as_str)
            .collect()
    }
}

/// Reads the manifest of a subdir and the packages files of its layers.
/// Returns `None` if the subdir has no index.
pub(crate) async fn read_existing(
    op: &Operator,
    subdir: Platform,
    metadata: &RepodataFileMetadata,
) -> Result<Option<ExistingLookup>, RepodataError> {
    let path = manifest_path(subdir.as_str());
    let location = Location::parse(&path);
    let bytes = match crate::utils::read_with_metadata_check(op, &path, metadata).await {
        Ok(bytes) => bytes.to_vec(),
        Err(err) if err.kind() == opendal::ErrorKind::NotFound => {
            tracing::info!("Could not find {path}. Creating a new lookup index.");
            return Ok(None);
        }
        Err(err) => return Err(err.into()),
    };
    let manifest = Manifest::parse(&bytes, &location).map_err(anyhow::Error::from)?;
    let mut layer_packages = Vec::with_capacity(manifest.layers.len());
    for layer in &manifest.layers {
        let bytes = read_layer_file(op, subdir, &layer.packages.file, layer.packages.size).await?;
        let packages =
            bulk::read_packages(bytes, &layer_file_location(subdir, &layer.packages.file))
                .await
                .map_err(anyhow::Error::from)?;
        layer_packages.push(packages);
    }
    Ok(Some(ExistingLookup {
        manifest,
        layer_packages,
    }))
}

fn layer_file_location(subdir: Platform, file: &str) -> Location {
    Location::parse(&format!("{subdir}/{LOOKUP_DIR}/{file}"))
}

/// Reads a complete layer file and verifies its size and SHA-256.
async fn read_layer_file(
    op: &Operator,
    subdir: Platform,
    file: &str,
    expected_size: u64,
) -> Result<bytes::Bytes, RepodataError> {
    let location = layer_file_location(subdir, file);
    let bytes = op.read(&location.to_string()).await?.to_bytes();
    if bytes.len() as u64 != expected_size {
        return Err(RepodataError::Other(anyhow::anyhow!(
            "{location} has {} bytes, but the manifest expects {expected_size}",
            bytes.len()
        )));
    }
    bulk::verify_sha256(file, &bytes, &location).map_err(anyhow::Error::from)?;
    Ok(bytes)
}

/// Updates the index of a subdir.
///
/// * `uploaded`: the artifacts that currently exist in the subdir.
/// * `new_paths`: the paths of every uploaded artifact that is not in the
///   existing index (or of all uploaded artifacts if there is none).
/// * `compact`: merge the existing layers and the new artifacts into one
///   base layer instead of adding a layer.
///
/// The layer files are uploaded first, then the manifest (with a conditional
/// write against `manifest_metadata`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn write_index(
    op: &Operator,
    subdir: Platform,
    channel: &str,
    existing: Option<ExistingLookup>,
    uploaded: &HashSet<DistArchiveIdentifier>,
    new_paths: HashMap<String, Vec<String>>,
    compact: bool,
    manifest_metadata: &RepodataFileMetadata,
) -> Result<LookupStats, RepodataError> {
    let uploaded: HashSet<String> = uploaded
        .iter()
        .map(DistArchiveIdentifier::to_file_name)
        .collect();
    let options = WriteOptions::default();
    let temp_dir = tempfile::tempdir()?;
    let mut stats = LookupStats::default();

    let (mut manifest, layer_packages) = match existing {
        Some(existing) => (existing.manifest, existing.layer_packages),
        None => (
            Manifest::empty(channel, subdir.as_str(), &KINDS),
            Vec::new(),
        ),
    };
    manifest.channel = channel.to_string();
    manifest.created_at = rattler_lookup::manifest::now_rfc3339();
    // An index gains kinds with a new base layer, so an existing index with
    // other kinds is compacted.
    let compact = compact || KINDS.iter().any(|kind| !manifest.has_kind(*kind));

    let written = if compact {
        stats.compacted = true;
        // The artifacts of the new base layer: everything that is uploaded
        // and either indexed already or fetched now.
        let mut packages: Vec<String> = layer_packages
            .iter()
            .flatten()
            .filter(|filename| uploaded.contains(*filename))
            .cloned()
            .chain(new_paths.keys().cloned())
            .collect();
        packages.sort_unstable();
        packages.dedup();
        let id_of: HashMap<&str, u32> = packages
            .iter()
            .enumerate()
            .map(|(id, filename)| (filename.as_str(), id as u32))
            .collect();

        // Merge the paths tables of the existing layers, translating their
        // package ids into the new ones and dropping removed artifacts.
        let mut paths: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        for (layer, old_packages) in manifest.layers.iter().zip(&layer_packages) {
            let Some(table) = layer.table(Kind::Paths) else {
                continue;
            };
            let remap: Vec<Option<u32>> = old_packages
                .iter()
                .map(|filename| id_of.get(filename.as_str()).copied())
                .collect();
            let bytes = read_layer_file(op, subdir, &table.file, table.size).await?;
            let rows = bulk::read_table(
                bytes,
                Kind::Paths,
                &layer_file_location(subdir, &table.file),
            )
            .await
            .map_err(anyhow::Error::from)?;
            for (path, ids) in rows {
                let ids = ids
                    .into_iter()
                    .filter_map(|id| remap.get(id as usize).copied().flatten());
                paths.entry(path).or_default().extend(ids);
            }
        }
        for (filename, artifact_paths) in &new_paths {
            let id = id_of[filename.as_str()];
            for path in artifact_paths {
                paths.entry(path.clone()).or_default().push(id);
            }
        }
        for ids in paths.values_mut() {
            ids.sort_unstable();
            ids.dedup();
        }
        paths.retain(|_, ids| !ids.is_empty());

        stats.packages_added = new_paths.len();
        manifest.layers.clear();
        manifest.removed.clear();
        manifest.kinds = KINDS.iter().map(|kind| kind.name().to_string()).collect();
        if packages.is_empty() {
            None
        } else {
            let written = tokio::task::block_in_place(|| {
                write_layer_sorted(
                    temp_dir.path(),
                    channel,
                    subdir.as_str(),
                    &KINDS,
                    &packages,
                    paths,
                    &options,
                )
            })
            .map_err(anyhow::Error::from)?;
            Some(written)
        }
    } else {
        // Artifacts that are in a layer but no longer in the subdir.
        let mut removed: Vec<String> = layer_packages
            .iter()
            .flatten()
            .filter(|filename| !uploaded.contains(*filename))
            .cloned()
            .collect();
        removed.sort_unstable();
        removed.dedup();
        manifest.removed = removed;
        stats.packages_added = new_paths.len();
        if new_paths.is_empty() {
            None
        } else {
            let artifacts: Vec<(String, Vec<String>)> = new_paths.into_iter().collect();
            let written = tokio::task::block_in_place(|| {
                write_layer(
                    temp_dir.path(),
                    channel,
                    subdir.as_str(),
                    &KINDS,
                    artifacts,
                    &options,
                )
            })
            .map_err(anyhow::Error::from)?;
            Some(written)
        }
    };

    if let Some(written) = written {
        // The layer files first: they are immutable and content-addressed,
        // so a file that exists already has the same content.
        for file in &written.files {
            let path = format!("{subdir}/{LOOKUP_DIR}/{}", file.name);
            if op.exists(&path).await? {
                tracing::debug!("{path} already exists");
                continue;
            }
            tracing::info!("Writing lookup layer file {path} ({} bytes)", file.size);
            upload_file(op, &path, &file.path).await?;
        }
        manifest.layers.push(written.layer);
    }

    stats.packages_removed = manifest.removed.len();
    stats.layers = manifest.layers.len();

    // The manifest last: it makes the layers live.
    let manifest_path = manifest_path(subdir.as_str());
    tracing::info!("Writing lookup manifest to {manifest_path}");
    crate::utils::write_with_metadata_check(
        op,
        &manifest_path,
        manifest.to_json().map_err(anyhow::Error::from)?,
        manifest_metadata,
        Some(CACHE_CONTROL_REPODATA),
    )
    .await?;
    Ok(stats)
}

/// Uploads a local file in chunks.
async fn upload_file(
    op: &Operator,
    path: &str,
    local: &std::path::Path,
) -> Result<(), RepodataError> {
    let mut file = fs_err::File::open(local)?;
    let mut writer = op
        .writer_with(path)
        .cache_control(CACHE_CONTROL_IMMUTABLE)
        .content_type("application/vnd.apache.parquet")
        .await?;
    let mut buf = vec![0u8; UPLOAD_CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write(buf[..n].to_vec()).await?;
    }
    writer.close().await?;
    Ok(())
}
