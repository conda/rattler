//! Which virtual package names a set of package records can reference.
//!
//! A detector only needs to run when a solve could ask for one of its names.
//! Scanning `depends`, `constrains`, and every `extra_depends` group of the
//! records taking part in the solve collects these potential references.

use std::collections::BTreeSet;

use crate::{MatchSpec, PackageName, PackageRecord, ParseStrictness};

/// The virtual package names, starting with two underscores, that any of
/// `records` mentions in `depends`, `constrains`, or any `extra_depends` group.
/// All optional groups contribute potential demand, regardless of activation.
///
/// Ordinary specs only have their exact package name parsed; the solver validates
/// the remaining constraints. Qualified and bracket specs use lenient `MatchSpec`
/// parsing so qualifiers and package-name patterns cannot become exact demands.
pub fn referenced_virtual_packages<'a>(
    records: impl IntoIterator<Item = &'a PackageRecord>,
) -> BTreeSet<PackageName> {
    let mut names = BTreeSet::new();
    for record in records {
        for spec in record
            .depends
            .iter()
            .chain(&record.constrains)
            .chain(record.extra_depends.values().flatten())
        {
            let spec = spec.trim_start();
            let name = if spec.contains(':') || spec.contains('[') {
                MatchSpec::from_str(spec, ParseStrictness::Lenient)
                    .ok()
                    .and_then(|spec| spec.name.into_exact())
            } else if spec.starts_with("__") {
                PackageName::from_matchspec_str(spec).ok()
            } else {
                None
            };
            if let Some(name) = name
                && name.as_normalized().starts_with("__")
            {
                names.insert(name);
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::Version;

    use super::*;

    fn record(depends: &[&str], constrains: &[&str]) -> PackageRecord {
        let mut record = PackageRecord::new(
            PackageName::try_from("pkg").unwrap(),
            Version::from_str("1").unwrap(),
            "0".to_string(),
        );
        record.depends = depends.iter().map(ToString::to_string).collect();
        record.constrains = constrains.iter().map(ToString::to_string).collect();
        record
    }

    #[test]
    fn collects_names_from_depends_and_constrains() {
        let records = [
            record(
                &["python >=3.10", "__conda_forge_openmpi >=5.0,<6.0a0"],
                &[],
            ),
            record(&["__cuda >=12"], &["__glibc >=2.28", "libfoo"]),
            record(&["conda-forge::__conda_forge_mpich 4.*"], &[]),
        ];
        let names: Vec<String> = referenced_virtual_packages(&records)
            .iter()
            .map(|name| name.as_normalized().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "__conda_forge_mpich",
                "__conda_forge_openmpi",
                "__cuda",
                "__glibc"
            ]
        );
    }

    #[test]
    fn preserves_exact_names_and_ignores_patterns() {
        let records = [record(
            &[
                "  __CUDA >=12",
                "__cuda<13",
                "conda-forge::__CUDA 12.*",
                "__glibc[version='>=2.28']",
                "__cuda[name=__other]",
                "*[name=__other]",
                "__cuda[ab]",
                "__cuda* >=12",
                "^__cuda.*$",
                "python >=3.10",
            ],
            &["__GLIBC >=2.17", "__archspec=1=x86_64"],
        )];
        let names: Vec<_> = referenced_virtual_packages(&records)
            .into_iter()
            .map(|name| name.as_normalized().to_owned())
            .collect();
        assert_eq!(names, ["__archspec", "__cuda", "__glibc"]);
    }

    #[test]
    fn collects_potential_names_from_all_optional_groups() {
        let mut record = record(&["__cuda >=12"], &["__glibc >=2.28"]);
        record.extra_depends.insert(
            "gpu".to_string(),
            vec!["__cuda >=11".to_string(), "__rocm >=6".to_string()],
        );
        record.extra_depends.insert(
            "mpi".to_string(),
            vec![
                "__conda_forge_openmpi >=5".to_string(),
                "__rocm >=5".to_string(),
                "__glibc >=2.17".to_string(),
            ],
        );
        record.extra_depends.insert("empty".to_string(), Vec::new());

        let names: Vec<_> = referenced_virtual_packages([&record])
            .into_iter()
            .map(|name| name.as_normalized().to_owned())
            .collect();
        assert_eq!(
            names,
            ["__conda_forge_openmpi", "__cuda", "__glibc", "__rocm"]
        );
    }

    #[test]
    fn preserves_exact_name_rules_in_optional_groups() {
        let mut record = record(&[], &[]);
        record.extra_depends.insert(
            "__group_name_is_not_a_dependency".to_string(),
            [
                "  __CUDA >=12",
                "__cuda<13",
                "conda-forge::__CUDA 12.*",
                "__glibc[version='>=2.28']",
                "__archspec=1=x86_64",
                "__cuda[name=__other]",
                "*[name=__other]",
                "__cuda[ab]",
                "__cuda* >=12",
                "conda-forge::__cuda* >=12",
                "^__cuda.*$",
                "python >=3.10",
            ]
            .into_iter()
            .map(ToString::to_string)
            .collect(),
        );

        let names: Vec<_> = referenced_virtual_packages([&record])
            .into_iter()
            .map(|name| name.as_normalized().to_owned())
            .collect();
        assert_eq!(names, ["__archspec", "__cuda", "__glibc"]);
    }
}
