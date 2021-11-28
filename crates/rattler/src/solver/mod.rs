use crate::conda::{Version};
use fxhash::FxHashMap;
use pubgrub::range::Range;
use pubgrub::solver::{Dependencies, DependencyProvider};
use pubgrub::version::Version as PubGrubVersion;
use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::error::Error;
use crate::conda;

type PackageName = String;

#[derive(Debug, Clone, Default)]
struct Deps {
    pub run: FxHashMap<PackageName, Range<Version>>,
}

#[derive(Clone, Default)]
pub struct Index {
    packages: FxHashMap<PackageName, BTreeMap<Version, Deps>>,
}

impl Index {
    pub fn add_record(&mut self, record: &conda::Record) -> anyhow::Result<()> {
        let version = Version::try_from(record.version.as_str())?;
        let package_versions = self.packages.entry(record.name.clone()).or_default();
        package_versions.insert(
            version,
            Deps {
                run: record
                    .depends
                    .iter()
                    .map(|s| {
                        (
                            s.clone()
                                .split_once(" ")
                                .unwrap_or((s.as_str(), ""))
                                .0
                                .to_owned(),
                            Range::any()
                        )
                    })
                    .collect(),
            },
        );

        Ok(())
    }
}

impl DependencyProvider<PackageName, Version> for Index {
    fn choose_package_version<T: Borrow<PackageName>, U: Borrow<Range<Version>>>(
        &self,
        potential_packages: impl Iterator<Item = (T, U)>,
    ) -> Result<(T, Option<Version>), Box<dyn Error>> {
        let result = pubgrub::solver::choose_package_with_fewest_versions(
            |p| self.available_versions(p),
            potential_packages,
        );

        Ok(result)
    }

    fn get_dependencies(
        &self,
        package: &PackageName,
        version: &Version,
    ) -> Result<Dependencies<PackageName, Version>, Box<dyn Error>> {
        let deps = self.packages.get(package).unwrap().get(version).unwrap();
        Ok(Dependencies::Known(
            deps.run
                .iter()
                .map(|(dep, constraints)| (dep.clone(), constraints.clone()))
                .collect(),
        ))
    }
}

impl Index {
    pub fn available_versions(&self, package: &PackageName) -> impl Iterator<Item = Version> + '_ {
        let result = self
            .packages
            .get(package)
            .into_iter()
            .flat_map(|versions| versions.keys())
            .rev()
            .cloned();
        result
    }
}
