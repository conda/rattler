//! Non-fatal warnings produced by gateway queries, collected on the
//! query output. New warning kinds become variants of
//! [`GatewayWarning`].

use super::channel_expander::ChannelRelationsWarning;

/// A non-fatal issue surfaced by a gateway query.
#[derive(Debug, Clone, thiserror::Error)]
pub enum GatewayWarning {
    /// A non-fatal issue surfaced while resolving [CEP-42]
    /// `channel_relations`.
    ///
    /// [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md
    #[error(transparent)]
    ChannelRelations(#[from] ChannelRelationsWarning),
}
