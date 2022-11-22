use serde::{Deserialize, Deserializer};
use serde_with::DeserializeAs;
use std::marker::PhantomData;
use url::Url;

/// Deserialize a sequence into `Vec<T>` but filter `None` values.
pub(crate) struct VecSkipNone<T>(PhantomData<T>);

impl<'de, T, I> DeserializeAs<'de, Vec<T>> for VecSkipNone<I>
where
    I: DeserializeAs<'de, Vec<Option<T>>>,
{
    fn deserialize_as<D>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(I::deserialize_as(deserializer)?
            .into_iter()
            .flatten()
            .collect())
    }
}

/// A helper type parser that tries to parse Urls that could be malformed.
pub(crate) struct LossyUrl;

impl<'de> DeserializeAs<'de, Option<Url>> for LossyUrl {
    fn deserialize_as<D>(deserializer: D) -> Result<Option<Url>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let str = match Option::<String>::deserialize(deserializer)? {
            Some(url) => url,
            None => return Ok(None),
        };
        let url = match Url::parse(&str) {
            Ok(url) => url,
            Err(e) => {
                tracing::warn!("unable to parse '{}' as an URL: {e}. Skipping...", str);
                return Ok(None);
            }
        };
        Ok(Some(url))
    }
}

/// A helper type that parses a string either as a string or a vector of strings.
pub(crate) struct MultiLineString;

impl<'de> DeserializeAs<'de, String> for MultiLineString {
    fn deserialize_as<D>(deserializer: D) -> Result<String, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Inner {
            String(String),
            Multi(Vec<String>),
        }

        Ok(match Inner::deserialize(deserializer)? {
            Inner::String(s) => s,
            Inner::Multi(s) => s.join("\n"),
        })
    }
}
