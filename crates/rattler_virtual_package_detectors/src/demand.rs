//! Which virtual package names a set of package records can reference.
//!
//! A detector only needs to run when a solve could ask for one of its names.
//! Scanning the `depends` and `constrains` of the records that take part in
//! the solve tells which names that is.

use std::collections::BTreeSet;

use rattler_conda_types::{MatchSpec, PackageName, PackageRecord, ParseStrictness};

/// The virtual package names, starting with two underscores, that any of
/// `records` mentions in `depends` or `constrains`.
///
/// Specs that do not parse are ignored: the solver rejects them anyway.
pub fn referenced_virtual_packages<'a>(
    records: impl IntoIterator<Item = &'a PackageRecord>,
) -> BTreeSet<PackageName> {
    let mut names = BTreeSet::new();
    for record in records {
        for spec in record.depends.iter().chain(&record.constrains) {
            let Ok(spec) = MatchSpec::from_str(spec, ParseStrictness::Lenient) else {
                continue;
            };
            if let Some(name) = spec.name.into_exact()
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

    use rattler_conda_types::Version;

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
}
