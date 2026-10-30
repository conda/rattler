use std::{collections::HashMap, ops::Range};

use rattler_conda_types::{
    GenericVirtualPackage, Matches, NamelessMatchSpec, Version, VersionSpec,
};
use resolvo::{Candidates, DenseIndex, NameId, SolvableId, utils::Pool};

use super::{NameType, SolverMatchSpec, SolverPackageRecord};

/// The records of each package, grouped by their [Version]
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
        let mut group_of_versions = HashMap::new();
        for &solvable in records.get(&name).map_or(&[][..], |c| &c.candidates) {
            let SolverPackageRecord::Record(record) = pool.resolve_solvable(solvable).record else {
                continue;
            };
            let version = record.package_record.version.version();
            let group = *group_of_versions.entry(version).or_insert_with(|| {
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
