use crate::version_spec::{LogicalOperator, VersionOperator};
use crate::{ChannelConfig, MatchSpec, PackageRecord, Range, RepoData, Version, VersionSpec};
use fxhash::FxHashMap;
use itertools::Itertools;
use pubgrub::solver::{Dependencies, DependencyProvider};
use pubgrub::version_set::VersionSet;
use std::borrow::Borrow;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};

#[derive(Default)]
pub struct PackageRecordIndex {
    versions: FxHashMap<Version, Vec<PackageRecord>>,
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
                    .entry(record.version.clone())
                    .or_default()
                    .push(record);
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
            .entry(record.version.clone())
            .or_default()
            .push(record);
    }

    pub fn available_versions(&self, package: &String) -> impl Iterator<Item = &PackageRecord> {
        // let result = self
        self.packages
            .get(package)
            .into_iter()
            .flat_map(|package_index| package_index.versions.values())
            .flatten()
            .sorted()
            .rev()
        // .collect_vec();

        // println!("available version: {package}\n{:#?}", &result);
        // result.into_iter()
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
        println!(
            "get_dependencies for {}={}={}",
            package, version.version, version.build
        );
        let deps = match version
            .depends
            .iter()
            .map(
                |spec_str| -> Result<(String, MatchSpecSet), anyhow::Error> {
                    let spec = MatchSpec::from_str(spec_str, &self.channel_config)?;
                    println!(" - {}", spec_str);
                    Ok((spec.name.as_ref().cloned().unwrap(), spec.into()))
                },
            )
            .collect::<Result<_, _>>()
        {
            Err(e) => {
                println!("{}", e);
                return Err(e.into());
            }
            Ok(v) => v,
        };
        Ok(Dependencies::Known(deps))
    }
}

impl From<VersionSpec> for Range<Version> {
    fn from(spec: VersionSpec) -> Self {
        match spec {
            VersionSpec::None => Range::none(),
            VersionSpec::Any => Range::any(),
            VersionSpec::Operator(VersionOperator::Less, v) => Range::less(v),
            VersionSpec::Operator(VersionOperator::LessEquals, v) => Range::less_equal(v),
            VersionSpec::Operator(VersionOperator::Greater, v) => Range::greater(v),
            VersionSpec::Operator(VersionOperator::GreaterEquals, v) => Range::greater_equal(v),
            VersionSpec::Operator(VersionOperator::Equals, v) => Range::equal(v),
            VersionSpec::Operator(VersionOperator::NotEquals, v) => Range::not_equal(v),
            VersionSpec::Operator(VersionOperator::StartsWith, v) => {
                Range::between(v.clone(), v.bump())
            }
            VersionSpec::Operator(op, _v) => {
                unreachable!("version operator {} not implemented", op)
            }
            VersionSpec::Group(LogicalOperator::And, specs) => specs
                .iter()
                .cloned()
                .map(Into::into)
                .reduce(|acc: Range<Version>, version: Range<Version>| acc.intersection(&version))
                .unwrap_or_else(|| Range::none()),
            VersionSpec::Group(LogicalOperator::Or, specs) => specs
                .iter()
                .cloned()
                .map(Into::into)
                .reduce(|acc: Range<Version>, version: Range<Version>| acc.union(&version))
                .unwrap_or_else(|| Range::none()),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MatchSpecSet {
    version_spec: Range<Version>,
}

impl Display for MatchSpecSet {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.version_spec)
    }
}

impl From<MatchSpec> for MatchSpecSet {
    fn from(m: MatchSpec) -> Self {
        Self {
            version_spec: m.version.unwrap_or(VersionSpec::Any).into(),
        }
    }
}

impl VersionSet for MatchSpecSet {
    type V = PackageRecord;

    /// Constructor for an empty set containing no version.
    fn empty() -> Self {
        Self {
            version_spec: Range::none(),
        }
    }

    /// Constructor for a set containing exactly one version.
    fn singleton(v: Self::V) -> Self {
        Self {
            version_spec: Range::equal(v.version.clone()),
        }
    }

    /// Compute the complement of this set.
    fn complement(&self) -> Self {
        Self {
            version_spec: self.version_spec.negate(),
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

        let reader = BufReader::new(File::open(repo_data_path).unwrap());
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
}
