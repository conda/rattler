use serde::{Deserialize, Serialize};

use crate::{PackageName, Version, package::BuildString};
use std::fmt::{Display, Formatter};

/// A `GenericVirtualPackage` is a Conda package description that contains a `name` and a
/// `version` and a `build_string`. Virtual packages without a build identifier
/// (e.g. `__cuda`) carry an empty build string.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct GenericVirtualPackage {
    /// The name of the package
    pub name: PackageName,

    /// The version of the package
    pub version: Version,

    /// The build identifier of the package. Empty when the virtual package
    /// has no build identifier.
    pub build_string: BuildString,
}

impl Display for GenericVirtualPackage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}={}", &self.name.as_normalized(), &self.version)?;
        if !self.build_string.is_empty() {
            write!(f, "={}", &self.build_string)?;
        }
        Ok(())
    }
}

impl Serialize for GenericVirtualPackage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let s = format!("{self}");
        serializer.serialize_str(&s)
    }
}

impl<'de> Deserialize<'de> for GenericVirtualPackage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let mut parts = s.split('=');

        let name = parts
            .next()
            .ok_or_else(|| serde::de::Error::custom("No package name given"))?
            .parse()
            .map_err(serde::de::Error::custom)?;
        let version = parts
            .next()
            .unwrap_or("0")
            .parse()
            .map_err(serde::de::Error::custom)?;
        let build_string = parts
            .next()
            .map(BuildString::new_unchecked)
            .unwrap_or_default();

        Ok(GenericVirtualPackage {
            name,
            version,
            build_string,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde() {
        let p = GenericVirtualPackage {
            name: "foo".parse().unwrap(),
            version: "1.2.3".parse().unwrap(),
            build_string: BuildString::new("py_0").unwrap(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(s, "\"foo=1.2.3=py_0\"");
        let p2: GenericVirtualPackage = serde_json::from_str(&s).unwrap();
        assert_eq!(p, p2);

        let p = GenericVirtualPackage {
            name: "foo".parse().unwrap(),
            version: "1.2.3".parse().unwrap(),
            build_string: BuildString::default(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(s, "\"foo=1.2.3\"");
        let p2: GenericVirtualPackage = serde_json::from_str(&s).unwrap();
        assert_eq!(p, p2);

        let p2: GenericVirtualPackage = serde_json::from_str("\"__cuda\"").unwrap();
        let s = serde_json::to_string(&p2).unwrap();
        assert_eq!(s, "\"__cuda=0\"");
    }
}
