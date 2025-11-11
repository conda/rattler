//! # Package Flag Matching
//!
//! This module provides a flexible and powerful system for matching package flags,
//! enabling users to select specific package variants based on their build characteristics.
//!
//! ## Overview
//!
//! Package flags are free-form strings that describe build characteristics of a package,
//! such as GPU support, BLAS implementation, architecture features, or release status.
//! For example, a package might have flags like `["gpu:cuda11.8", "blas:mkl", "release"]`.
//!
//! The flag matching system allows users to filter packages based on these flags using
//! a simple but expressive syntax.
//!
//! ## Flag Matcher Syntax
//!
//! Flag matchers support three types of matching patterns:
//!
//! ### 1. Required Flags (default)
//! - **Syntax**: `flag` or `flag:value`
//! - **Behavior**: Package must have this flag
//! - **Examples**:
//!   - `release` - matches packages with the "release" flag
//!   - `gpu:cuda` - matches packages with the exact flag "gpu:cuda"
//!
//! ### 2. Negated Flags
//! - **Syntax**: `~flag`
//! - **Behavior**: Package must NOT have this flag
//! - **Examples**:
//!   - `~debug` - matches packages without the "debug" flag
//!   - `~gpu:cuda` - matches packages without the exact flag "gpu:cuda"
//!
//! ### 3. Optional Flags
//! - **Syntax**: `?flag`
//! - **Behavior**: If a flag with this prefix exists, it must match the condition;
//!   if no such flag exists, the matcher passes
//! - **Examples**:
//!   - `?release` - passes if package has no "release" flag OR has the "release" flag
//!   - `?gpu:cuda` - passes if package has no "gpu:cuda" flag OR has the "gpu:cuda" flag
//!
//! ## Advanced Matching Features
//!
//! ### Wildcard Matching
//! - **Syntax**: `flag:*`
//! - **Behavior**: Matches any flag that starts with the given prefix
//! - **Examples**:
//!   - `gpu:*` - matches "gpu:cuda11.8", "gpu:cuda12.0", "gpu:rocm", etc.
//!   - `blas:*` - matches "blas:mkl", "blas:openblas", etc.
//!
//! ### Numeric Comparisons
//! - **Syntax**: `flag:op<number>` where op is `>`, `<`, `>=`, `<=`, or `=`
//! - **Behavior**: Treats the part after the colon as a number and applies the comparison
//! - **Examples**:
//!   - `archspec:>3` - matches "archspec:4", "archspec:5", etc.
//!   - `cuda:>=11` - matches "cuda:11", "cuda:12", etc.
//!   - `openmp:<=5` - matches "openmp:5", "openmp:4", etc.
//!
//! ### Combining with Prefixes
//! All advanced features can be combined with negation (`~`) and optional (`?`) prefixes:
//! - `~gpu:*` - matches packages without any GPU flag
//! - `?archspec:>3` - if package has an archspec flag, it must be > 3; otherwise passes
//! - `~cuda:>=12` - matches packages that don't have cuda version 12 or higher
//!
//! ## Usage Examples
//!
//! ```text
//! # Select packages with CUDA GPU support
//! pytorch[flags=[gpu:cuda*]]
//!
//! # Select packages with MKL but without debug symbols
//! numpy[flags=[blas:mkl, ~debug]]
//!
//! # Select packages with optional high architecture support
//! scipy[flags=[?archspec:>=4]]
//!
//! # Complex example: GPU support, release build, optional experimental features
//! tensorflow[flags=[gpu:*, release, ?experimental, ~debug]]
//! ```
//!
//! ## Implementation Notes
//!
//! - Flags are stored as simple strings in package metadata
//! - Flag matching is performed during package resolution
//! - Multiple flag matchers are combined with AND logic
//! - The system is designed to be extensible for future matching patterns

use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

/// Comparison operators used in flag matching for numeric comparisons and wildcards
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum ComparisonOperator {
    /// Greater than (`>`)
    Greater,
    /// Less than (`<`)
    Less,
    /// Equal to (`=`)
    Equal,
    /// Greater than or equal (`>=`)
    GreaterEqual,
    /// Less than or equal (`<=`)
    LessEqual,
    /// Wildcard match (`*`) - matches any suffix
    StartsWith,
}

impl ComparisonOperator {
    fn matches(&self, value: i32, target: i32) -> bool {
        match self {
            ComparisonOperator::Greater => value > target,
            ComparisonOperator::Less => value < target,
            ComparisonOperator::Equal => value == target,
            ComparisonOperator::GreaterEqual => value >= target,
            ComparisonOperator::LessEqual => value <= target,
            ComparisonOperator::StartsWith => true, // Value is ignored for StartsWith
        }
    }
}

/// A comparison operation with an operator and a value
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Comparison {
    operator: ComparisonOperator,
    value: i32, // Ignored for StartsWith
}

/// A flag matcher that can match against package flags with various conditions
///
/// Flag matchers are used to filter packages based on their build flags.
/// They support three matching modes (Required, Negated, Optional) and can
/// optionally include comparison operations for more complex matching.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum FlagMatcher {
    /// Match packages that have this flag
    ///
    /// Examples:
    /// - `release` - must have the "release" flag
    /// - `gpu:cuda` - must have the exact "gpu:cuda" flag  
    /// - `gpu:*` - must have any flag starting with "gpu:"
    /// - `archspec:>3` - must have an archspec flag with value > 3
    Required {
        flag: String,
        comparison: Option<Comparison>,
    },
    /// Match packages that do NOT have this flag
    ///
    /// Examples:
    /// - `~debug` - must not have the "debug" flag
    /// - `~gpu:*` - must not have any flag starting with "gpu:"
    /// - `~archspec:>3` - must not have an archspec flag with value > 3
    Negated {
        flag: String,
        comparison: Option<Comparison>,
    },
    /// Match packages where the flag is optional
    ///
    /// If a flag with the given prefix exists, it must match the condition.
    /// If no such flag exists, the matcher passes.
    ///
    /// Examples:
    /// - `?release` - passes if no "release" flag OR has "release" flag
    /// - `?gpu:*` - passes if no gpu flags OR has any gpu flag
    /// - `?archspec:>3` - passes if no archspec flag OR archspec > 3
    Optional {
        flag: String,
        comparison: Option<Comparison>,
    },
}

impl FlagMatcher {
    /// Check if this flag matcher matches against a set of package flags
    ///
    /// # Arguments
    /// * `flags` - The set of flags from a package
    ///
    /// # Returns
    /// * `true` if the matcher's conditions are satisfied by the flags
    /// * `false` otherwise
    ///
    /// # Matching Logic
    /// - **Required**: The flag must exist and match any comparison
    /// - **Negated**: The flag must NOT exist or NOT match the comparison  
    /// - **Optional**: If the flag exists, it must match; if it doesn't exist, pass
    pub fn matches(&self, flags: &BTreeSet<String>) -> bool {
        match self {
            FlagMatcher::Required { flag, comparison } => {
                if let Some(comp) = comparison {
                    flags.iter().any(|f| {
                        if let Some(num_str) = f.strip_prefix(flag) {
                            if comp.operator == ComparisonOperator::StartsWith {
                                return true; // Match any flag starting with prefix
                            }
                            if let Ok(num) = num_str.parse::<i32>() {
                                return comp.operator.matches(num, comp.value);
                            }
                        }
                        false
                    })
                } else {
                    flags.contains(flag)
                }
            }
            FlagMatcher::Negated { flag, comparison } => {
                if let Some(comp) = comparison {
                    !flags.iter().any(|f| {
                        if let Some(num_str) = f.strip_prefix(flag) {
                            if comp.operator == ComparisonOperator::StartsWith {
                                return true;
                            }
                            if let Ok(num) = num_str.parse::<i32>() {
                                return comp.operator.matches(num, comp.value);
                            }
                        }
                        false
                    })
                } else {
                    !flags.contains(flag)
                }
            }
            FlagMatcher::Optional { flag, comparison } => {
                if let Some(comp) = comparison {
                    let has_matching_flag = flags.iter().any(|f| f.starts_with(flag));
                    if !has_matching_flag {
                        return true; // No flag with prefix, so pass
                    }
                    flags.iter().any(|f| {
                        if let Some(num_str) = f.strip_prefix(flag) {
                            if comp.operator == ComparisonOperator::StartsWith {
                                return true;
                            }
                            if let Ok(num) = num_str.parse::<i32>() {
                                return comp.operator.matches(num, comp.value);
                            }
                        }
                        false
                    })
                } else {
                    true // Optional flags without comparison always pass
                }
            }
        }
    }
}

impl Display for FlagMatcher {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            FlagMatcher::Required { flag, comparison } => {
                if let Some(comp) = comparison {
                    if comp.operator == ComparisonOperator::StartsWith {
                        write!(f, "{flag}*")
                    } else {
                        let op_str = match comp.operator {
                            ComparisonOperator::Greater => ">",
                            ComparisonOperator::Less => "<",
                            ComparisonOperator::Equal => "=",
                            ComparisonOperator::GreaterEqual => ">=",
                            ComparisonOperator::LessEqual => "<=",
                            ComparisonOperator::StartsWith => unreachable!(),
                        };
                        write!(f, "{flag}{op_str}{}", comp.value)
                    }
                } else {
                    write!(f, "{flag}")
                }
            }
            FlagMatcher::Negated { flag, comparison } => {
                if let Some(comp) = comparison {
                    if comp.operator == ComparisonOperator::StartsWith {
                        write!(f, "~{flag}*")
                    } else {
                        let op_str = match comp.operator {
                            ComparisonOperator::Greater => ">",
                            ComparisonOperator::Less => "<",
                            ComparisonOperator::Equal => "=",
                            ComparisonOperator::GreaterEqual => ">=",
                            ComparisonOperator::LessEqual => "<=",
                            ComparisonOperator::StartsWith => unreachable!(),
                        };
                        write!(f, "~{flag}{op_str}{}", comp.value)
                    }
                } else {
                    write!(f, "~{flag}")
                }
            }
            FlagMatcher::Optional { flag, comparison } => {
                if let Some(comp) = comparison {
                    if comp.operator == ComparisonOperator::StartsWith {
                        write!(f, "?{flag}*")
                    } else {
                        let op_str = match comp.operator {
                            ComparisonOperator::Greater => ">",
                            ComparisonOperator::Less => "<",
                            ComparisonOperator::Equal => "=",
                            ComparisonOperator::GreaterEqual => ">=",
                            ComparisonOperator::LessEqual => "<=",
                            ComparisonOperator::StartsWith => unreachable!(),
                        };
                        write!(f, "?{flag}{op_str}{}", comp.value)
                    }
                } else {
                    write!(f, "?{flag}")
                }
            }
        }
    }
}

impl FromStr for FlagMatcher {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Check for invalid prefixes (any non-alphanumeric character at the start that isn't ~ or ?)
        if let Some(first_char) = s.chars().next() {
            if !first_char.is_alphanumeric() && first_char != '~' && first_char != '?' {
                return Err(());
            }
        }

        let (prefix, rest) = if let Some(stripped) = s.strip_prefix('~') {
            (Some('~'), stripped)
        } else if let Some(stripped) = s.strip_prefix('?') {
            (Some('?'), stripped)
        } else {
            (None, s)
        };

        // Check for flag:* pattern (wildcard)
        if let Some(flag_base) = rest.strip_suffix('*') {
            if flag_base.ends_with(':') {
                let flag = flag_base.to_string();
                let comparison = Some(Comparison {
                    operator: ComparisonOperator::StartsWith,
                    value: 0,
                });
                return match prefix {
                    Some('~') => Ok(FlagMatcher::Negated { flag, comparison }),
                    Some('?') => Ok(FlagMatcher::Optional { flag, comparison }),
                    None => Ok(FlagMatcher::Required { flag, comparison }),
                    _ => Err(()),
                };
            }
        }

        // Check for flag:op<number> pattern (comparison)
        if let Some((flag_base, comp_str)) = rest.split_once(':') {
            // Try to parse comparison operators with their values
            for (op_str, op) in [
                (">=", ComparisonOperator::GreaterEqual),
                ("<=", ComparisonOperator::LessEqual),
                (">", ComparisonOperator::Greater),
                ("<", ComparisonOperator::Less),
                ("=", ComparisonOperator::Equal),
            ] {
                if let Some(num_str) = comp_str.strip_prefix(op_str) {
                    if let Ok(value) = num_str.parse::<i32>() {
                        let flag = flag_base.to_string() + ":";
                        let comparison = Some(Comparison {
                            operator: op,
                            value,
                        });
                        return match prefix {
                            Some('~') => Ok(FlagMatcher::Negated { flag, comparison }),
                            Some('?') => Ok(FlagMatcher::Optional { flag, comparison }),
                            None => Ok(FlagMatcher::Required { flag, comparison }),
                            _ => Err(()),
                        };
                    }
                }
            }
        }

        // Treat as regular flag
        match prefix {
            Some('~') => Ok(FlagMatcher::Negated {
                flag: rest.to_string(),
                comparison: None,
            }),
            Some('?') => Ok(FlagMatcher::Optional {
                flag: rest.to_string(),
                comparison: None,
            }),
            None => Ok(FlagMatcher::Required {
                flag: rest.to_string(),
                comparison: None,
            }),
            _ => Err(()),
        }
    }
}

impl Serialize for FlagMatcher {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for FlagMatcher {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        FlagMatcher::from_str(&s).map_err(|_| serde::de::Error::custom("Invalid flag matcher"))
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        match_spec::Matches, MatchSpec, PackageName, PackageRecord, ParseStrictness::Strict,
        Version,
    };

    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn test_flagmatcher_matches() {
        let mut flags = BTreeSet::new();
        flags.insert("release".to_string());
        flags.insert("gpu:cuda11".to_string());
        flags.insert("archspec:3".to_string());

        // Test release
        let matcher = FlagMatcher::Required {
            flag: "release".to_string(),
            comparison: None,
        };
        assert!(matcher.matches(&flags));
        let matcher = FlagMatcher::Required {
            flag: "norelease".to_string(),
            comparison: None,
        };
        assert!(!matcher.matches(&flags));

        // Test ~release
        let matcher = FlagMatcher::Negated {
            flag: "norelease".to_string(),
            comparison: None,
        };
        assert!(matcher.matches(&flags));
        let matcher = FlagMatcher::Negated {
            flag: "release".to_string(),
            comparison: None,
        };
        assert!(!matcher.matches(&flags));

        // Test ?release
        let matcher = FlagMatcher::Optional {
            flag: "release".to_string(),
            comparison: None,
        };
        assert!(matcher.matches(&flags));
        let matcher = FlagMatcher::Optional {
            flag: "norelease".to_string(),
            comparison: None,
        };
        assert!(matcher.matches(&flags));

        // Test gpu:*
        let matcher = FlagMatcher::Required {
            flag: "gpu:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::StartsWith,
                value: 0,
            }),
        };
        assert!(matcher.matches(&flags));
        let matcher = FlagMatcher::Required {
            flag: "cpu:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::StartsWith,
                value: 0,
            }),
        };
        assert!(!matcher.matches(&flags));

        // Test archspec:>2
        let matcher = FlagMatcher::Required {
            flag: "archspec:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::Greater,
                value: 2,
            }),
        };
        assert!(matcher.matches(&flags));
        let matcher = FlagMatcher::Required {
            flag: "archspec:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::Greater,
                value: 3,
            }),
        };
        assert!(!matcher.matches(&flags));

        // Test ?archspec:>2
        let matcher = FlagMatcher::Optional {
            flag: "archspec:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::Greater,
                value: 2,
            }),
        };
        assert!(matcher.matches(&flags));
        let matcher = FlagMatcher::Optional {
            flag: "archspec:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::Greater,
                value: 3,
            }),
        };
        assert!(!matcher.matches(&flags));
        let matcher = FlagMatcher::Optional {
            flag: "nospec:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::Greater,
                value: 5,
            }),
        };
        assert!(matcher.matches(&flags)); // No flag with prefix, so pass

        // Test ?gpu:*
        let matcher = FlagMatcher::Optional {
            flag: "gpu:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::StartsWith,
                value: 0,
            }),
        };
        assert!(matcher.matches(&flags));
        let matcher = FlagMatcher::Optional {
            flag: "cpu:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::StartsWith,
                value: 0,
            }),
        };
        assert!(matcher.matches(&flags)); // No flag with prefix, so pass
    }

    #[test]
    fn test_flagmatcher_parsing() {
        assert!(matches!(
            FlagMatcher::from_str("release").unwrap(),
            FlagMatcher::Required {
                comparison: None,
                ..
            }
        ));
        assert!(matches!(
            FlagMatcher::from_str("~release").unwrap(),
            FlagMatcher::Negated {
                comparison: None,
                ..
            }
        ));
        assert!(matches!(
            FlagMatcher::from_str("?release").unwrap(),
            FlagMatcher::Optional {
                comparison: None,
                ..
            }
        ));
        assert!(matches!(
            FlagMatcher::from_str("gpu:*").unwrap(),
            FlagMatcher::Required {
                comparison: Some(Comparison {
                    operator: ComparisonOperator::StartsWith,
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(
            FlagMatcher::from_str("archspec:>2").unwrap(),
            FlagMatcher::Required {
                comparison: Some(Comparison {
                    operator: ComparisonOperator::Greater,
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(
            FlagMatcher::from_str("?archspec:>2").unwrap(),
            FlagMatcher::Optional {
                comparison: Some(Comparison {
                    operator: ComparisonOperator::Greater,
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(
            FlagMatcher::from_str("?gpu:*").unwrap(),
            FlagMatcher::Optional {
                comparison: Some(Comparison {
                    operator: ComparisonOperator::StartsWith,
                    ..
                }),
                ..
            }
        ));
        assert!(FlagMatcher::from_str("@invalid").is_err()); // Invalid prefix
    }

    #[test]
    fn test_flagmatcher_display() {
        let matcher = FlagMatcher::Required {
            flag: "release".to_string(),
            comparison: None,
        };
        assert_eq!(matcher.to_string(), "release");

        let matcher = FlagMatcher::Negated {
            flag: "release".to_string(),
            comparison: None,
        };
        assert_eq!(matcher.to_string(), "~release");

        let matcher = FlagMatcher::Optional {
            flag: "release".to_string(),
            comparison: None,
        };
        assert_eq!(matcher.to_string(), "?release");

        let matcher = FlagMatcher::Required {
            flag: "gpu:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::StartsWith,
                value: 0,
            }),
        };
        assert_eq!(matcher.to_string(), "gpu:*");

        let matcher = FlagMatcher::Required {
            flag: "archspec:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::Greater,
                value: 2,
            }),
        };
        assert_eq!(matcher.to_string(), "archspec:>2");

        let matcher = FlagMatcher::Optional {
            flag: "archspec:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::Greater,
                value: 2,
            }),
        };
        assert_eq!(matcher.to_string(), "?archspec:>2");

        let matcher = FlagMatcher::Optional {
            flag: "gpu:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::StartsWith,
                value: 0,
            }),
        };
        assert_eq!(matcher.to_string(), "?gpu:*");
    }

    #[test]
    fn test_flagmatcher_serde() {
        use serde_json;

        let matcher = FlagMatcher::Required {
            flag: "release".to_string(),
            comparison: None,
        };
        assert_eq!(serde_json::to_string(&matcher).unwrap(), "\"release\"");

        let matcher = FlagMatcher::Negated {
            flag: "release".to_string(),
            comparison: None,
        };
        assert_eq!(serde_json::to_string(&matcher).unwrap(), "\"~release\"");

        let matcher = FlagMatcher::Optional {
            flag: "release".to_string(),
            comparison: None,
        };
        assert_eq!(serde_json::to_string(&matcher).unwrap(), "\"?release\"");

        let matcher = FlagMatcher::Required {
            flag: "gpu:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::StartsWith,
                value: 0,
            }),
        };
        assert_eq!(serde_json::to_string(&matcher).unwrap(), "\"gpu:*\"");

        let matcher = FlagMatcher::Required {
            flag: "archspec:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::Greater,
                value: 2,
            }),
        };
        assert_eq!(serde_json::to_string(&matcher).unwrap(), "\"archspec:>2\"");

        let matcher = FlagMatcher::Optional {
            flag: "archspec:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::Greater,
                value: 2,
            }),
        };
        assert_eq!(serde_json::to_string(&matcher).unwrap(), "\"?archspec:>2\"");

        let matcher = FlagMatcher::Optional {
            flag: "gpu:".to_string(),
            comparison: Some(Comparison {
                operator: ComparisonOperator::StartsWith,
                value: 0,
            }),
        };
        assert_eq!(serde_json::to_string(&matcher).unwrap(), "\"?gpu:*\"");

        // Test deserialization
        assert!(matches!(
            serde_json::from_str::<FlagMatcher>("\"release\"").unwrap(),
            FlagMatcher::Required {
                comparison: None,
                ..
            }
        ));
        assert!(matches!(
            serde_json::from_str::<FlagMatcher>("\"~release\"").unwrap(),
            FlagMatcher::Negated {
                comparison: None,
                ..
            }
        ));
        assert!(matches!(
            serde_json::from_str::<FlagMatcher>("\"?release\"").unwrap(),
            FlagMatcher::Optional {
                comparison: None,
                ..
            }
        ));
        assert!(matches!(
            serde_json::from_str::<FlagMatcher>("\"gpu:*\"").unwrap(),
            FlagMatcher::Required {
                comparison: Some(Comparison {
                    operator: ComparisonOperator::StartsWith,
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(
            serde_json::from_str::<FlagMatcher>("\"archspec:>2\"").unwrap(),
            FlagMatcher::Required {
                comparison: Some(Comparison {
                    operator: ComparisonOperator::Greater,
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(
            serde_json::from_str::<FlagMatcher>("\"?archspec:>2\"").unwrap(),
            FlagMatcher::Optional {
                comparison: Some(Comparison {
                    operator: ComparisonOperator::Greater,
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(
            serde_json::from_str::<FlagMatcher>("\"?gpu:*\"").unwrap(),
            FlagMatcher::Optional {
                comparison: Some(Comparison {
                    operator: ComparisonOperator::StartsWith,
                    ..
                }),
                ..
            }
        ));
    }

    #[test]
    fn test_matchspec_with_flags() {
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
        let spec = MatchSpec::from_str("numpy[flags=[mkl]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test match with negated flag
        let spec = MatchSpec::from_str("numpy[flags=[~nomkl]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test match with optional flag
        let spec = MatchSpec::from_str("numpy[flags=[?mkl]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test match with multiple flags
        let spec = MatchSpec::from_str("numpy[flags=[mkl, cuda]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test nonmatch with missing required flag
        let spec = MatchSpec::from_str("numpy[flags=[nomkl]]", Strict).unwrap();
        assert!(!spec.matches(&package));
    }

    #[test]
    fn test_matchspec_with_wildcard_flags() {
        // Create a package record with flags
        let mut flags = BTreeSet::new();
        flags.insert("gpu:cuda11.8".to_string());
        flags.insert("blas:mkl".to_string());
        flags.insert("release".to_string());

        let mut package = PackageRecord::new(
            PackageName::new_unchecked("pytorch"),
            Version::from_str("2.0.0").unwrap(),
            String::from("py39_cuda118"),
        );
        package.flags = flags;

        // Test wildcard matching
        let spec = MatchSpec::from_str("pytorch[flags=[gpu:*]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test wildcard with multiple flags
        let spec = MatchSpec::from_str("pytorch[flags=[gpu:*, blas:*]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test wildcard that doesn't match
        let spec = MatchSpec::from_str("pytorch[flags=[cpu:*]]", Strict).unwrap();
        assert!(!spec.matches(&package));

        // Test negated wildcard
        let spec = MatchSpec::from_str("pytorch[flags=[~cpu:*]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test optional wildcard that matches
        let spec = MatchSpec::from_str("pytorch[flags=[?gpu:*]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test optional wildcard that doesn't match but still passes
        let spec = MatchSpec::from_str("pytorch[flags=[?cpu:*]]", Strict).unwrap();
        assert!(spec.matches(&package));
    }

    #[test]
    fn test_matchspec_with_numeric_comparison_flags() {
        // Create a package record with numeric flags
        let mut flags = BTreeSet::new();
        flags.insert("archspec:4".to_string());
        flags.insert("cuda:12".to_string());
        flags.insert("openmp:5".to_string());

        let mut package = PackageRecord::new(
            PackageName::new_unchecked("scipy"),
            Version::from_str("1.11.0").unwrap(),
            String::from("py39_0"),
        );
        package.flags = flags;

        // Test greater than
        let spec = MatchSpec::from_str("scipy[flags=[archspec:>3]]", Strict).unwrap();
        assert!(spec.matches(&package));

        let spec = MatchSpec::from_str("scipy[flags=[archspec:>4]]", Strict).unwrap();
        assert!(!spec.matches(&package));

        // Test greater than or equal
        let spec = MatchSpec::from_str("scipy[flags=[archspec:>=4]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test less than
        let spec = MatchSpec::from_str("scipy[flags=[cuda:<15]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test less than or equal
        let spec = MatchSpec::from_str("scipy[flags=[openmp:<=5]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test equals
        let spec = MatchSpec::from_str("scipy[flags=[archspec:=4]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test optional numeric comparison
        let spec = MatchSpec::from_str("scipy[flags=[?archspec:>3]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test optional numeric comparison that doesn't exist
        let spec = MatchSpec::from_str("scipy[flags=[?avx:>2]]", Strict).unwrap();
        assert!(spec.matches(&package)); // Should pass because it's optional

        // Test negated numeric comparison
        let spec = MatchSpec::from_str("scipy[flags=[~archspec:>5]]", Strict).unwrap();
        assert!(spec.matches(&package));
    }

    #[test]
    fn test_matchspec_with_complex_flag_combinations() {
        // Create a package record with various flags
        let mut flags = BTreeSet::new();
        flags.insert("gpu:cuda11.8".to_string());
        flags.insert("archspec:4".to_string());
        flags.insert("release".to_string());
        flags.insert("blas:mkl".to_string());

        let mut package = PackageRecord::new(
            PackageName::new_unchecked("tensorflow"),
            Version::from_str("2.13.0").unwrap(),
            String::from("py39_cuda118"),
        );
        package.flags = flags;

        // Test combination of different flag types
        let spec = MatchSpec::from_str(
            "tensorflow[flags=[gpu:*, archspec:>=3, release, ~debug]]",
            Strict,
        )
        .unwrap();
        assert!(spec.matches(&package));

        // Test with version and flags
        let spec =
            MatchSpec::from_str("tensorflow >=2.0[flags=[gpu:*, ?release]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test complex optional logic
        let spec = MatchSpec::from_str("tensorflow[flags=[?archspec:>5, gpu:*]]", Strict).unwrap();
        assert!(!spec.matches(&package)); // archspec:4 exists but fails >5, so the optional check fails

        // Test all flag types together
        let spec = MatchSpec::from_str(
            "tensorflow[flags=[release, ~debug, ?experimental, blas:*, archspec:<=4]]",
            Strict,
        )
        .unwrap();
        assert!(spec.matches(&package));
    }

    #[test]
    fn test_matchspec_flag_edge_cases() {
        let mut flags = BTreeSet::new();
        flags.insert("feature:123".to_string());
        flags.insert("version:3.14".to_string());

        let mut package = PackageRecord::new(
            PackageName::new_unchecked("package"),
            Version::from_str("1.0.0").unwrap(),
            String::from("0"),
        );
        package.flags = flags.clone();

        // Test flag with colon but no wildcard or comparison
        let spec = MatchSpec::from_str("package[flags=[feature:123]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test empty package flags
        package.flags = BTreeSet::new();
        let spec = MatchSpec::from_str("package[flags=[~anything]]", Strict).unwrap();
        assert!(spec.matches(&package));

        let spec = MatchSpec::from_str("package[flags=[?anything]]", Strict).unwrap();
        assert!(spec.matches(&package));

        // Test with many flags
        for i in 0..20 {
            flags.insert(format!("flag{}", i));
        }
        package.flags = flags;

        let spec = MatchSpec::from_str("package[flags=[flag5, flag15, ~flag25]]", Strict).unwrap();
        assert!(spec.matches(&package));
    }

    #[test]
    fn test_matchspec_simple_optional_flag() {
        // Test simple optional flag matching
        let mut flags = BTreeSet::new();
        flags.insert("archspec:4".to_string());

        let mut package = PackageRecord::new(
            PackageName::new_unchecked("test"),
            Version::from_str("1.0.0").unwrap(),
            String::from("0"),
        );
        package.flags = flags;

        // This should NOT pass because archspec:4 exists but is not > 5
        let spec = MatchSpec::from_str("test[flags=[?archspec:>5]]", Strict).unwrap();
        assert!(!spec.matches(&package));

        // But this should pass because no 'missing' flag exists
        let spec = MatchSpec::from_str("test[flags=[?missing:>5]]", Strict).unwrap();
        assert!(spec.matches(&package));
    }

    #[test]
    fn test_matchspec_flag_parsing_and_display_roundtrip() {
        let test_cases = vec![
            "package[flags=[release]]",
            "package[flags=[~debug]]",
            "package[flags=[?experimental]]",
            "package[flags=[gpu:*]]",
            "package[flags=[archspec:>3]]",
            "package[flags=[cuda:>=11, archspec:<5]]",
            "package[flags=[release, ~debug, ?test, gpu:*, arch:>2]]",
        ];

        for original in test_cases {
            let spec = MatchSpec::from_str(original, Strict)
                .expect(&format!("Failed to parse: {}", original));
            let displayed = spec.to_string();
            let reparsed = MatchSpec::from_str(&displayed, Strict)
                .expect(&format!("Failed to reparse: {}", displayed));

            // Check that the flags are the same
            assert_eq!(
                spec.flags, reparsed.flags,
                "Flags mismatch for: {}",
                original
            );
        }
    }
}
