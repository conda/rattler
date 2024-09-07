use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{
    de::{Error, MapAccess, Visitor},
    ser::SerializeMap,
    Deserializer, Serializer,
};

use crate::{MatchSpec, NamedChannelOrUrl, ParseStrictness};

/// A representation of an `environment.yaml` file.
#[derive(Default, Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct EnvironmentYaml {
    /// The preferred name for the environment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// The preferred path to the environment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<PathBuf>,

    /// A list of channels that are used to resolve dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<NamedChannelOrUrl>,

    /// A list of matchspecs that are required for the environment. Or a
    /// subsection of specs for another package manager.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<MatchSpecOrSubSection>,

    /// An optional list of variables.
    /// These variables should be dumped into the `conda-meta/state` file of the
    /// target environment.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub variables: IndexMap<String, String>,
}

/// A matchspec or a subsection, as part of the `dependencies` section of an
/// `environment.yaml` file.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchSpecOrSubSection {
    /// A Conda package match spec
    MatchSpec(MatchSpec),
    /// A list of specs for another package manager (pip)
    SubSection(String, Vec<String>),
}

impl MatchSpecOrSubSection {
    /// Returns the matchspec if this is a matchspec, or `None` otherwise.
    pub fn as_match_spec(&self) -> Option<&MatchSpec> {
        match self {
            MatchSpecOrSubSection::MatchSpec(s) => Some(s),
            MatchSpecOrSubSection::SubSection(_, _) => None,
        }
    }

    /// Returns the subsection if this is a subsection, or `None` otherwise.
    pub fn as_sub_section(&self) -> Option<(&String, &Vec<String>)> {
        match self {
            MatchSpecOrSubSection::MatchSpec(_) => None,
            MatchSpecOrSubSection::SubSection(key, specs) => Some((key, specs)),
        }
    }
}

impl EnvironmentYaml {
    /// Returns all the matchspecs in the `dependencies` section of the file.
    pub fn match_specs(&self) -> impl DoubleEndedIterator<Item = &'_ MatchSpec> + '_ {
        self.dependencies
            .iter()
            .filter_map(MatchSpecOrSubSection::as_match_spec)
    }

    /// Returns the subsection with the given name or `None` if no such
    /// subsection exists.
    pub fn find_sub_section(&self, name: &str) -> Option<&[String]> {
        self.dependencies
            .iter()
            .filter_map(MatchSpecOrSubSection::as_sub_section)
            .find_map(|(subsection_name, specs)| {
                (subsection_name == name).then_some(specs.as_slice())
            })
    }

    /// Returns the `pip` subsection
    pub fn pip_specs(&self) -> Option<&[String]> {
        self.find_sub_section("pip")
    }

    /// Reads the contents of a file at the given path and parses it as an
    /// `environment.yaml` file.
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        Self::from_yaml_str(&contents)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Reads the contents of a string and parses it as an `environment.yaml`
    pub fn from_yaml_str(contents: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(contents)
    }

    /// Write the contents of this `environment.yaml` file to the given path.
    pub fn to_path(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_yaml_string())
    }

    /// Converts the contents of this `environment.yaml` file to a string.
    pub fn to_yaml_string(&self) -> String {
        serde_yaml::to_string(&self).expect("failed to serialize to a string")
    }
}

impl<'a> serde::Deserialize<'a> for MatchSpecOrSubSection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'a>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .string(|v| {
                Ok(MatchSpecOrSubSection::MatchSpec(
                    MatchSpec::from_str(v, ParseStrictness::Lenient)
                        .map_err(serde_untagged::de::Error::custom)?,
                ))
            })
            .map(|v| {
                struct SubSectionVisitor;

                impl<'a> Visitor<'a> for SubSectionVisitor {
                    type Value = MatchSpecOrSubSection;

                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str("a list of strings")
                    }

                    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
                    where
                        A: MapAccess<'a>,
                    {
                        let (key, value) = map
                            .next_entry()?
                            .ok_or_else(|| serde::de::Error::custom("expected a map entry"))?;
                        if map.next_key::<String>()?.is_some() {
                            return Err(serde::de::Error::custom(
                                "expected a map with a single entry",
                            ));
                        }
                        Ok(MatchSpecOrSubSection::SubSection(key, value))
                    }
                }

                SubSectionVisitor.visit_map(v)
            })
            .deserialize(deserializer)
    }
}

impl serde::Serialize for MatchSpecOrSubSection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            MatchSpecOrSubSection::MatchSpec(spec) => spec.to_string().serialize(serializer),
            MatchSpecOrSubSection::SubSection(key, value) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(key, value)?;
                map.end()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::get_test_data_dir;

    #[test]
    fn test_deserialize_environment_yaml() {
        insta::glob!(
            "../../../test-data/environments",
            "*.environment.yaml",
            |path| {
                insta::assert_yaml_snapshot!(EnvironmentYaml::from_path(path).unwrap());
            }
        );
    }

    #[test]
    fn test_pip_section() {
        let environment_yaml = EnvironmentYaml::from_path(
            &get_test_data_dir().join("environments/asymmetric_vqgan.environment.yaml"),
        )
        .unwrap();
        insta::assert_debug_snapshot!(environment_yaml.pip_specs());
    }
}
