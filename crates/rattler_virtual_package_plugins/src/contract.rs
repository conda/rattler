//! Checking a plugin against what its channel registered it for.
//!
//! The registration in `info.virtual_package_plugins` is a promise: this plugin
//! speaks for exactly these virtual packages. Enforcing it before anything
//! reaches the solver keeps a plugin from quietly claiming names its channel
//! never advertised, which is checkable without trusting the plugin.

use std::collections::BTreeSet;

use rattler_conda_types::PackageName;

use crate::protocol::PluginReport;

/// A plugin's report does not match what its channel registered it for.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ContractViolation {
    /// The plugin reported a virtual package it was not registered for.
    #[error(
        "the plugin reported {} which its channel did not register it for",
        format_names(undeclared)
    )]
    Undeclared {
        /// The names that were not registered, sorted.
        undeclared: Vec<PackageName>,
    },

    /// The plugin gave no verdict for something it was registered for. Absence
    /// is reported as an explicit null, so silence is a bug in the plugin rather
    /// than a system without that hardware.
    #[error(
        "the plugin gave no verdict for {}, which its channel registered it for",
        format_names(missing)
    )]
    Missing {
        /// The names that were registered but not reported, sorted.
        missing: Vec<PackageName>,
    },
}

fn format_names(names: &[PackageName]) -> String {
    names
        .iter()
        .map(PackageName::as_source)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Check that a plugin gave a verdict for every virtual package its channel
/// registered it for, and none for anything else.
///
/// A verdict cannot be given twice: the report is keyed by name, so the wire
/// format has no way to say the same thing about one virtual package twice.
pub fn validate(
    declared: &BTreeSet<PackageName>,
    report: &PluginReport,
) -> Result<(), ContractViolation> {
    let undeclared: Vec<_> = report
        .virtual_packages
        .keys()
        .filter(|name| !declared.contains(*name))
        .cloned()
        .collect();
    if !undeclared.is_empty() {
        return Err(ContractViolation::Undeclared { undeclared });
    }

    let missing: Vec<_> = declared
        .iter()
        .filter(|name| !report.virtual_packages.contains_key(*name))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(ContractViolation::Missing { missing });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::parse_report;

    fn declared(names: &[&str]) -> BTreeSet<PackageName> {
        names
            .iter()
            .map(|n| PackageName::new_unchecked(*n))
            .collect()
    }

    /// A report giving each named virtual package a verdict: present at version
    /// 1, or absent where the name is prefixed with `!`.
    fn report(names: &[&str]) -> PluginReport {
        let verdicts: Vec<_> = names
            .iter()
            .map(|name| match name.strip_prefix('!') {
                Some(absent) => format!(r#""{absent}": null"#),
                None => format!(r#""{name}": {{"version": "1"}}"#),
            })
            .collect();
        parse_report(&format!(
            r#"{{"version": 1, "virtual_packages": {{{}}}}}"#,
            verdicts.join(", ")
        ))
        .expect("valid protocol")
    }

    #[test]
    fn exact_coverage_passes() {
        assert_eq!(
            validate(
                &declared(&["__cuda", "__cuda_arch"]),
                &report(&["__cuda", "__cuda_arch"])
            ),
            Ok(())
        );
    }

    #[test]
    fn all_absent_passes() {
        assert_eq!(
            validate(
                &declared(&["__cuda", "__cuda_arch"]),
                &report(&["!__cuda", "!__cuda_arch"])
            ),
            Ok(())
        );
    }

    #[test]
    fn undeclared_name_is_rejected() {
        assert_eq!(
            validate(&declared(&["__cuda"]), &report(&["__cuda", "__rocm"])),
            Err(ContractViolation::Undeclared {
                undeclared: vec![PackageName::new_unchecked("__rocm")]
            })
        );
    }

    #[test]
    fn silence_about_a_registered_name_is_rejected() {
        assert_eq!(
            validate(&declared(&["__cuda", "__cuda_arch"]), &report(&["__cuda"])),
            Err(ContractViolation::Missing {
                missing: vec![PackageName::new_unchecked("__cuda_arch")]
            })
        );
    }

    #[test]
    fn registering_nothing_permits_nothing() {
        assert_eq!(validate(&declared(&[]), &report(&[])), Ok(()));
        assert!(validate(&declared(&[]), &report(&["__cuda"])).is_err());
    }

    #[test]
    fn violations_name_every_offender() {
        let err = validate(&declared(&["__c"]), &report(&["__a", "__b"])).unwrap_err();
        assert_eq!(
            err.to_string(),
            "the plugin reported __a, __b which its channel did not register it for"
        );
    }
}
