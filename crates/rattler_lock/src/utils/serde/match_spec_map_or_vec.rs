use fxhash::FxBuildHasher;
use indexmap::IndexMap;
use rattler_conda_types::{MatchSpec, NamelessMatchSpec, PackageName};
use serde::{Deserialize, Deserializer};
use serde_with::{serde_as, DeserializeAs, DisplayFromStr};

pub(crate) struct MatchSpecMapOrVec;

impl<'de> DeserializeAs<'de, Vec<String>> for MatchSpecMapOrVec {
    fn deserialize_as<D>(deserializer: D) -> Result<Vec<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[serde_as]
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum MapOrVec {
            Vec(Vec<String>),
            Map(
                #[serde_as(as = "IndexMap<_, DisplayFromStr, FxBuildHasher>")]
                IndexMap<PackageName, NamelessMatchSpec, FxBuildHasher>,
            ),
        }

        Ok(match MapOrVec::deserialize(deserializer)? {
            MapOrVec::Vec(v) => v,
            MapOrVec::Map(m) => m
                .into_iter()
                .map(|(name, spec)| MatchSpec::from_nameless(spec, Some(name)).to_string())
                .collect(),
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use serde::Deserialize;
    use serde_with::serde_as;

    #[test]
    fn test_parse_dependencies_as_map_or_vec() {
        #[serde_as]
        #[derive(Deserialize, Eq, PartialEq, Debug)]
        struct Data {
            #[serde_as(deserialize_as = "MatchSpecMapOrVec")]
            dependencies: Vec<String>,
        }

        let data_with_map: Data = serde_yaml::from_str("dependencies:\n  foo: \">3.12\"").unwrap();
        let data_with_vec: Data = serde_yaml::from_str("dependencies:\n- \"foo >3.12\"").unwrap();
        assert_eq!(data_with_map, data_with_vec);
    }
}
