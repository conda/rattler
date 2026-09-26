//! A subdir index: the manifest and the layers it lists, queried together.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use futures::future::try_join_all;
use reqwest_middleware::ClientWithMiddleware;

use crate::{
    Kind, Location, LookupError, Manifest, Query,
    fetch::fetch_optional,
    query::PathPattern,
    table::{LookupTable, PackagesFile},
};

impl Manifest {
    /// Fetches and parses the manifest at a location. Returns `None` if there
    /// is no manifest.
    pub async fn fetch(
        location: &Location,
        client: &ClientWithMiddleware,
    ) -> Result<Option<Self>, LookupError> {
        match fetch_optional(location, client).await? {
            Some(bytes) => Self::parse(&bytes, location).map(Some),
            None => Ok(None),
        }
    }
}

/// The opened tables of one layer.
struct OpenLayer {
    tables: HashMap<Kind, LookupTable>,
    packages: PackagesFile,
}

impl OpenLayer {
    /// Turns the package ids of rows into filenames.
    async fn resolve(
        &mut self,
        rows: Vec<(String, Vec<u32>)>,
    ) -> Result<Vec<(String, Vec<String>)>, LookupError> {
        let ids = rows.iter().flat_map(|(_, ids)| ids.iter().copied());
        let resolved = self.packages.resolve(ids).await?;
        Ok(rows
            .into_iter()
            .map(|(key, ids)| {
                let filenames = ids.iter().map(|id| resolved[id].clone()).collect();
                (key, filenames)
            })
            .collect())
    }

    fn stats(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.tables
            .values()
            .map(LookupTable::stats)
            .chain([self.packages.stats()])
    }
}

/// The result of a query in one subdir: the matching paths (one for an exact
/// query) and the filenames of the artifacts containing each of them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Matches {
    /// Path -> filenames, both sorted.
    pub paths: BTreeMap<String, BTreeSet<String>>,
    /// The scan stopped at the limit; there may be more matching paths.
    pub truncated: bool,
}

impl Matches {
    /// All filenames, without duplicates.
    pub fn filenames(&self) -> BTreeSet<&str> {
        self.paths
            .values()
            .flat_map(|filenames| filenames.iter().map(String::as_str))
            .collect()
    }

    /// Whether nothing matched.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// A subdir index that has been opened for querying: the footers of the
/// needed tables and of the packages files of all layers have been read.
pub struct SubdirIndex {
    location: Location,
    manifest: Manifest,
    layers: Vec<OpenLayer>,
    removed: HashSet<String>,
}

impl std::fmt::Debug for SubdirIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubdirIndex")
            .field("location", &self.location)
            .field("manifest", &self.manifest)
            .field("layers", &self.layers.len())
            .finish_non_exhaustive()
    }
}

impl SubdirIndex {
    /// Opens the index whose manifest is at `location`, with the tables of the
    /// given kinds. Every layer's files are opened concurrently.
    pub async fn open(
        location: Location,
        kinds: &[Kind],
        client: &ClientWithMiddleware,
    ) -> Result<Self, LookupError> {
        let manifest = Manifest::fetch(&location, client)
            .await?
            .ok_or_else(|| LookupError::NotFound(location.clone()))?;
        Self::from_manifest(location, manifest, kinds, client).await
    }

    /// Opens the layers of an already fetched manifest.
    pub async fn from_manifest(
        location: Location,
        manifest: Manifest,
        kinds: &[Kind],
        client: &ClientWithMiddleware,
    ) -> Result<Self, LookupError> {
        for &kind in kinds {
            if !manifest.has_kind(kind) {
                return Err(LookupError::MissingKind {
                    location: location.clone(),
                    kind,
                });
            }
        }
        let layers = try_join_all(manifest.layers.iter().map(|layer| {
            let location = &location;
            async move {
                let tables = try_join_all(
                    kinds
                        .iter()
                        .map(|&kind| LookupTable::open_layer(location, layer, kind, client)),
                );
                let packages = PackagesFile::open_layer(location, layer, client);
                let (tables, packages) = futures::try_join!(tables, packages)?;
                Ok::<_, LookupError>(OpenLayer {
                    tables: kinds.iter().copied().zip(tables).collect(),
                    packages,
                })
            }
        }))
        .await?;
        Ok(Self {
            removed: manifest.removed.iter().cloned().collect(),
            location,
            manifest,
            layers,
        })
    }

    /// The location of the manifest.
    pub fn location(&self) -> &Location {
        &self.location
    }

    /// The manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The subdir.
    pub fn subdir(&self) -> &str {
        &self.manifest.subdir
    }

    /// The base URL of the indexed channel.
    pub fn channel(&self) -> &str {
        &self.manifest.channel
    }

    /// Requests and bytes read from the layer files so far.
    pub fn stats(&self) -> (u64, u64) {
        self.layers
            .iter()
            .flat_map(OpenLayer::stats)
            .fold((0, 0), |(r, b), (r2, b2)| (r + r2, b + b2))
    }

    /// Answers a query. For a pattern, at most `limit` matching paths (in
    /// key order) are returned.
    pub async fn query(
        &mut self,
        query: &Query,
        limit: Option<usize>,
    ) -> Result<Matches, LookupError> {
        match query {
            Query::Path(path) => self.find(path).await,
            Query::Pattern(pattern) => self.scan(pattern, limit).await,
        }
    }

    /// The artifacts containing `path`.
    pub async fn find(&mut self, path: &str) -> Result<Matches, LookupError> {
        let location = &self.location;
        let found = try_join_all(self.layers.iter_mut().map(|layer| async move {
            let table =
                layer
                    .tables
                    .get_mut(&Kind::Paths)
                    .ok_or_else(|| LookupError::MissingKind {
                        location: location.clone(),
                        kind: Kind::Paths,
                    })?;
            let rows: Vec<_> = table
                .find(path)
                .await?
                .map(|ids| (path.to_string(), ids))
                .into_iter()
                .collect();
            layer.resolve(rows).await
        }))
        .await?;
        let filenames: BTreeSet<String> = found
            .into_iter()
            .flatten()
            .flat_map(|(_, filenames)| filenames)
            .filter(|filename| !self.removed.contains(filename))
            .collect();
        let mut matches = Matches::default();
        if !filenames.is_empty() {
            matches.paths.insert(path.to_string(), filenames);
        }
        Ok(matches)
    }

    /// The first `limit` paths (in key order) matching a pattern, with the
    /// artifacts containing them.
    pub async fn scan(
        &mut self,
        pattern: &PathPattern,
        limit: Option<usize>,
    ) -> Result<Matches, LookupError> {
        let kind = pattern.kind();
        let location = &self.location;
        let scans = try_join_all(self.layers.iter_mut().map(|layer| async move {
            let table = layer
                .tables
                .get_mut(&kind)
                .ok_or_else(|| LookupError::MissingKind {
                    location: location.clone(),
                    kind,
                })?;
            let scan = table
                .scan(pattern.key_range(), |key| pattern.matches_key(key), limit)
                .await?;
            let rows = layer.resolve(scan.rows).await?;
            Ok::<_, LookupError>((rows, scan.truncated))
        }))
        .await?;

        // Every layer returned its first `limit` keys, so the first `limit`
        // keys of the union are the first `limit` keys overall.
        let mut truncated = false;
        let mut by_key: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (rows, layer_truncated) in scans {
            truncated |= layer_truncated;
            for (key, filenames) in rows {
                let filenames = filenames
                    .into_iter()
                    .filter(|filename| !self.removed.contains(filename));
                by_key.entry(key).or_default().extend(filenames);
            }
        }
        by_key.retain(|_, filenames| !filenames.is_empty());
        if limit.is_some_and(|limit| by_key.len() > limit) {
            truncated = true;
        }
        let paths = by_key
            .into_iter()
            .take(limit.unwrap_or(usize::MAX))
            .map(|(key, filenames)| (kind.path_of(&key), filenames))
            .collect();
        Ok(Matches { paths, truncated })
    }
}
