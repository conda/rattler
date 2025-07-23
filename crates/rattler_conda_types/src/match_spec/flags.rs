use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum FlagMatcher {
    /// Match if flag exists
    Required(String),
    /// Match if flag doesn't exist
    Negated(String),
    /// Match if flag exists, but don't fail if it doesn't
    Optional(String),
}

impl FlagMatcher {
    pub fn matches(&self, flags: &BTreeSet<String>) -> bool {
        match self {
            FlagMatcher::Required(flag) => flags.contains(flag),
            FlagMatcher::Negated(flag) => !flags.contains(flag),
            FlagMatcher::Optional(flag) => !flags.contains(flag) || flags.contains(flag),
        }
    }
}

impl Display for FlagMatcher {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            FlagMatcher::Required(flag) => write!(f, "{flag}"),
            FlagMatcher::Negated(flag) => write!(f, "~{flag}"),
            FlagMatcher::Optional(flag) => write!(f, "?{flag}"),
        }
    }
}

impl FromStr for FlagMatcher {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(stripped) = s.strip_prefix('~') {
            Ok(FlagMatcher::Negated(stripped.to_string()))
        } else if let Some(stripped) = s.strip_prefix('?') {
            Ok(FlagMatcher::Optional(stripped.to_string()))
        } else {
            Ok(FlagMatcher::Required(s.to_string()))
        }
    }
}

impl Serialize for FlagMatcher {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            FlagMatcher::Required(flag) => serializer.serialize_str(flag),
            FlagMatcher::Negated(flag) => serializer.serialize_str(&format!("~{flag}")),
            FlagMatcher::Optional(flag) => serializer.serialize_str(&format!("?{flag}")),
        }
    }
}

// Add deserialization implementation
impl<'de> Deserialize<'de> for FlagMatcher {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if s.starts_with('~') || s.starts_with('!') {
            Ok(FlagMatcher::Negated(s[1..].to_string()))
        } else if let Some(stripped) = s.strip_prefix('?') {
            Ok(FlagMatcher::Optional(stripped.to_string()))
        } else {
            Ok(FlagMatcher::Required(s))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::match_spec::Matches;
    use crate::ParseStrictness::Strict;
    use crate::{match_spec::flags::FlagMatcher, MatchSpec, PackageName, PackageRecord, Version};
    #[test]
    fn test_flagmatcher() {
        use std::collections::BTreeSet;

        // Create a set of flags to test against
        let mut flags = BTreeSet::new();
        flags.insert("mkl".to_string());
        flags.insert("cuda".to_string());

        // Test Required flag matcher
        let matcher = FlagMatcher::Required("mkl".to_string());
        assert!(matcher.matches(&flags));

        let matcher = FlagMatcher::Required("nomkl".to_string());
        assert!(!matcher.matches(&flags));

        // Test Negated flag matcher
        let matcher = FlagMatcher::Negated("nomkl".to_string());
        assert!(matcher.matches(&flags));

        let matcher = FlagMatcher::Negated("mkl".to_string());
        assert!(!matcher.matches(&flags));

        // Test Optional flag matcher
        let matcher = FlagMatcher::Optional("mkl".to_string());
        assert!(matcher.matches(&flags));

        let matcher = FlagMatcher::Optional("nomkl".to_string());
        assert!(matcher.matches(&flags));
    }

    #[test]
    fn test_flagmatcher_parsing() {
        // Test parsing standard flag
        let matcher = FlagMatcher::from_str("mkl").unwrap();
        assert!(matches!(matcher, FlagMatcher::Required(_)));

        // Test parsing negated flag
        let matcher = FlagMatcher::from_str("~mkl").unwrap();
        assert!(matches!(matcher, FlagMatcher::Negated(_)));

        // Test parsing optional flag
        let matcher = FlagMatcher::from_str("?mkl").unwrap();
        assert!(matches!(matcher, FlagMatcher::Optional(_)));
    }

    #[test]
    fn test_flagmatcher_display() {
        // Test display formatting for Required flag
        let matcher = FlagMatcher::Required("mkl".to_string());
        assert_eq!(matcher.to_string(), "mkl");

        // Test display formatting for Negated flag
        let matcher = FlagMatcher::Negated("mkl".to_string());
        assert_eq!(matcher.to_string(), "~mkl");

        // Test display formatting for Optional flag
        let matcher = FlagMatcher::Optional("mkl".to_string());
        assert_eq!(matcher.to_string(), "?mkl");
    }

    #[test]
    fn test_flagmatcher_serde() {
        use serde_json;

        // Test serialization
        let matcher = FlagMatcher::Required("mkl".to_string());
        assert_eq!(serde_json::to_string(&matcher).unwrap(), "\"mkl\"");

        let matcher = FlagMatcher::Negated("mkl".to_string());
        assert_eq!(serde_json::to_string(&matcher).unwrap(), "\"~mkl\"");

        let matcher = FlagMatcher::Optional("mkl".to_string());
        assert_eq!(serde_json::to_string(&matcher).unwrap(), "\"?mkl\"");

        // Test deserialization
        let matcher: FlagMatcher = serde_json::from_str("\"mkl\"").unwrap();
        assert!(matches!(matcher, FlagMatcher::Required(_)));

        let matcher: FlagMatcher = serde_json::from_str("\"~mkl\"").unwrap();
        assert!(matches!(matcher, FlagMatcher::Negated(_)));

        let matcher: FlagMatcher = serde_json::from_str("\"?mkl\"").unwrap();
        assert!(matches!(matcher, FlagMatcher::Optional(_)));
    }

    #[test]
    fn test_matchspec_with_flags() {
        use std::collections::BTreeSet;

        // Create a package record with flags
        let mut flags = BTreeSet::new();
        flags.insert("mkl".to_string());
        flags.insert("cuda".to_string());

        let mut package = PackageRecord::new(
            PackageName::new_unchecked("numpy"),
            Version::from_str("1.0").unwrap(),
            String::from("py37_0"),
        );
        package.flags = flags;

        // Test match with required flag
        let spec = MatchSpec::from_str("numpy[flags=['mkl']]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test match with negated flag
        let spec = MatchSpec::from_str("numpy[flags=['~nomkl']]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test match with optional flag
        let spec = MatchSpec::from_str("numpy[flags=['?mkl']]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test match with multiple flags
        let spec = MatchSpec::from_str("numpy[flags=['mkl', 'cuda']]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test non-match with missing required flag
        let spec = MatchSpec::from_str("numpy[flags=['nomkl']]", Strict).unwrap();
        assert!(!spec.matches(&package));
    }
}
