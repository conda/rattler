use crate::version_spec::{LogicalOperator, VersionOperator};
use crate::{match_spec, ChannelConfig, MatchSpec, PackageRecord, RepoData, Version, VersionSpec};
use async_compression::Level::Default;
use fxhash::FxHashMap;
use itertools::Itertools;
use pubgrub::range::Range;
use pubgrub::solver::{Dependencies, DependencyProvider};
use pubgrub::type_aliases::DependencyConstraints;
use pubgrub::version_set::VersionSet;
use std::borrow::Borrow;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::iter::once;

#[derive(Default)]
pub struct PackageRecordIndex {
    versions: FxHashMap<Version, PackageRecord>,
}

#[derive(Default)]
pub struct PackageIndex {
    packages: FxHashMap<String, PackageRecordIndex>,
}

impl From<Vec<RepoData>> for PackageIndex {
    fn from(repo_datas: Vec<RepoData>) -> Self {
        let mut index = Self::default();
        for repo_data in repo_datas {
            for (_, record) in repo_data.packages {
                let package_index = index.packages.entry(record.name.clone()).or_default();
                package_index
                    .versions
                    .insert(record.version.clone(), record);
            }
        }
        index
    }
}

impl PackageIndex {
    pub fn add(&mut self, record: PackageRecord) {
        let package_index = self.packages.entry(record.name.clone()).or_default();
        package_index
            .versions
            .insert(record.version.clone(), record);
    }

    pub fn available_versions(&self, package: &String) -> impl Iterator<Item = &PackageRecord> {
        self.packages
            .get(package)
            .into_iter()
            .flat_map(|package_index| package_index.versions.values())
    }
}

pub struct SolverIndex {
    index: PackageIndex,
    channel_config: ChannelConfig,
}

impl SolverIndex {
    pub fn new(index: PackageIndex) -> Self {
        SolverIndex {
            index,
            channel_config: ChannelConfig::default(),
        }
    }

    pub fn add(&mut self, record: PackageRecord) {
        self.index.add(record);
    }

    pub fn available_versions(&self, package: &Package) -> impl Iterator<Item = &PackageRecord> {
        self.index.available_versions(package)
    }
}

pub type Package = String;

impl DependencyProvider<Package, MatchSpecSet> for SolverIndex {
    fn choose_package_version<T: Borrow<Package>, U: Borrow<MatchSpecSet>>(
        &self,
        potential_packages: impl Iterator<Item = (T, U)>,
    ) -> Result<(T, Option<PackageRecord>), Box<dyn Error>> {
        let (package, version) = pubgrub::solver::choose_package_with_fewest_versions(
            |p| self.available_versions(p).cloned(),
            potential_packages,
        );
        Ok((package, version))
    }

    fn get_dependencies(
        &self,
        package: &Package,
        version: &PackageRecord,
    ) -> Result<Dependencies<Package, MatchSpecSet>, Box<dyn Error>> {
        println!("get_dependencies for `{}` {}", package, version.version);
        let deps = version
            .depends
            .iter()
            .map(
                |spec_str| -> Result<(String, MatchSpecSet), anyhow::Error> {
                    let spec = MatchSpec::from_str(spec_str, &self.channel_config)?;
                    println!(" - {}", spec_str);
                    Ok((spec.name.as_ref().cloned().unwrap(), spec.into()))
                },
            )
            .collect::<Result<_, _>>()?;
        println!("{:?}", &deps);
        Ok(Dependencies::Known(deps))
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MatchSpecSet {
    version_spec: VersionSpec,
}

impl Display for MatchSpecSet {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.version_spec)
    }
}

impl From<MatchSpec> for MatchSpecSet {
    fn from(m: MatchSpec) -> Self {
        Self {
            version_spec: m.version.unwrap_or(VersionSpec::Any),
        }
    }
}

impl VersionSet for VersionSpec {
    type V = Version;

    /// Constructor for an empty set containing no version.
    fn empty() -> Self {
        VersionSpec::None
    }

    /// Constructor for a set containing exactly one version.
    fn singleton(v: Self::V) -> Self {
        VersionSpec::Operator(VersionOperator::Equals, v)
    }

    /// Compute the complement of this set.
    fn complement(&self) -> Self {
        let result = match self {
            VersionSpec::None => VersionSpec::Any,
            VersionSpec::Any => VersionSpec::None,
            VersionSpec::Operator(op, version) => {
                VersionSpec::Operator(op.complement(), version.clone())
            }
            VersionSpec::Group(op, versions) => VersionSpec::Group(
                op.complement(),
                versions.iter().map(|op| op.complement()).collect(),
            ),
        };
        // println!("complement of '{}' = '{}'", self, &result);
        result
    }

    /// Compute the intersection with another set.
    fn intersection(&self, other: &Self) -> Self {
        let intersection = if self == other {
            self.clone()
        } else {
            match (self, other) {
                // If one is None, the result is None
                (VersionSpec::None, _) | (_, VersionSpec::None) => VersionSpec::None,

                // If one if Any, the other spec is enough
                (VersionSpec::Any, other) | (other, VersionSpec::Any) => other.clone(),

                // Both are and groups, concatenate them
                (
                    VersionSpec::Group(LogicalOperator::And, elems1),
                    VersionSpec::Group(LogicalOperator::And, elems2),
                ) => VersionSpec::Group(
                    LogicalOperator::And,
                    elems1
                        .iter()
                        .cloned()
                        .chain(elems2.iter().cloned())
                        .sorted()
                        .collect(),
                ),

                // One of the specs an and group, fuse it with an element
                (VersionSpec::Group(LogicalOperator::And, elems), other)
                | (other, VersionSpec::Group(LogicalOperator::And, elems)) => VersionSpec::Group(
                    LogicalOperator::And,
                    elems.iter().cloned().chain(once(other.clone())).collect(),
                ),

                // Otherwise create a new group
                (a, b) => VersionSpec::Group(
                    LogicalOperator::And,
                    once(a.clone()).chain(once(b.clone())).collect(),
                ),
            }
        };
        // println!(
        //     "intersection of '{}' and '{}' = '{}'",
        //     self, other, &intersection
        // );
        intersection
    }

    /// Evaluate membership of a version in this set.
    fn contains(&self, v: &Self::V) -> bool {
        let contains = match self {
            VersionSpec::None => false,
            VersionSpec::Any => true,
            VersionSpec::Group(LogicalOperator::And, elems) => {
                elems.iter().all(|spec| spec.contains(v))
            }
            VersionSpec::Group(LogicalOperator::Or, elems) => {
                elems.iter().any(|spec| spec.contains(v))
            }
            VersionSpec::Operator(VersionOperator::Equals, other) => v == other,
            VersionSpec::Operator(VersionOperator::NotEquals, other) => v != other,
            VersionSpec::Operator(VersionOperator::Less, other) => v < other,
            VersionSpec::Operator(VersionOperator::LessEquals, other) => v <= other,
            VersionSpec::Operator(VersionOperator::Greater, other) => v > other,
            VersionSpec::Operator(VersionOperator::GreaterEquals, other) => v >= other,
            VersionSpec::Operator(VersionOperator::StartsWith, other) => v.starts_with(other),
            VersionSpec::Operator(VersionOperator::NotStartsWith, other) => !v.starts_with(other),
            VersionSpec::Operator(op, _) => unimplemented!("operator {} is not implemented", op),
        };

        // if contains {
        //     println!("{} does contain {}", self, v);
        // } else {
        //     println!("{} does NOT contain {}", self, v);
        // }

        contains
    }
}

impl VersionSet for MatchSpecSet {
    type V = PackageRecord;

    /// Constructor for an empty set containing no version.
    fn empty() -> Self {
        Self {
            version_spec: VersionSpec::empty(),
        }
    }

    /// Constructor for a set containing exactly one version.
    fn singleton(v: Self::V) -> Self {
        Self {
            version_spec: VersionSpec::singleton(v.version),
        }
    }

    /// Compute the complement of this set.
    fn complement(&self) -> Self {
        Self {
            version_spec: self.version_spec.complement(),
        }
    }

    fn intersection(&self, other: &Self) -> Self {
        Self {
            version_spec: self.version_spec.intersection(&other.version_spec),
        }
    }

    fn contains(&self, v: &Self::V) -> bool {
        self.version_spec.contains(&v.version)
    }
}

// use crate::conda;
// use crate::conda::Version;
// use fxhash::FxHashMap;
// use once_cell::sync::Lazy;
// use pubgrub::range::Range;
// use pubgrub::solver::{Dependencies, DependencyProvider};
// use pubgrub::version::Version as PubGrubVersion;
// use std::borrow::Borrow;
// use std::collections::BTreeMap;
// use std::error::Error;
//
// static LOWEST: Lazy<Version> = Lazy::new(|| "0a0".parse().unwrap());
//
// impl PubGrubVersion for Version {
//     fn lowest() -> Self {
//         LOWEST.clone()
//     }
//
//     fn bump(&self) -> Self {
//         self.bump()
//     }
// }
//
// type PackageName = String;
//
// #[derive(Debug, Clone, Default)]
// struct Deps {
//     pub run: FxHashMap<PackageName, Range<Version>>,
// }
//
// #[derive(Clone, Default)]
// pub struct Index {
//     packages: FxHashMap<PackageName, BTreeMap<Version, Deps>>,
// }
//
// impl Index {
//     pub fn add_record(&mut self, record: &conda::Record) -> anyhow::Result<()> {
//         let package_versions = self.packages.entry(record.name.clone()).or_default();
//         package_versions.insert(
//             record.version.clone(),
//             Deps {
//                 run: record
//                     .depends
//                     .iter()
//                     .map(|s| {
//                         (
//                             s.clone()
//                                 .split_once(" ")
//                                 .unwrap_or((s.as_str(), ""))
//                                 .0
//                                 .to_owned(),
//                             Range::any(),
//                         )
//                     })
//                     .collect(),
//             },
//         );
//
//         Ok(())
//     }
// }
//
// impl DependencyProvider<PackageName, Version> for Index {
//     fn choose_package_version<T: Borrow<PackageName>, U: Borrow<Range<Version>>>(
//         &self,
//         potential_packages: impl Iterator<Item = (T, U)>,
//     ) -> Result<(T, Option<Version>), Box<dyn Error>> {
//         let result = pubgrub::solver::choose_package_with_fewest_versions(
//             |p| self.available_versions(p),
//             potential_packages,
//         );
//
//         Ok(result)
//     }
//
//     fn get_dependencies(
//         &self,
//         package: &PackageName,
//         version: &Version,
//     ) -> Result<Dependencies<PackageName, Version>, Box<dyn Error>> {
//         let deps = self.packages.get(package).unwrap().get(version).unwrap();
//         Ok(Dependencies::Known(
//             deps.run
//                 .iter()
//                 .map(|(dep, constraints)| (dep.clone(), constraints.clone()))
//                 .collect(),
//         ))
//     }
// }
//
// impl Index {
//     pub fn available_versions(&self, package: &PackageName) -> impl Iterator<Item = Version> + '_ {
//         let result = self
//             .packages
//             .get(package)
//             .into_iter()
//             .flat_map(|versions| versions.keys())
//             .rev()
//             .cloned();
//         result
//     }
// }

#[cfg(test)]
mod tests {
    use crate::solver::MatchSpecSet;
    use crate::{ChannelConfig, MatchSpec, RepoData};
    use itertools::Itertools;
    use pubgrub::version_set::VersionSet;
    use std::fs::File;
    use std::io::BufReader;
    use std::path::PathBuf;

    fn repo_data() -> RepoData {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_data_path = manifest_dir.join("resources/conda_forge_noarch_repodata.json");

        let mut reader = BufReader::new(File::open(repo_data_path).unwrap());
        serde_json::from_reader(reader).unwrap()
    }

    #[test]
    fn test_versions() {
        let repo_data = repo_data();
        let all_versions = repo_data.packages.values();
        for record in all_versions {
            assert!(!MatchSpecSet::empty().contains(record));
            assert!(MatchSpecSet::full().contains(record));
            assert!(MatchSpecSet::singleton(record.clone()).contains(&record));
            assert!(!MatchSpecSet::singleton(record.clone())
                .complement()
                .contains(&record));
        }
    }

    #[test]
    fn test_version_compare() {
        let repo_data = repo_data();
        for record in repo_data.packages.values().take(100) {
            for record2 in repo_data.packages.values().take(100) {
                assert_ne!(record2 < record, record2 >= record);
                assert_ne!(record2 <= record, record2 > record);
                assert_ne!(record2 == record, record2 != record);
                assert_ne!(record2 >= record, record2 < record);
                assert_ne!(record2 > record, record2 <= record);
            }
        }
    }

    #[test]
    fn test_version_and_set() {
        let repo_data = repo_data();
        let sets = repo_data
            .packages
            .values()
            .flat_map(|p| p.depends.iter())
            .map(|d| {
                MatchSpecSet::from(MatchSpec::from_str(&d, &ChannelConfig::default()).unwrap())
            })
            .take(100);
        let versions = repo_data.packages.values().take(100).collect_vec();
        for set in sets {
            assert_eq!(MatchSpecSet::empty(), set.complement().intersection(&set));
            assert_eq!(MatchSpecSet::full(), set.complement().union(&set));

            for version in versions.iter() {
                assert_eq!(set.contains(&version), !set.complement().contains(&version));
            }
        }
        // for record in repo_data.packages.values().take(100) {
        //     for record2 in repo_data.packages.values().take(100) {
        //         assert_ne!(record2 < record, record2 >= record);
        //         assert_ne!(record2 <= record, record2 > record);
        //         assert_ne!(record2 == record, record2 != record);
        //         assert_ne!(record2 >= record, record2 < record);
        //         assert_ne!(record2 > record, record2 <= record);
        //     }
        // }
    }
}
