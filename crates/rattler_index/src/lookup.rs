//! Writing the index of the file paths contained in the packages of a subdir.
//!
//! The index lives in `<subdir>/lookup/` and consists of a
//! `manifest.json` that lists content-addressed Parquet layers, see
//! [`rattler_lookup`]. Every run adds one layer with the packages that no
//! layer covers yet and merges all layers into one once there are more than
//! [`LookupOptions::compact_threshold`] of them.

use std::{collections::BTreeSet, sync::Arc};

use futures::{StreamExt, stream::FuturesUnordered};
use opendal::Operator;
use rattler_conda_types::{
    Platform,
    package::{DistArchiveIdentifier, DistArchiveType},
};
use rattler_lookup::{
    EntrySource, Kind, LayerBuilder, LayerFile, LayerWriter, Manifest, WriteOptions, layer_entries,
    manifest::{LOOKUP_DIR, MANIFEST_FILE},
    merge_entries, package_names,
};
use tokio::sync::Semaphore;

use crate::{
    CACHE_CONTROL_IMMUTABLE, CACHE_CONTROL_REPODATA, FilePaths, IndexedPackageRecord,
    LookupOptions, PreconditionChecks, RepodataFileMetadata, cache, error::RepodataError,
    read_and_parse_package,
};

/// The url of the manifest, relative to the repodata files of the subdir.
pub(crate) const LOOKUP_URL: &str = "./lookup/manifest.json";

/// The kinds of lookup tables this indexer writes: `paths` answers exact and
/// prefix lookups, `reversed-paths` answers `**/<name>` lookups.
const KINDS: [Kind; 2] = Kind::ALL;

/// The channel of the manifest, relative to the manifest itself. An indexer
/// does not necessarily know under which url its channel is served, and readers
/// resolve the field against the url of the manifest.
const RELATIVE_CHANNEL: &str = "../../";

/// Writes the lookup index of `subdir`.
///
/// Reads the paths of every package of `registered_packages` that no layer
/// covers yet, either from the record (when it was read during this run) or
/// from the archive, and publishes them as a new layer. Returns whether the
/// subdir has a manifest, i.e. whether the repodata should point at it.
pub(crate) async fn write_index(
    op: &Operator,
    subdir: Platform,
    registered_packages: &ahash::HashMap<DistArchiveIdentifier, IndexedPackageRecord>,
    options: &LookupOptions,
    precondition_checks: PreconditionChecks,
    semaphore: &Arc<Semaphore>,
    cache: &cache::PackageRecordCache,
) -> Result<bool, RepodataError> {
    let dir = format!("{subdir}/{LOOKUP_DIR}");
    let manifest_path = format!("{dir}/{MANIFEST_FILE}");

    // Collected before anything is read, so that a manifest another publisher
    // writes in the meantime is detected by the conditional write below and
    // makes `index_subdir` retry.
    let manifest_metadata =
        RepodataFileMetadata::new(op, &manifest_path, precondition_checks).await?;
    let previous = read_manifest(op, &manifest_path).await?;

    // An index without a `paths` table cannot be extended — its layers cannot be
    // read back — so it is replaced by a new one instead.
    let reusable = previous.as_ref().filter(|manifest| {
        manifest.has_kind(Kind::Paths) || {
            tracing::warn!("The lookup index of {subdir} has no `paths` table, rebuilding it");
            false
        }
    });

    // The layers of the previous manifest and the packages they cover. An
    // artifact is in at most one layer, so a package that is covered already is
    // not read again.
    let layers = reusable
        .map(|manifest| manifest.layers.clone())
        .unwrap_or_default();
    let mut layer_packages = Vec::with_capacity(layers.len());
    for layer in &layers {
        let bytes = op.read(&format!("{dir}/{}", layer.packages.file)).await?;
        layer_packages.push(Arc::new(
            package_names(bytes.to_bytes()).map_err(anyhow::Error::new)?,
        ));
    }
    let covered: BTreeSet<String> = layer_packages
        .iter()
        .flat_map(|names| names.iter().cloned())
        .collect();

    // Only conda archives have an `info/paths.json`; other artifact types are
    // not covered by the CEP.
    let uploaded: BTreeSet<String> = registered_packages
        .keys()
        .filter(|filename| matches!(filename.archive_type, DistArchiveType::Conda(_)))
        .map(DistArchiveIdentifier::to_file_name)
        .collect();

    // Packages that were removed from the channel stay in the layer that
    // indexes them, so the manifest lists them as removed instead. Filenames
    // that are not covered by any layer, e.g. because a previous compaction
    // dropped them, are forgotten, and so are filenames that were uploaded
    // again (their old layer describes the new artifact).
    let mut removed = reusable
        .map(|manifest| manifest.removed.clone())
        .unwrap_or_default();
    removed.extend(covered.difference(&uploaded).cloned());
    removed.retain(|filename| covered.contains(filename) && !uploaded.contains(filename));

    // The packages this run indexes. A package whose paths could not be read is
    // not in here, so the next run tries it again.
    let mut indexed = BTreeSet::new();
    let mut builder = LayerBuilder::new(RELATIVE_CHANNEL, subdir.as_str());
    let mut to_read = Vec::new();
    for (filename, record) in registered_packages {
        let name = filename.to_file_name();
        if !uploaded.contains(&name) || covered.contains(&name) {
            continue;
        }
        match &record.file_paths {
            Some(paths) => {
                builder.add_package(name.clone(), paths.iter().cloned());
                indexed.insert(name);
            }
            // Registered by an earlier run, so its paths were never read.
            None => to_read.push(name),
        }
    }
    for (name, paths) in read_paths(op, subdir, to_read, semaphore, cache).await? {
        builder.add_package(name.clone(), paths);
        indexed.insert(name);
    }

    // Every layer of an index has to have a table of every kind the manifest
    // lists, so an index of other kinds is rewritten rather than appended to.
    let kinds_differ = reusable.is_some_and(|manifest| {
        manifest.kinds.len() != KINDS.len() || !KINDS.iter().all(|kind| manifest.has_kind(*kind))
    });

    // Compaction keeps the number of files a reader has to open bounded. The
    // layer this run adds counts towards the threshold.
    let num_layers = layers.len() + usize::from(!indexed.is_empty());
    let compact = (options.compact || kinds_differ || num_layers > options.compact_threshold)
        && num_layers > 0;

    let new_layer = if compact {
        tracing::info!("Compacting the {num_layers} layers of the lookup index of {subdir}");
        // Only what is still uploaded and actually indexed goes into the
        // compacted layer; everything else is dropped rather than carried along
        // in `removed`.
        let packages: BTreeSet<&String> =
            covered.difference(&removed).chain(indexed.iter()).collect();
        let mut sources: Vec<EntrySource> = Vec::with_capacity(layers.len() + 1);
        for (layer, names) in layers.iter().zip(&layer_packages) {
            let table = layer
                .table(Kind::Paths)
                .ok_or_else(|| anyhow::anyhow!("a layer of {manifest_path} has no paths table"))?;
            let bytes = op.read(&format!("{dir}/{}", table.file)).await?;
            sources.push(Box::new(
                layer_entries(bytes.to_bytes(), names.clone()).map_err(anyhow::Error::new)?,
            ));
        }
        sources.push(Box::new(builder.into_entries()));

        if packages.is_empty() {
            None
        } else {
            let mut writer = LayerWriter::new(
                RELATIVE_CHANNEL,
                subdir.as_str(),
                packages,
                &KINDS,
                &WriteOptions::default(),
            )
            .map_err(anyhow::Error::new)?;
            for entry in merge_entries(sources, &removed) {
                let (path, packages) = entry.map_err(anyhow::Error::new)?;
                writer.push(&path, &packages).map_err(anyhow::Error::new)?;
            }
            Some(writer.finish().map_err(anyhow::Error::new)?)
        }
    } else {
        builder
            .finish_with_kinds(&KINDS, &WriteOptions::default())
            .map_err(anyhow::Error::new)?
    };

    let mut manifest = Manifest::empty(RELATIVE_CHANNEL, subdir.as_str(), KINDS);
    if compact {
        // The compacted layer holds every artifact that is still uploaded, so
        // nothing is left to exclude.
        manifest.layers = new_layer
            .iter()
            .map(rattler_lookup::LayerFiles::manifest_layer)
            .collect();
    } else {
        manifest.layers = layers;
        manifest.layers.extend(
            new_layer
                .iter()
                .map(rattler_lookup::LayerFiles::manifest_layer),
        );
        manifest.removed = removed;
    }

    if let Some(previous) = &previous
        && previous.kinds == manifest.kinds
        && previous.layers == manifest.layers
        && previous.removed == manifest.removed
    {
        tracing::debug!("The lookup index of {subdir} is up to date");
        return Ok(true);
    }

    if let Some(files) = &new_layer {
        tracing::info!(
            "Adding {} paths of {} packages to the lookup index of {subdir}",
            files.num_paths,
            files.num_packages
        );
        for file in files.files() {
            write_layer_file(op, &dir, file).await?;
        }
    }

    tracing::info!("Writing the lookup index manifest to {manifest_path}");
    crate::utils::write_with_metadata_check(
        op,
        &manifest_path,
        manifest.to_bytes()?,
        &manifest_metadata,
        Some(CACHE_CONTROL_REPODATA),
    )
    .await?;

    Ok(true)
}

/// Reads the manifest of the index, or `None` if the subdir has none yet.
async fn read_manifest(op: &Operator, path: &str) -> Result<Option<Manifest>, RepodataError> {
    let bytes = match op.read(path).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == opendal::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    // Rewriting an index that cannot be understood would silently drop it.
    let manifest = Manifest::from_bytes(&bytes.to_bytes()).map_err(|e| {
        anyhow::Error::new(e).context(format!("invalid lookup index manifest {path}"))
    })?;
    Ok(Some(manifest))
}

/// Stores one file of a layer under its content-addressed name.
async fn write_layer_file(
    op: &Operator,
    dir: &str,
    file: &LayerFile,
) -> Result<(), opendal::Error> {
    let path = format!("{dir}/{}", file.file_name);
    tracing::trace!("Writing a lookup index layer to {path}");
    match op
        .write_with(&path, file.bytes.clone())
        .if_not_exists(true)
        .cache_control(CACHE_CONTROL_IMMUTABLE)
        .await
    {
        // The name contains the hash of the contents, so the file is identical.
        Err(e) if e.kind() == opendal::ErrorKind::ConditionNotMatch => {
            tracing::trace!("{path} already exists");
            Ok(())
        }
        Ok(_metadata) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Reads the paths of packages that are not covered by a layer yet.
///
/// A package whose paths cannot be read is skipped entirely instead of being
/// indexed as empty, so that the next run tries again.
async fn read_paths(
    op: &Operator,
    subdir: Platform,
    filenames: Vec<String>,
    semaphore: &Arc<Semaphore>,
    cache: &cache::PackageRecordCache,
) -> Result<Vec<(String, Vec<String>)>, RepodataError> {
    if filenames.is_empty() {
        return Ok(Vec::new());
    }
    tracing::info!(
        "Reading the paths of {} packages of {subdir} that are not indexed yet",
        filenames.len()
    );

    let mut tasks = FuturesUnordered::new();
    for filename in filenames {
        let op = op.clone();
        let cache = cache.clone();
        let semaphore = semaphore.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = semaphore
                .acquire()
                .await
                .expect("Semaphore was unexpectedly closed");
            let record =
                read_and_parse_package(&op, &cache, subdir, &filename, FilePaths::Read).await;
            (filename, record)
        }));
    }

    let mut paths = Vec::new();
    while let Some(joined) = tasks.next().await {
        let (filename, record) = joined?;
        match record {
            Ok(record) => {
                if let Some(file_paths) = record.file_paths {
                    paths.push((filename, file_paths.to_vec()));
                } else {
                    tracing::warn!(
                        "{subdir}/{filename} lists no paths, leaving it out of the lookup index"
                    );
                }
            }
            Err(e) => tracing::warn!(
                "Failed to read the paths of {subdir}/{filename}, leaving it out of the lookup index: {e}"
            ),
        }
    }
    Ok(paths)
}
