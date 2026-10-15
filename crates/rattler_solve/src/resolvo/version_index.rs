use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    ops::Range,
};

use rattler_conda_types::{
    GenericVirtualPackage, Matches, NamelessMatchSpec, Version, VersionSpec,
};
use resolvo::{Candidates, DenseIndex, NameId, SolvableId, utils::Pool};

use super::{NameType, SolverMatchSpec, SolverPackageRecord};

/// The records of each package, grouped by the [`Structure`] of their version
/// on first use.
#[derive(Default)]
pub(super) struct VersionIndex<'a> {
    group_of_solvable: Vec<u32>,
    /// Ranges never overlap, so other packages' candidates are matched in full.
    groups_of_package: HashMap<NameId, Range<u32>>,
    versions: Vec<&'a Version>,
    group_matches: Vec<bool>,
}

const NO_GROUP: u32 = u32::MAX;

impl<'a> VersionIndex<'a> {
    /// Returns the `candidates` that match `spec`, or the others if `inverse` is
    /// set. The version spec is tested once per group of `name`; candidates in a
    /// passing group, or in none (like virtual packages), are then matched in full.
    pub(super) fn filter(
        &mut self,
        pool: &Pool<SolverMatchSpec<'a>, NameType>,
        records: &HashMap<NameId, Candidates>,
        name: NameId,
        candidates: &[SolvableId],
        spec: &NamelessMatchSpec,
        inverse: bool,
    ) -> Vec<SolvableId> {
        let groups = match &spec.version {
            Some(version_spec) if !matches!(version_spec, VersionSpec::Any) => {
                let groups = self.package_groups(pool, records, name);
                let versions = &self.versions[groups.start as usize..groups.end as usize];
                self.group_matches.clear();
                self.group_matches
                    .extend(versions.iter().map(|version| version_spec.matches(version)));
                groups
            }
            _ => 0..0,
        };

        candidates
            .iter()
            .copied()
            .filter(|&candidate| {
                let group = self.group_of_solvable.get(candidate.to_index()).copied();
                let fails_version = group.is_some_and(|group| {
                    groups.contains(&group) && !self.group_matches[(group - groups.start) as usize]
                });
                (!fails_version && matches_one_by_one(pool, candidate, spec)) != inverse
            })
            .collect()
    }

    fn package_groups(
        &mut self,
        pool: &Pool<SolverMatchSpec<'a>, NameType>,
        records: &HashMap<NameId, Candidates>,
        name: NameId,
    ) -> Range<u32> {
        if let Some(groups) = self.groups_of_package.get(&name) {
            return groups.clone();
        }

        let first_group = self.versions.len() as u32;
        let mut group_of_structure = HashMap::new();
        for &solvable in records.get(&name).map_or(&[][..], |c| &c.candidates) {
            let SolverPackageRecord::Record(record) = pool.resolve_solvable(solvable).record else {
                continue;
            };
            let version = record.package_record.version.version();
            let group = *group_of_structure
                .entry(Structure(version))
                .or_insert_with(|| {
                    self.versions.push(version);
                    self.versions.len() as u32 - 1
                });

            let index = solvable.to_index();
            if index >= self.group_of_solvable.len() {
                self.group_of_solvable.resize(index + 1, NO_GROUP);
            }
            self.group_of_solvable[index] = group;
        }

        let groups = first_group..self.versions.len() as u32;
        self.groups_of_package.insert(name, groups.clone());
        groups
    }
}

fn matches_one_by_one(
    pool: &Pool<SolverMatchSpec<'_>, NameType>,
    candidate: SolvableId,
    spec: &NamelessMatchSpec,
) -> bool {
    match &pool.resolve_solvable(candidate).record {
        SolverPackageRecord::Record(record) => spec.matches(*record),
        SolverPackageRecord::VirtualPackage(GenericVirtualPackage {
            version,
            build_string,
            ..
        }) => {
            spec.version.as_ref().is_none_or(|v| v.matches(version))
                && spec
                    .build
                    .as_ref()
                    .is_none_or(|b| b.matches(build_string.as_str()))
        }
        SolverPackageRecord::Extra { .. } => {
            unreachable!("extras should never be compared to matchspecs")
        }
    }
}

/// Compares versions by everything a version spec looks at. `Version` equality
/// isn't enough: `1.1c.1 == 1.1c0.1`, but only the first matches `1.1c.1.*`.
/// Nor is the source text, which may not match the parsed version.
struct Structure<'a>(&'a Version);

impl PartialEq for Structure<'_> {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (self.0, other.0);
        a.epoch() == b.epoch()
            && a.segments().len() == b.segments().len()
            && a.local_segments().len() == b.local_segments().len()
            && a.segments()
                .zip(b.segments())
                .chain(a.local_segments().zip(b.local_segments()))
                .all(|(a_segment, b_segment)| a_segment.components().eq(b_segment.components()))
    }
}

impl Eq for Structure<'_> {}

impl Hash for Structure<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Versions with the same structure are equal, so they hash alike.
        self.0.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_is_stricter_than_version_equality() {
        let version = |source: &str| source.parse::<Version>().unwrap();
        let (a, b) = (version("1.1c.1"), version("1.1c0.1"));
        assert_eq!(a, b);
        let spec: VersionSpec = "1.1c.1.*".parse().unwrap();
        assert!(spec.matches(&a));
        assert!(!spec.matches(&b));
        assert!(Structure(&a) != Structure(&b));
        assert!(Structure(&a) == Structure(&version("1.1c.1")));
    }
}
