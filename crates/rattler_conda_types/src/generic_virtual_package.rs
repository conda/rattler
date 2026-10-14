use serde::{Deserialize, Serialize};

use crate::{PackageName, Version, package::BuildString};
use std::fmt::{Display, Formatter};

/// A `GenericVirtualPackage` is a Conda package description that contains a `name` and a
/// `version` and a `build_string`. Virtual packages without a build identifier
/// (e.g. `__cuda`) carry the build string `"0"`.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct GenericVirtualPackage {
    /// The name of the package
    pub name: PackageName,

    /// The version of the package
    pub version: Version,

    /// The build identifier of the package. `"0"` when the virtual package
    /// has no build identifier.
    pub build_string: BuildString,
}

impl Display for GenericVirtualPackage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}={}={}",
            self.name.as_normalized(),
            self.version,
            self.build_string
        )
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
        let mut parts = s.splitn(3, '=');

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
        // Like BuildString's serde implementation, preserve legacy metadata
        // without CEP26 validation. Checked construction uses FromStr instead.
        let build_string = BuildString::new_unchecked(parts.next().unwrap_or("0"));

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
    fn legacy_builds_roundtrip() {
        for build in ["", "py3-none-any", "legacy=build"] {
            let json = serde_json::to_string(&format!("foo=1.2.3={build}")).unwrap();
            let package: GenericVirtualPackage = serde_json::from_str(&json).unwrap();
            assert_eq!(package.build_string.as_str(), build);
            assert_eq!(serde_json::to_string(&package).unwrap(), json);
        }
    }

    #[test]
    fn test_serde() {
        let p = GenericVirtualPackage {
            name: "foo".parse().unwrap(),
            version: "1.2.3".parse().unwrap(),
            build_string: "py_0".parse::<BuildString>().unwrap(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(s, "\"foo=1.2.3=py_0\"");
        let p2: GenericVirtualPackage = serde_json::from_str(&s).unwrap();
        assert_eq!(p, p2);

        let p = GenericVirtualPackage {
            name: "foo".parse().unwrap(),
            version: "1.2.3".parse().unwrap(),
            build_string: "0".parse::<BuildString>().unwrap(),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(s, "\"foo=1.2.3=0\"");
        let p2: GenericVirtualPackage = serde_json::from_str(&s).unwrap();
        assert_eq!(p, p2);

        // A missing version and build string default to "0".
        let p2: GenericVirtualPackage = serde_json::from_str("\"__cuda\"").unwrap();
        let s = serde_json::to_string(&p2).unwrap();
        assert_eq!(s, "\"__cuda=0=0\"");
    }
}
