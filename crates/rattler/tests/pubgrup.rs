use rattler::RepoData;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

struct Package {
    name: String,
    build_number: usize,
    dependencies:
}

use crate::{MatchSpec, PackageRecord, Range, Version};
use itertools::Itertools;
use once_cell::sync::OnceCell;
use pubgrub::version_set::VersionSet;
use smallvec::SmallVec;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::iter::once;
use std::sync::RwLock;

static COMPLEMENT_CACHE: OnceCell<RwLock<HashMap<MatchSpecConstraints, MatchSpecConstraints>>> =
    OnceCell::new();

/// A single AND group in a `MatchSpecConstraints`
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct MatchSpecElement {
    version: Range<Version>,
    build_number: Range<usize>,
}

impl MatchSpecElement {
    /// Returns an instance that matches nothing.
    fn none() -> Self {
        Self {
            version: Range::none(),
            build_number: Range::none(),
        }
    }

    /// Returns an instance that matches anything.
    fn any() -> Self {
        Self {
            version: Range::any(),
            build_number: Range::any(),
        }
    }

    /// Returns the intersection of this element and another
    fn intersection(&self, other: &Self) -> Self {
        let version = self.version.intersection(&other.version);
        let build_number = self.build_number.intersection(&other.build_number);
        if version == Range::none() || build_number == Range::none() {
            Self::none()
        } else {
            Self {
                version,
                build_number,
            }
        }
    }

    /// Returns true if the specified packages matches this instance
    pub fn contains(&self, package: &PackageRecord) -> bool {
        self.version.contains(&package.version) && self.build_number.contains(&package.build_number)
    }
}

/// Represents several constraints as a DNF.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct MatchSpecConstraints {
    groups: Vec<MatchSpecElement>,
}

impl From<MatchSpec> for MatchSpecConstraints {
    fn from(spec: MatchSpec) -> Self {
        Self {
            groups: vec![MatchSpecElement {
                version: spec.version.map(Into::into).unwrap_or_else(|| Range::any()),
                build_number: spec
                    .build_number
                    .clone()
                    .map(Range::equal)
                    .unwrap_or_else(|| Range::any()),
            }],
        }
    }
}

impl From<MatchSpecElement> for MatchSpecConstraints {
    fn from(elem: MatchSpecElement) -> Self {
        Self { groups: vec![elem] }
    }
}

impl Display for MatchSpecConstraints {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "bla")
    }
}

impl MatchSpecConstraints {
    fn compute_complement(&self) -> Self {
        if self.groups.is_empty() {
            Self {
                groups: vec![MatchSpecElement::any()],
            }
        } else {
            let mut permutations = Vec::with_capacity(self.groups.len());
            for spec in self.groups.iter() {
                let mut group_entries: SmallVec<[MatchSpecElement; 2]> = SmallVec::new();
                let version_complement = spec.version.negate();
                if version_complement != Range::none() {
                    group_entries.push(MatchSpecElement {
                        version: version_complement,
                        build_number: Range::any(),
                    });
                }

                let build_complement = spec.build_number.negate();
                if build_complement != Range::none() {
                    group_entries.push(MatchSpecElement {
                        version: Range::any(),
                        build_number: spec.build_number.negate(),
                    });
                }

                permutations.push(group_entries);
            }

            let mut groups = HashSet::new();
            for perm in permutations.into_iter().multi_cartesian_product() {
                let group = perm.into_iter().reduce(|a, b| a.intersection(&b)).unwrap();

                if group == MatchSpecElement::any() {
                    return MatchSpecConstraints::from(group);
                } else if group != MatchSpecElement::none() {
                    groups.insert(group);
                }
            }

            Self {
                groups: groups
                    .into_iter()
                    .sorted_by_cached_key(|e| {
                        let mut hasher = DefaultHasher::new();
                        e.hash(&mut hasher);
                        hasher.finish()
                    })
                    .collect(),
            }
        }
    }
}

impl VersionSet for MatchSpecConstraints {
    type V = PackageRecord;

    fn empty() -> Self {
        Self { groups: vec![] }
    }

    fn full() -> Self {
        Self {
            groups: vec![MatchSpecElement {
                version: Range::any(),
                build_number: Range::any(),
            }],
        }
    }

    fn singleton(v: Self::V) -> Self {
        Self {
            groups: vec![MatchSpecElement {
                version: Range::equal(v.version),
                build_number: Range::equal(v.build_number),
            }],
        }
    }

    fn complement(&self) -> Self {
        // dbg!("taking the complement of group ",  self.groups.len());

        let complement_cache = COMPLEMENT_CACHE.get_or_init(|| RwLock::new(Default::default()));
        {
            let read_lock = complement_cache.read().unwrap();
            if let Some(result) = read_lock.get(self) {
                return result.clone();
            }
        }

        dbg!("-- NOT CACHED", self);

        let complement = self.compute_complement();
        {
            let mut write_lock = complement_cache.write().unwrap();
            write_lock.insert(self.clone(), complement.clone());
        }

        return complement;
    }

    fn intersection(&self, other: &Self) -> Self {
        let groups: HashSet<_> = once(self.groups.iter())
            .chain(once(other.groups.iter()))
            .multi_cartesian_product()
            .map(|elems| {
                elems
                    .into_iter()
                    .cloned()
                    .reduce(|a, b| a.intersection(&b))
                    .unwrap()
            })
            .filter(|group| group != &MatchSpecElement::none())
            .collect();

        if groups.iter().any(|group| group == &MatchSpecElement::any()) {
            return MatchSpecElement::any().into();
        }

        let mut groups = groups.into_iter().collect_vec();

        groups.sort_by_cached_key(|e| {
            let mut hasher = DefaultHasher::new();
            e.hash(&mut hasher);
            hasher.finish()
        });

        Self { groups }
    }

    fn contains(&self, v: &Self::V) -> bool {
        self.groups.iter().any(|group| group.contains(v))
    }
}

#[test]
fn test_pubgrub() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_data_path = manifest_dir.join("resources/conda_forge_noarch_repodata.json");

    let reader = BufReader::new(File::open(repo_data_path).unwrap());
    let repo_data: RepoData = serde_json::from_reader(reader).unwrap();
}
