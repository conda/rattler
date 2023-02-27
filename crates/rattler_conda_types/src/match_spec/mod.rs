use crate::{Channel, VersionSpec};
use serde::Serialize;
use serde_with::skip_serializing_none;
use std::fmt::{Debug, Display, Formatter};

mod parse;

/// A [`MatchSpec`] is, fundamentally, a query language for conda packages. Any of the fields that
/// comprise a [`crate::PackageRecord`] can be used to compose a [`MatchSpec`].
#[skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Eq, PartialEq)]
pub struct MatchSpec {
    pub name: Option<String>,
    pub version: Option<VersionSpec>,
    pub build: Option<String>,
    pub build_number: Option<usize>,
    pub filename: Option<String>,
    pub channel: Option<Channel>,
    pub namespace: Option<String>,
}

impl Display for MatchSpec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(channel) = &self.channel {
            // TODO: namespace
            write!(f, "{}::", channel.canonical_name())?;
        }

        match &self.name {
            Some(name) => write!(f, "{name}")?,
            None => write!(f, "*")?,
        }

        match &self.version {
            Some(version) => write!(f, " {version}")?,
            None => ()
        }

        match &self.build {
            Some(build) => write!(f, " {build}")?,
            None => ()
        }

        Ok(())
    }
}
