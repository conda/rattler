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
    /// The name of the package
    pub name: Option<String>,
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    pub version: Option<VersionSpec>,
    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    pub build: Option<String>,
    /// The build number of the package
    pub build_number: Option<usize>,
    /// Match the specific filename of the package
    pub filename: Option<String>,
    /// The channel of the package
    pub channel: Option<Channel>,
    /// The namespace of the package (currently not used)
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
            None => (),
        }

        match &self.build {
            Some(build) => write!(f, " {build}")?,
            None => (),
        }

        Ok(())
    }
}
