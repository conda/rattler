//! Non-fatal warnings produced by gateway queries.
//!
//! Warnings are collected on the query output and surfaced to the
//! caller, who decides whether to log them, propagate them as errors,
//! or ignore them. New warning kinds are added as variants of
//! [`GatewayWarning`].

use super::channel_expander::ChannelRelationsWarning;

/// A non-fatal issue surfaced by a gateway query. Each variant wraps
/// the typed warning for a specific subsystem so callers can match
/// exhaustively.
#[derive(Debug, Clone, thiserror::Error)]
pub enum GatewayWarning {
    /// A non-fatal issue surfaced while resolving [CEP-42]
    /// `channel_relations`.
    ///
    /// [CEP-42]: https://github.com/conda/ceps/blob/main/cep-0042.md
    #[error(transparent)]
    ChannelRelations(#[from] ChannelRelationsWarning),
}
