use std::hash::BuildHasherDefault;

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer};
use serde_with::{serde_as, DeserializeAs, DisplayFromStr};
use uv_pep508::{Requirement, VersionOrUrl};

pub(crate) struct Pep440MapOrVec;

impl<'de> DeserializeAs<'de, Vec<Requirement>> for Pep440MapOrVec {
    fn deserialize_as<D>(deserializer: D) -> Result<Vec<Requirement>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[serde_as]
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum MapOrVec {
            Vec(Vec<Requirement>),
            Map(
                #[serde_as(as = "IndexMap<_, DisplayFromStr, BuildHasherDefault<ahash::AHasher>>")]
                IndexMap<String, uv_pep440::VersionSpecifiers, BuildHasherDefault<ahash::AHasher>>,
            ),
        }

        Ok(match MapOrVec::deserialize(deserializer)? {
            MapOrVec::Vec(v) => v,
            MapOrVec::Map(m) => m
                .into_iter()
                .map(|(name, spec)| {
                    Ok::<_, uv_normalize::InvalidNameError>(uv_pep508::Requirement {
                        name: uv_normalize::PackageName::from_owned(name)?,
                        extras: Box::new([]),
                        version_or_url: if spec.is_empty() {
                            None
                        } else {
                            Some(VersionOrUrl::VersionSpecifier(spec))
                        },
                        #[allow(clippy::default_trait_access)]
                        marker: Default::default(),
                        origin: None,
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(serde::de::Error::custom)?,
        })
    }
}
