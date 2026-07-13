use std::{
    fmt::{Debug, Display, Formatter},
    hash::Hash,
    str::FromStr,
};

/// A helper to keep the user-provided input in addition to the evaluated
/// value, behaving like the inner value `T` as much as possible.
///
/// Only really makes sense for things represented as a `String` in the lock file...
pub struct Verbatim<T>
where
    T: Sized,
{
    given: Option<String>,
    inner: T,
}

impl<T> serde::Serialize for Verbatim<T>
where
    T: Sized + serde::Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if let Some(given) = &self.given {
            serializer.serialize_str(given)
        } else {
            T::serialize(&self.inner, serializer)
        }
    }
}

impl<'de, T> serde::Deserialize<'de> for Verbatim<T>
where
    T: Sized + std::str::FromStr,
    <T as std::str::FromStr>::Err: std::fmt::Debug,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let given = String::deserialize(deserializer)?;
        let inner = T::from_str(&given).map_err(|e| serde::de::Error::custom(format!("{e:?}")))?;
        Ok(Self {
            inner,
            given: Some(given),
        })
    }
}

impl<T> PartialEq for Verbatim<T>
where
    T: Sized + PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}
impl<T> Eq for Verbatim<T> where T: Sized + Eq {}

impl<T> From<T> for Verbatim<T>
where
    T: Sized,
{
    fn from(inner: T) -> Self {
        Self { inner, given: None }
    }
}

impl<T> Display for Verbatim<T>
where
    T: Sized + Display,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        T::fmt(&self.inner, f)
    }
}

impl<T> Debug for Verbatim<T>
where
    T: Sized + Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verbatim")
            .field("given", &self.given)
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T> Clone for Verbatim<T>
where
    T: Sized + Clone,
{
    fn clone(&self) -> Self {
        Self {
            given: self.given.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<T> std::ops::Deref for Verbatim<T>
where
    T: Sized,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Verbatim<T>
where
    T: Sized,
{
    /// Create a new `Verbatim<T>`
    pub fn new(inner: T) -> Self {
        Self { given: None, inner }
    }

    /// Create a new `Verbatim<T>`
    pub fn new_with_given(inner: T, given: String) -> Self {
        Self {
            given: Some(given),
            inner,
        }
    }

    /// Take the `T` out of the `Verbatim<T>` and leave the `given` behind
    pub fn take(self) -> T {
        self.inner
    }

    /// Set the verbatim string on which the `T` is based.
    pub fn set_given(&mut self, given: String) {
        self.given = Some(given);
    }

    /// Return the verbatim string the `T` is based on
    pub fn given(&self) -> Option<&str> {
        self.given.as_deref()
    }

    /// The inner type
    pub fn inner(&self) -> &T {
        &self.inner
    }
}

impl<T> Hash for Verbatim<T>
where
    T: Sized + Hash,
{
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.inner.hash(state);
    }
}

impl<T> FromStr for Verbatim<T>
where
    T: Sized + FromStr,
{
    type Err = T::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            given: Some(s.to_owned()),
            inner: T::from_str(s)?,
        })
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use rstest::*;

    use super::*;

    #[derive(Debug, Eq, PartialEq)]
    struct Uppercase(String);

    impl FromStr for Uppercase {
        type Err = ();

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            Ok(Self(s.to_uppercase()))
        }
    }

    #[rstest]
    #[case("TEST", Verbatim::new_with_given(Uppercase("TEST".to_string()), "TEST".to_string()))]
    #[case("test", Verbatim::new_with_given(Uppercase("TEST".to_string()), "test".to_string()))]
    fn test_verbatim_construction(#[case] input: String, #[case] expected: Verbatim<Uppercase>) {
        let result: Verbatim<Uppercase> = input.parse().unwrap();
        assert_eq!(result.inner(), expected.inner());
        assert_eq!(result.given(), expected.given());
    }
}
