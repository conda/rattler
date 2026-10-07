//! Source enum and `RepoDataSource` trait for custom repodata providers.

use std::{collections::HashSet, sync::Arc};

use rattler_conda_types::{Channel, ChannelUrl, PackageName, RepoDataRecord, Subdir};
use thiserror::Error;

use super::{
    GatewayError,
    subdir::{PackageRecords, SubdirClient, extract_unique_deps_split},
};
use crate::{Reporter, sparse::SparseRepoData};

/// A source of repodata records for a specific subdirectory.
///
/// Implement this trait to provide custom repodata records without
/// going through traditional channel URLs. The gateway will call
/// these methods for each platform in the query.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait RepoDataSource: Send + Sync {
    /// Fetch records for a specific package name and platform.
    ///
    /// This method is called by the gateway when it needs repodata records
    /// for a particular package. The platform parameter indicates which
    /// subdirectory the gateway is querying for.
    async fn fetch_package_records(
        &self,
        platform: Subdir,
        name: &PackageName,
    ) -> Result<Vec<Arc<RepoDataRecord>>, GatewayError>;

    /// Return all available package names for the given platform.
    ///
    /// This is used by the gateway to know which packages are available
    /// in this source for a given platform/subdirectory.
    fn package_names(&self, platform: Subdir) -> Vec<String>;
}

/// A source of repodata, either a channel or a custom source.
///
/// This enum allows the [`Gateway::query()`](super::Gateway::query) method
/// to accept both traditional channels custom repodata sources and sparse repodata.
#[derive(Clone)]
pub enum Source {
    /// A traditional conda channel (expanded to all requested platforms).
    Channel(Channel),

    /// A custom repodata source (provides records for requested platforms).
    Custom(Arc<dyn RepoDataSource>),

    /// A sparse repodata source (provides records for requested platforms from sparse
    /// repodata). Each entry represents a different subdir.
    SparseRepoData(Vec<Arc<SparseRepoData>>),

    /// A named group of sources that is requested as a whole, like a
    /// multichannel of conda. See [`MultiSource`].
    Multi(MultiSource),
}

impl From<Channel> for Source {
    fn from(channel: Channel) -> Self {
        Source::Channel(channel)
    }
}

impl From<MultiSource> for Source {
    fn from(multi_source: MultiSource) -> Self {
        Source::Multi(multi_source)
    }
}

impl From<Arc<dyn RepoDataSource>> for Source {
    fn from(source: Arc<dyn RepoDataSource>) -> Self {
        Source::Custom(source)
    }
}

impl From<Arc<SparseRepoData>> for Source {
    fn from(source: Arc<SparseRepoData>) -> Self {
        Source::SparseRepoData(vec![source])
    }
}

impl From<Vec<Arc<SparseRepoData>>> for Source {
    fn from(sources: Vec<Arc<SparseRepoData>>) -> Self {
        Source::SparseRepoData(sources)
    }
}

/// A named, ordered group of sources that is requested as a whole, like the
/// `defaults` channel or the `custom_multichannels` of conda.
///
/// Every source of the group is queried as if it was passed on its own, and
/// the resulting [`RepoData`](super::RepoData) is marked with the name of the
/// group (see [`RepoData::multi_channel`](super::RepoData::multi_channel)).
/// Records keep referring to the channel they came from. When solving, the
/// sources of a group share a single channel priority tier, and their order
/// only breaks ties between otherwise identical packages.
///
/// Channels that a member pulls in through [CEP-42] `channel_relations` do
/// not join the group. A `base` of any member is placed before the whole
/// group and an `overrides` target after it, so the group stays contiguous.
///
/// [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md
#[derive(Clone)]
pub struct MultiSource {
    name: Arc<str>,
    sources: Vec<Source>,
}

impl MultiSource {
    /// Creates a group called `name` from its sources, in order of
    /// preference.
    ///
    /// Returns an error if `sources` is empty, contains another
    /// [`MultiSource`], or contains the same channel or the same subdirectory
    /// of sparse repodata more than once.
    pub fn new(name: impl Into<Arc<str>>, sources: Vec<Source>) -> Result<Self, MultiSourceError> {
        let name = name.into();
        if sources.is_empty() {
            return Err(MultiSourceError::Empty {
                name: name.to_string(),
            });
        }

        let mut channels = HashSet::new();
        let mut subdirs = HashSet::new();
        for source in &sources {
            match source {
                Source::Channel(channel) => {
                    if !channels.insert(&channel.base_url) {
                        return Err(MultiSourceError::DuplicateChannel {
                            name: name.to_string(),
                            channel: Box::new(channel.base_url.clone()),
                        });
                    }
                }
                Source::SparseRepoData(sparse_list) => {
                    for sparse in sparse_list {
                        if !subdirs.insert((&sparse.channel.base_url, sparse.subdir())) {
                            return Err(MultiSourceError::DuplicateSubdir {
                                name: name.to_string(),
                                channel: Box::new(sparse.channel.base_url.clone()),
                                subdir: sparse.subdir().to_string(),
                            });
                        }
                    }
                }
                // Custom sources cannot be compared, so they are never
                // considered duplicates.
                Source::Custom(_) => {}
                Source::Multi(nested) => {
                    return Err(MultiSourceError::Nested {
                        name: name.to_string(),
                        nested: nested.name.to_string(),
                    });
                }
            }
        }

        Ok(Self { name, sources })
    }

    /// Returns the name of the group.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the sources of the group, in order of preference.
    pub fn sources(&self) -> &[Source] {
        &self.sources
    }
}

/// An error that can occur when constructing a [`MultiSource`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MultiSourceError {
    /// The group does not contain any sources.
    #[error("multichannel '{name}' does not contain any sources")]
    Empty {
        /// The name of the group.
        name: String,
    },

    /// The group contains another group.
    #[error("multichannel '{name}' cannot contain another multichannel ('{nested}')")]
    Nested {
        /// The name of the group.
        name: String,
        /// The name of the group it contains.
        nested: String,
    },

    /// The group contains the same channel more than once.
    #[error("multichannel '{name}' contains '{channel}' more than once")]
    DuplicateChannel {
        /// The name of the group.
        name: String,
        /// The channel that occurs more than once.
        channel: Box<ChannelUrl>,
    },

    /// The group contains the same subdirectory of sparse repodata more than
    /// once.
    #[error("multichannel '{name}' contains '{subdir}' of '{channel}' more than once")]
    DuplicateSubdir {
        /// The name of the group.
        name: String,
        /// The channel of the subdirectory.
        channel: Box<ChannelUrl>,
        /// The subdirectory that occurs more than once.
        subdir: String,
    },
}

/// A [`Source`] with every [`MultiSource`] replaced by its sources, each
/// carrying the name of the group it came from.
#[derive(Clone)]
pub(super) enum ExpandedSource {
    Channel(Channel, Option<Arc<str>>),
    Custom(Arc<dyn RepoDataSource>, Option<Arc<str>>),
    SparseRepoData(Vec<Arc<SparseRepoData>>, Option<Arc<str>>),
}

/// Where an [`ExpandedSource`] sits in the caller's list of sources.
///
/// Ordering by position keeps the order of the caller's sources and, within
/// a group, the order of its sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct SourcePosition {
    /// Index in the caller's list of sources.
    pub(super) source: usize,
    /// Index within the group at `source`, or `0` if it is not a group.
    pub(super) member: usize,
}

impl ExpandedSource {
    /// Replaces every group in `sources` by its sources, keeping the order of
    /// the sources and of the sources within a group.
    pub(super) fn expand(sources: impl IntoIterator<Item = Source>) -> Vec<(SourcePosition, Self)> {
        let mut expanded = Vec::new();
        for (index, source) in sources.into_iter().enumerate() {
            let first = expanded.len();
            Self::push(&mut expanded, index, first, source, None);
        }
        expanded
    }

    /// Pushes `source`, the caller's source at `index`, onto `expanded`,
    /// whose entries for that source start at `first`.
    fn push(
        expanded: &mut Vec<(SourcePosition, Self)>,
        index: usize,
        first: usize,
        source: Source,
        group: Option<Arc<str>>,
    ) {
        let position = SourcePosition {
            source: index,
            member: expanded.len() - first,
        };
        match source {
            Source::Channel(channel) => {
                expanded.push((position, ExpandedSource::Channel(channel, group)));
            }
            Source::Custom(custom) => {
                expanded.push((position, ExpandedSource::Custom(custom, group)));
            }
            Source::SparseRepoData(sparse) => {
                expanded.push((position, ExpandedSource::SparseRepoData(sparse, group)));
            }
            // `MultiSource::new` rejects nested groups, so `group` is always
            // `None` here.
            Source::Multi(multi_source) => {
                for member in multi_source.sources {
                    Self::push(
                        expanded,
                        index,
                        first,
                        member,
                        Some(multi_source.name.clone()),
                    );
                }
            }
        }
    }
}

/// Adapts a [`RepoDataSource`] to the internal [`SubdirClient`] trait
/// for a specific platform.
///
/// This adapter is used internally by the gateway to treat custom sources
/// the same way as channel subdirectories.
pub(super) struct CustomSourceClient {
    source: Arc<dyn RepoDataSource>,
    platform: Subdir,
}

impl CustomSourceClient {
    /// Create a new adapter for the given source and platform.
    pub fn new(source: Arc<dyn RepoDataSource>, platform: Subdir) -> Self {
        Self { source, platform }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SubdirClient for CustomSourceClient {
    async fn fetch_package_records(
        &self,
        name: &PackageName,
        _reporter: Option<&dyn Reporter>,
    ) -> Result<PackageRecords, GatewayError> {
        let records = self
            .source
            .fetch_package_records(self.platform, name)
            .await?;
        let (unique_base_deps, unique_extra_deps) =
            extract_unique_deps_split(records.iter().map(|r| &**r));
        Ok(PackageRecords {
            records,
            removed: Vec::new(),
            unique_base_deps,
            unique_extra_deps,
        })
    }

    fn package_names(&self) -> Vec<String> {
        self.source.package_names(self.platform)
    }
}
