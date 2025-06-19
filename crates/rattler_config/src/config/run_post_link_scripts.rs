use std::str::FromStr;

use serde::{de::IntoDeserializer, Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RunPostLinkScripts {
    /// Run the post link scripts, we call this insecure as it may run arbitrary code.
    Insecure,
    /// Do not run the post link scripts
    #[default]
    False,
}

impl FromStr for RunPostLinkScripts {
    type Err = serde::de::value::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::deserialize(s.into_deserializer())
    }
}
