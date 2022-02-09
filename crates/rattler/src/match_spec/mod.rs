use crate::{Channel, VersionSpec};
use serde::Serialize;
use std::fmt::Debug;

mod parse;

/// A `MatchSpec` is, fundamentally, a query language for conda packages. Any of the fields that
/// comprise a [`PackageRecord`] can be used to compose a `MatchSpec`.
#[derive(Debug, Default, Clone, Serialize)]
struct MatchSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<VersionSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    build: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel: Option<Channel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
}
