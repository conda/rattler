use crate::PackageRecord;
use fxhash::{FxHashMap, FxHashSet};

/// Sorts the packages topologically
///
/// This function is deterministic, meaning that it will return the same result regardless of the
/// order of `packages` and of the `depends` vector inside the records.
///
/// If cycles are encountered, and one of the packages in the cycle is noarch, the noarch package
/// is sorted _after_ the other packages in the cycle. This is done to ensure that the noarch
/// package is installed last, so that it can be linked correctly (ie. compiled with Python if
/// necessary).
///
/// Note that this function only works for packages with unique names.
pub fn sort_topologically<T: AsRef<PackageRecord> + Clone>(packages: Vec<T>) -> Vec<T> {
    let mut all_packages: FxHashMap<String, T> = packages
        .iter()
        .cloned()
        .map(|p| (p.as_ref().name.as_normalized().to_owned(), p))
        .collect();

    let cycles = find_all_cycles(&all_packages);
    let cycle_breaks = break_cycles(&cycles, &all_packages);
    let roots = get_graph_roots(&packages, &cycle_breaks);

    get_topological_order(roots, &mut all_packages, &cycle_breaks)
}

/// Find cycles with DFS
fn find_all_cycles<T: AsRef<PackageRecord>>(packages: &FxHashMap<String, T>) -> Vec<Vec<String>> {
    let mut all_cycles = Vec::new();
    let mut visited = FxHashSet::default();

    for package in packages.keys() {
        if !visited.contains(package) {
            let mut path = Vec::new();
            dfs(package, packages, &mut visited, &mut path, &mut all_cycles);
        }
    }

    all_cycles
}

fn dfs<T: AsRef<PackageRecord>>(
    node: &str,
    packages: &FxHashMap<String, T>,
    visited: &mut FxHashSet<String>,
    path: &mut Vec<String>,
    all_cycles: &mut Vec<Vec<String>>,
) {
    if path.contains(&node.to_string()) {
        // Cycle detected
        let cycle_start = path.iter().position(|x| x == node).unwrap();
        all_cycles.push(path[cycle_start..].to_vec());
        return;
    }

    if visited.contains(node) {
        return;
    }

    visited.insert(node.to_string());
    path.push(node.to_string());

    if let Some(package) = packages.get(node) {
        for dependency in package.as_ref().depends.iter() {
            let dependency = package_name_from_match_spec(dependency);
            dfs(dependency, packages, visited, path, all_cycles);
        }
    }

    path.pop();
}

/// Retrieves the names of the packages that form the roots of the graph and breaks specified
/// cycles (e.g. if there is a cycle between A and B and there is a `cycle_break (A, B)`, the edge
/// A -> B will be removed)
fn get_graph_roots<T: AsRef<PackageRecord>>(
    records: &[T],
    cycle_breaks: &FxHashSet<(String, String)>,
) -> Vec<String> {
    let all_packages: FxHashSet<_> = records
        .iter()
        .map(|r| r.as_ref().name.as_normalized())
        .collect();

    let dependencies: FxHashSet<_> = records
        .iter()
        .flat_map(|r| {
            r.as_ref()
                .depends
                .iter()
                .map(|d| package_name_from_match_spec(d))
                .filter(|d| {
                    // filter out circular dependencies
                    !cycle_breaks
                        .contains(&(r.as_ref().name.as_normalized().to_owned(), (*d).to_string()))
                })
        })
        .collect();

    all_packages
        .difference(&dependencies)
        .map(ToString::to_string)
        .collect()
}

/// Helper enum to model the recursion inside `get_topological_order` as iteration, to support
/// dependency graphs of arbitrary depth without causing stack overflows
enum Action {
    ResolveAndInstall(String),
    Install(String),
}

/// Breaks cycles by removing the edges that form them
/// Edges from arch to noarch packages are removed to break the cycles.
fn break_cycles<T: AsRef<PackageRecord>>(
    cycles: &[Vec<String>],
    packages: &FxHashMap<String, T>,
) -> FxHashSet<(String, String)> {
    // we record the edges that we want to remove
    let mut cycle_breaks = FxHashSet::default();

    for cycle in cycles {
        for i in 0..cycle.len() {
            let pi1 = &cycle[i];
            // Next package in cycle, wraps around
            let pi2 = &cycle[(i + 1) % cycle.len()];

            let p1 = &packages[pi1];
            let p2 = &packages[pi2];

            // prefer arch packages over noarch packages
            let p1_noarch = p1.as_ref().noarch.is_none();
            let p2_noarch = p2.as_ref().noarch.is_none();

            if p1_noarch && !p2_noarch {
                cycle_breaks.insert((pi1.clone(), pi2.clone()));
                break;
            } else if !p1_noarch && p2_noarch || i == cycle.len() - 1 {
                // This branch should also be taken if we're at the last package in the cycle and no noarch packages are found
                cycle_breaks.insert((pi2.clone(), pi1.clone()));
                break;
            }
        }
    }
    tracing::debug!("Breaking cycle: {:?}", cycle_breaks);
    cycle_breaks
}

/// Returns a vector containing the topological ordering of the packages, based on the provided
/// roots
fn get_topological_order<T: AsRef<PackageRecord>>(
    mut roots: Vec<String>,
    packages: &mut FxHashMap<String, T>,
    cycle_breaks: &FxHashSet<(String, String)>,
) -> Vec<T> {
    // Sorting makes this step deterministic (i.e. the same output is returned, regardless of the
    // original order of the input)
    roots.sort();

    // Store the name of each package in `order` according to the graph's topological sort
    let mut order = Vec::new();
    let mut visited_packages = FxHashSet::default();
    let mut stack: Vec<_> = roots.into_iter().map(Action::ResolveAndInstall).collect();
    while let Some(action) = stack.pop() {
        match action {
            Action::Install(package_name) => {
                order.push(package_name);
            }
            Action::ResolveAndInstall(package_name) => {
                let already_visited = !visited_packages.insert(package_name.clone());
                if already_visited {
                    continue;
                }

                let mut deps = match &packages.get(package_name.as_str()) {
                    Some(p) => p
                        .as_ref()
                        .depends
                        .iter()
                        .map(|d| package_name_from_match_spec(d).to_string())
                        .collect::<Vec<_>>(),
                    None => {
                        // This is a virtual package, so no real package was found for it
                        continue;
                    }
                };

                // Remove the edges that form cycles
                deps.retain(|dep| !cycle_breaks.contains(&(package_name.clone(), dep.clone())));

                // Sorting makes this step deterministic (i.e. the same output is returned, regardless of the
                // original order of the input)
                deps.sort();

                // Install dependencies, then ourselves (the order is reversed because of the stack)
                stack.push(Action::Install(package_name));
                stack.extend(deps.into_iter().map(Action::ResolveAndInstall));
            }
        }
    }

    // Apply the order we just obtained
    let mut output = Vec::with_capacity(order.len());
    for name in order {
        let package = packages.remove(&name).unwrap();
        output.push(package);
    }

    output
}

/// Helper function to obtain the package name from a match spec
fn package_name_from_match_spec(d: &str) -> &str {
    // Unwrap is safe because split always returns at least one value
    d.split([' ', '=']).next().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{get_test_data_dir, RepoDataRecord};
    use rstest::rstest;

    /// Ensures that the packages are the same before and after the sort, and panics otherwise
    fn sanity_check_topological_sort(
        sorted_packages: &[RepoDataRecord],
        original_packages: &[RepoDataRecord],
    ) {
        let all_sorted_packages: FxHashSet<_> = sorted_packages
            .iter()
            .map(|p| p.package_record.name.as_normalized())
            .collect();
        let all_original_packages: FxHashSet<_> = original_packages
            .iter()
            .map(|p| p.package_record.name.as_normalized())
            .collect();
        let missing_in_sorted: Vec<_> = all_original_packages
            .difference(&all_sorted_packages)
            .cloned()
            .collect();
        let new_in_sorted: Vec<_> = all_sorted_packages
            .difference(&all_original_packages)
            .cloned()
            .collect();

        if !missing_in_sorted.is_empty() {
            let joined = missing_in_sorted.join(", ");
            panic!("The following packages are missing after topological sort: {joined}");
        }

        if !new_in_sorted.is_empty() {
            let joined = new_in_sorted.join(", ");
            panic!(
                "The following packages appeared out of thin air after topological sort: {joined}"
            );
        }
    }

    /// Simulates an install of the sorted packages in order, panicking if we attempt to install a
    /// package before its dependencies are met (ignoring circular dependencies)
    fn simulate_install(
        sorted_packages: &[RepoDataRecord],
        circular_dependencies: &FxHashSet<(&str, &str)>,
    ) {
        let packages_by_name: FxHashMap<_, _> = sorted_packages
            .iter()
            .map(|p| {
                (
                    p.package_record.name.as_normalized(),
                    p.package_record.depends.as_slice(),
                )
            })
            .collect();
        let mut installed = FxHashSet::default();

        for (i, p) in sorted_packages.iter().enumerate() {
            let name = p.package_record.name.as_normalized();
            let &deps = packages_by_name.get(name).unwrap();

            // All the package's dependencies must have already been installed
            for dep in deps {
                let dep_name = package_name_from_match_spec(dep);

                if circular_dependencies.contains(&(name, dep_name)) {
                    // Ignore circular dependencies
                } else {
                    assert!(
                        installed.contains(dep_name),
                        "attempting to install {name} (package {i} of {}) but dependency {dep_name} is not yet installed",
                        sorted_packages.len()
                    );
                }
            }

            // Now mark this package as installed too
            installed.insert(name);
        }
    }

    #[rstest]
    #[case("python >=3.0", "python")]
    #[case("python", "python")]
    #[case("python=*=*", "python")]
    #[case("", "")]
    fn test_package_name_from_match_spec(#[case] match_spec: &str, #[case] expected_name: &str) {
        let name = package_name_from_match_spec(match_spec);
        assert_eq!(name, expected_name);
    }

    #[rstest]
    #[case(get_resolved_packages_for_python(), &["python"])]
    #[case(get_resolved_packages_for_python_pip(), &["pip"])]
    #[case(get_resolved_packages_for_numpy(), &["numpy"])]
    #[case(get_resolved_packages_for_two_roots(), &["4ti2", "micromamba"])]
    fn test_get_graph_roots(
        #[case] packages: Vec<RepoDataRecord>,
        #[case] expected_roots: &[&str],
    ) {
        let mut roots = get_graph_roots(&packages, &FxHashSet::default());
        roots.sort();
        assert_eq!(roots.as_slice(), expected_roots);
    }

    #[rstest]
    #[case(get_resolved_packages_for_python(), "python", &[("libzlib", "libgcc-ng")])]
    #[case(get_resolved_packages_for_numpy(), "numpy", &[("llvm-openmp", "libzlib")])]
    #[case(get_resolved_packages_for_two_roots(), "4ti2", &[("libzlib", "libgcc-ng")])]
    #[case(get_resolved_packages_for_rootless_graph(), "pip", &[("python", "pip")])]
    #[case(get_resolved_packages_for_python_pip(), "pip", &[("pip", "python"), ("libzlib", "libgcc-ng")])]
    #[case(get_big_resolved_packages(), "panel", &[("holoviews", "panel")])]
    fn test_topological_sort(
        #[case] packages: Vec<RepoDataRecord>,
        #[case] expected_last_package: &str,
        #[case] circular_deps: &[(&str, &str)],
    ) {
        let sorted_packages = sort_topologically(packages.clone());
        let circular_deps = circular_deps.iter().cloned().collect();

        sanity_check_topological_sort(&sorted_packages, &packages);
        simulate_install(&sorted_packages, &circular_deps);

        // Sanity check: the last package should be python (or pip when it is present)
        let last_package = &sorted_packages[sorted_packages.len() - 1];
        assert_eq!(
            last_package.package_record.name.as_normalized(),
            expected_last_package
        );
    }

    fn get_big_resolved_packages() -> Vec<RepoDataRecord> {
        // load from test-data folder
        let path = get_test_data_dir().join("topological-sort/big_resolution.json");
        let repodata_json = std::fs::read_to_string(path).unwrap();

        serde_json::from_str(&repodata_json).unwrap()
    }

    fn get_resolved_packages_for_two_roots() -> Vec<RepoDataRecord> {
        let repodata_json = r#"[
            {
              "name": "micromamba",
              "version": "1.3.1",
              "build": "0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "c011b30555cb10474c073c46e4f049a2",
              "sha256": "44fdd6c8805a8456d3ecbe8ae05c1904d3c44f022361d8f7027d344ebf55c618",
              "size": 6116169,
              "depends": [],
              "constrains": [],
              "license": "BSD-3-Clause AND MIT AND OpenSSL",
              "license_family": "BSD",
              "timestamp": 1676029825385,
              "fn": "micromamba-1.3.1-0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/micromamba-1.3.1-0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libstdcxx-ng",
              "version": "12.2.0",
              "build": "h46fd767_18",
              "build_number": 18,
              "subdir": "linux-64",
              "md5": "f19e96f96cc89617da02fed96f28974c",
              "sha256": "bc9180d1d5dcb253d89a03ed4ba30877d43dcd3ab77b2ba7cd0bb648edc6176f",
              "size": 4497047,
              "depends": [],
              "constrains": [],
              "license": "GPL-3.0-only WITH GCC-exception-3.1",
              "license_family": "GPL",
              "timestamp": 1665987640909,
              "fn": "libstdcxx-ng-12.2.0-h46fd767_18.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libstdcxx-ng-12.2.0-h46fd767_18.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libzlib",
              "version": "1.2.13",
              "build": "h166bdaf_4",
              "build_number": 4,
              "subdir": "linux-64",
              "md5": "f3f9de449d32ca9b9c66a22863c96f41",
              "sha256": "22f3663bcf294d349327e60e464a51cd59664a71b8ed70c28a9f512d10bc77dd",
              "size": 65503,
              "depends": [
                "libgcc-ng >=12"
              ],
              "constrains": [
                "zlib 1.2.13 *_4"
              ],
              "license": "Zlib",
              "license_family": "Other",
              "timestamp": 1665759624457,
              "fn": "libzlib-1.2.13-h166bdaf_4.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libzlib-1.2.13-h166bdaf_4.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "llvm-openmp",
              "version": "15.0.7",
              "build": "h0cdce71_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "589c9a3575a050b583241c3d688ad9aa",
              "sha256": "7c67d383a8b1f3e7bf9e046e785325c481f6868194edcfb9d78d261da4ad65d4",
              "size": 3268766,
              "depends": [
                "libzlib >=1.2.13,<1.3.0a0"
              ],
              "constrains": [
                "openmp 15.0.7|15.0.7.*"
              ],
              "license": "Apache-2.0 WITH LLVM-exception",
              "license_family": "APACHE",
              "timestamp": 1673584331056,
              "fn": "llvm-openmp-15.0.7-h0cdce71_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/llvm-openmp-15.0.7-h0cdce71_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "_libgcc_mutex",
              "version": "0.1",
              "build": "conda_forge",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "d7c89558ba9fa0495403155b64376d81",
              "sha256": "fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726",
              "size": 2562,
              "depends": [],
              "constrains": [],
              "license": "None",
              "timestamp": 1578324546067,
              "fn": "_libgcc_mutex-0.1-conda_forge.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "_openmp_mutex",
              "version": "4.5",
              "build": "1_llvm",
              "build_number": 1,
              "subdir": "linux-64",
              "md5": "fa8d764c883a53d22ce622bea830c818",
              "sha256": "336db438c84eca10d0765ab81bd0bce677dbc0ab03c136ecf27ed05028397660",
              "size": 5187,
              "depends": [
                "_libgcc_mutex 0.1 conda_forge",
                "llvm-openmp >=9.0.1"
              ],
              "constrains": [],
              "license": "BSD-3-Clause",
              "license_family": "BSD",
              "timestamp": 1582045300613,
              "fn": "_openmp_mutex-4.5-1_llvm.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/_openmp_mutex-4.5-1_llvm.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libgcc-ng",
              "version": "12.2.0",
              "build": "h65d4601_19",
              "build_number": 19,
              "subdir": "linux-64",
              "md5": "e4c94f80aef025c17ab0828cd85ef535",
              "sha256": "f3899c26824cee023f1e360bd0859b0e149e2b3e8b1668bc6dd04bfc70dcd659",
              "size": 953812,
              "depends": [
                "_libgcc_mutex 0.1 conda_forge",
                "_openmp_mutex >=4.5"
              ],
              "constrains": [
                "libgomp 12.2.0 h65d4601_19"
              ],
              "license": "GPL-3.0-only WITH GCC-exception-3.1",
              "license_family": "GPL",
              "timestamp": 1666519671227,
              "fn": "libgcc-ng-12.2.0-h65d4601_19.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libgcc-ng-12.2.0-h65d4601_19.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "gmp",
              "version": "6.2.1",
              "build": "h58526e2_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "b94cf2db16066b242ebd26db2facbd56",
              "sha256": "07a5319e1ac54fe5d38f50c60f7485af7f830b036da56957d0bfb7558a886198",
              "size": 825784,
              "depends": [
                "libgcc-ng >=7.5.0",
                "libstdcxx-ng >=7.5.0"
              ],
              "constrains": [],
              "license": "GPL-2.0-or-later AND LGPL-3.0-or-later",
              "timestamp": 1605751468661,
              "fn": "gmp-6.2.1-h58526e2_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/gmp-6.2.1-h58526e2_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "glpk",
              "version": "4.65",
              "build": "h9202a9a_1004",
              "build_number": 1004,
              "subdir": "linux-64",
              "md5": "e2b206a63a520f880bb87a0a02dfb625",
              "sha256": "bd115cc9dd999e5b670c5d33c85735769e8081f8e1ece76ce8e00efedc8b09e5",
              "size": 1089030,
              "depends": [
                "gmp >=6.2.1,<7.0a0",
                "libgcc-ng >=7.5.0"
              ],
              "constrains": [],
              "license": "GPL-3.0-or-later",
              "license_family": "GPL",
              "timestamp": 1616879608685,
              "fn": "glpk-4.65-h9202a9a_1004.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/glpk-4.65-h9202a9a_1004.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "4ti2",
              "version": "1.6.9",
              "build": "h618b193_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "8426873908534129a44695f0cec442d9",
              "sha256": "d9f122bbb25d291391f1b4438e556ccee350e2487bde1fd3942d3577dcee8f42",
              "size": 1966583,
              "depends": [
                "glpk >=4.65,<4.66.0a0",
                "gmp >=6.1.2,<7.0a0",
                "libgcc-ng >=7.3.0",
                "libstdcxx-ng >=7.3.0"
              ],
              "constrains": [],
              "license": "GPLv2+",
              "timestamp": 1564325714337,
              "fn": "4ti2-1.6.9-h618b193_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/4ti2-1.6.9-h618b193_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            }
          ]"#;

        serde_json::from_str(repodata_json).unwrap()
    }

    fn get_resolved_packages_for_numpy() -> Vec<RepoDataRecord> {
        let repodata_json = r#"[
            {
              "name": "llvm-openmp",
              "version": "15.0.7",
              "build": "h0cdce71_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "589c9a3575a050b583241c3d688ad9aa",
              "sha256": "7c67d383a8b1f3e7bf9e046e785325c481f6868194edcfb9d78d261da4ad65d4",
              "size": 3268766,
              "depends": [
                "libzlib >=1.2.13,<1.3.0a0"
              ],
              "constrains": [
                "openmp 15.0.7|15.0.7.*"
              ],
              "license": "Apache-2.0 WITH LLVM-exception",
              "license_family": "APACHE",
              "timestamp": 1673584331056,
              "fn": "llvm-openmp-15.0.7-h0cdce71_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/llvm-openmp-15.0.7-h0cdce71_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "_libgcc_mutex",
              "version": "0.1",
              "build": "conda_forge",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "d7c89558ba9fa0495403155b64376d81",
              "sha256": "fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726",
              "size": 2562,
              "depends": [],
              "constrains": [],
              "license": "None",
              "timestamp": 1578324546067,
              "fn": "_libgcc_mutex-0.1-conda_forge.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "_openmp_mutex",
              "version": "4.5",
              "build": "1_llvm",
              "build_number": 1,
              "subdir": "linux-64",
              "md5": "fa8d764c883a53d22ce622bea830c818",
              "sha256": "336db438c84eca10d0765ab81bd0bce677dbc0ab03c136ecf27ed05028397660",
              "size": 5187,
              "depends": [
                "_libgcc_mutex 0.1 conda_forge",
                "llvm-openmp >=9.0.1"
              ],
              "constrains": [],
              "license": "BSD-3-Clause",
              "license_family": "BSD",
              "timestamp": 1582045300613,
              "fn": "_openmp_mutex-4.5-1_llvm.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/_openmp_mutex-4.5-1_llvm.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libgcc-ng",
              "version": "12.2.0",
              "build": "h65d4601_19",
              "build_number": 19,
              "subdir": "linux-64",
              "md5": "e4c94f80aef025c17ab0828cd85ef535",
              "sha256": "f3899c26824cee023f1e360bd0859b0e149e2b3e8b1668bc6dd04bfc70dcd659",
              "size": 953812,
              "depends": [
                "_libgcc_mutex 0.1 conda_forge",
                "_openmp_mutex >=4.5"
              ],
              "constrains": [
                "libgomp 12.2.0 h65d4601_19"
              ],
              "license": "GPL-3.0-only WITH GCC-exception-3.1",
              "license_family": "GPL",
              "timestamp": 1666519671227,
              "fn": "libgcc-ng-12.2.0-h65d4601_19.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libgcc-ng-12.2.0-h65d4601_19.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libzlib",
              "version": "1.2.13",
              "build": "h166bdaf_4",
              "build_number": 4,
              "subdir": "linux-64",
              "md5": "f3f9de449d32ca9b9c66a22863c96f41",
              "sha256": "22f3663bcf294d349327e60e464a51cd59664a71b8ed70c28a9f512d10bc77dd",
              "size": 65503,
              "depends": [
                "libgcc-ng >=12"
              ],
              "constrains": [
                "zlib 1.2.13 *_4"
              ],
              "license": "Zlib",
              "license_family": "Other",
              "timestamp": 1665759624457,
              "fn": "libzlib-1.2.13-h166bdaf_4.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libzlib-1.2.13-h166bdaf_4.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "zlib",
              "version": "1.2.13",
              "build": "h166bdaf_4",
              "build_number": 4,
              "subdir": "linux-64",
              "md5": "4b11e365c0275b808be78b30f904e295",
              "sha256": "282ce274ebe6da1fbd52efbb61bd5a93dec0365b14d64566e6819d1691b75300",
              "size": 94099,
              "depends": [
                "libgcc-ng >=12",
                "libzlib 1.2.13 h166bdaf_4"
              ],
              "constrains": [],
              "license": "Zlib",
              "license_family": "Other",
              "timestamp": 1665759636124,
              "fn": "zlib-1.2.13-h166bdaf_4.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/zlib-1.2.13-h166bdaf_4.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "xz",
              "version": "5.2.6",
              "build": "h166bdaf_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "2161070d867d1b1204ea749c8eec4ef0",
              "sha256": "03a6d28ded42af8a347345f82f3eebdd6807a08526d47899a42d62d319609162",
              "size": 418368,
              "depends": [
                "libgcc-ng >=12"
              ],
              "constrains": [],
              "license": "LGPL-2.1 and GPL-2.0",
              "timestamp": 1660346797927,
              "fn": "xz-5.2.6-h166bdaf_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/xz-5.2.6-h166bdaf_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "tzdata",
              "version": "2022g",
              "build": "h191b570_0",
              "build_number": 0,
              "subdir": "noarch",
              "md5": "51fc4fcfb19f5d95ffc8c339db5068e8",
              "sha256": "0bfae0b9962bc0dbf79048f9175b913ed4f53c4310d06708dc7acbb290ad82f6",
              "size": 108083,
              "depends": [],
              "constrains": [],
              "noarch": "generic",
              "license": "LicenseRef-Public-Domain",
              "timestamp": 1669765202563,
              "fn": "tzdata-2022g-h191b570_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/noarch/tzdata-2022g-h191b570_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "tk",
              "version": "8.6.12",
              "build": "h27826a3_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "5b8c42eb62e9fc961af70bdd6a26e168",
              "sha256": "032fd769aad9d4cad40ba261ab222675acb7ec951a8832455fce18ef33fa8df0",
              "size": 3456292,
              "depends": [
                "libgcc-ng >=9.4.0",
                "libzlib >=1.2.11,<1.3.0a0"
              ],
              "constrains": [],
              "license": "TCL",
              "license_family": "BSD",
              "timestamp": 1645033615058,
              "fn": "tk-8.6.12-h27826a3_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/tk-8.6.12-h27826a3_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "ncurses",
              "version": "6.3",
              "build": "h9c3ff4c_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "fb31bcb7af058244479ca635d20f0f4a",
              "sha256": "bcb38449634bfe58e821c28d6814795b5bbad73514f0c7a9af7a710bbffc8243",
              "size": 1036278,
              "depends": [
                "libgcc-ng >=9.4.0"
              ],
              "constrains": [],
              "license": "X11 AND BSD-3-Clause",
              "timestamp": 1641758190772,
              "fn": "ncurses-6.3-h9c3ff4c_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/ncurses-6.3-h9c3ff4c_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "readline",
              "version": "8.1.2",
              "build": "h0f457ee_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "db2ebbe2943aae81ed051a6a9af8e0fa",
              "sha256": "f5f383193bdbe01c41cb0d6f99fec68e820875e842e6e8b392dbe1a9b6c43ed8",
              "size": 298080,
              "depends": [
                "libgcc-ng >=12",
                "ncurses >=6.3,<7.0a0"
              ],
              "constrains": [],
              "license": "GPL-3.0-only",
              "license_family": "GPL",
              "timestamp": 1654822435090,
              "fn": "readline-8.1.2-h0f457ee_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/readline-8.1.2-h0f457ee_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libsqlite",
              "version": "3.40.0",
              "build": "h753d276_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "2e5f9a37d487e1019fd4d8113adb2f9f",
              "sha256": "6008a0b914bd1a3510a3dba38eada93aa0349ebca3a21e5fa276833c8205bf49",
              "size": 810493,
              "depends": [
                "libgcc-ng >=12",
                "libzlib >=1.2.13,<1.3.0a0"
              ],
              "constrains": [],
              "license": "Unlicense",
              "timestamp": 1668697355661,
              "fn": "libsqlite-3.40.0-h753d276_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libsqlite-3.40.0-h753d276_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "sqlite",
              "version": "3.40.0",
              "build": "h4ff8645_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "bb11803129cbbb53ed56f9506ff74145",
              "sha256": "baf0e77938e5215653aa6609ff154cb94aeb0a08083ff8dec2d3ba8dd62263e9",
              "size": 820173,
              "depends": [
                "libgcc-ng >=12",
                "libsqlite 3.40.0 h753d276_0",
                "libzlib >=1.2.13,<1.3.0a0",
                "ncurses >=6.3,<7.0a0",
                "readline >=8.1.2,<9.0a0"
              ],
              "constrains": [],
              "license": "Unlicense",
              "timestamp": 1668697365233,
              "fn": "sqlite-3.40.0-h4ff8645_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/sqlite-3.40.0-h4ff8645_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "ca-certificates",
              "version": "2022.12.7",
              "build": "ha878542_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "ff9f73d45c4a07d6f424495288a26080",
              "sha256": "8f6c81b0637771ae0ea73dc03a6d30bec3326ba3927f2a7b91931aa2d59b1789",
              "size": 145992,
              "depends": [],
              "constrains": [],
              "license": "ISC",
              "timestamp": 1670457595707,
              "fn": "ca-certificates-2022.12.7-ha878542_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/ca-certificates-2022.12.7-ha878542_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "openssl",
              "version": "3.0.8",
              "build": "h0b41bf4_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "e043403cd18faf815bf7705ab6c1e092",
              "sha256": "cd981c5c18463bc7a164fcf45c5cf697d58852b780b4dfa5e83c18c1fda6d7cd",
              "size": 2601535,
              "depends": [
                "ca-certificates",
                "libgcc-ng >=12"
              ],
              "constrains": [],
              "license": "Apache-2.0",
              "license_family": "Apache",
              "timestamp": 1675814291854,
              "fn": "openssl-3.0.8-h0b41bf4_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/openssl-3.0.8-h0b41bf4_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libstdcxx-ng",
              "version": "12.2.0",
              "build": "h46fd767_18",
              "build_number": 18,
              "subdir": "linux-64",
              "md5": "f19e96f96cc89617da02fed96f28974c",
              "sha256": "bc9180d1d5dcb253d89a03ed4ba30877d43dcd3ab77b2ba7cd0bb648edc6176f",
              "size": 4497047,
              "depends": [],
              "constrains": [],
              "license": "GPL-3.0-only WITH GCC-exception-3.1",
              "license_family": "GPL",
              "timestamp": 1665987640909,
              "fn": "libstdcxx-ng-12.2.0-h46fd767_18.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libstdcxx-ng-12.2.0-h46fd767_18.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libffi",
              "version": "3.4.2",
              "build": "h9c3ff4c_2",
              "build_number": 2,
              "subdir": "linux-64",
              "md5": "cb832368fd30ed8b58c6750fe8c3bb74",
              "sha256": "382da0f9ffbaecad8385d93112d1e1d4cd30a1df99a8e89c308cdc4add237640",
              "size": 61735,
              "depends": [
                "libgcc-ng >=9.4.0",
                "libstdcxx-ng >=9.4.0"
              ],
              "constrains": [],
              "license": "MIT",
              "license_family": "MIT",
              "timestamp": 1632174087618,
              "fn": "libffi-3.4.2-h9c3ff4c_2.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libffi-3.4.2-h9c3ff4c_2.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "gdbm",
              "version": "1.18",
              "build": "h0a1914f_2",
              "build_number": 2,
              "subdir": "linux-64",
              "md5": "b77bc399b07a19c00fe12fdc95ee0297",
              "sha256": "8b9606dc896bd9262d09ab2ef1cb55c4ee43f352473209b58b37a9289dd7b00c",
              "size": 194790,
              "depends": [
                "libgcc-ng >=7.5.0",
                "readline >=8.0,<9.0a0"
              ],
              "constrains": [],
              "license": "GPL-3.0",
              "license_family": "GPL",
              "timestamp": 1597622040785,
              "fn": "gdbm-1.18-h0a1914f_2.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/gdbm-1.18-h0a1914f_2.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "expat",
              "version": "2.5.0",
              "build": "h27087fc_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "c4fbad8d4bddeb3c085f18cbf97fbfad",
              "sha256": "b44db0b92ae926b3fbbcd57c179fceb64fa11a9f9d09082e03be58b74dcad832",
              "size": 194025,
              "depends": [
                "libgcc-ng >=12",
                "libstdcxx-ng >=12"
              ],
              "constrains": [],
              "license": "MIT",
              "license_family": "MIT",
              "timestamp": 1666724630498,
              "fn": "expat-2.5.0-h27087fc_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/expat-2.5.0-h27087fc_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "bzip2",
              "version": "1.0.8",
              "build": "h7f98852_4",
              "build_number": 4,
              "subdir": "linux-64",
              "md5": "a1fd65c7ccbf10880423d82bca54eb54",
              "sha256": "cb521319804640ff2ad6a9f118d972ed76d86bea44e5626c09a13d38f562e1fa",
              "size": 495686,
              "depends": [
                "libgcc-ng >=9.3.0"
              ],
              "constrains": [],
              "license": "bzip2-1.0.6",
              "license_family": "BSD",
              "timestamp": 1606604745109,
              "fn": "bzip2-1.0.8-h7f98852_4.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/bzip2-1.0.8-h7f98852_4.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "pypy3.9",
              "version": "7.3.11",
              "build": "h527bfed_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "24cd81dbe06d6d7171ab9996f78fdfc3",
              "sha256": "50d34e9521b3d6d76b135f773e6e79f3020ca3fd9ae774ce9d3d9a96eb57513c",
              "size": 33680019,
              "depends": [
                "bzip2 >=1.0.8,<2.0a0",
                "expat >=2.5.0,<3.0a0",
                "gdbm >=1.18,<1.19.0a0",
                "libffi >=3.4,<4.0a0",
                "libgcc-ng >=12",
                "libsqlite >=3.40.0,<4.0a0",
                "libzlib >=1.2.13,<1.3.0a0",
                "ncurses >=6.3,<7.0a0",
                "openssl >=3.0.7,<4.0a0",
                "sqlite",
                "tk >=8.6.12,<8.7.0a0",
                "tzdata",
                "xz >=5.2.6,<6.0a0",
                "zlib"
              ],
              "constrains": [
                "pypy3.5 ==99999999999",
                "pypy3.6 ==99999999999",
                "pypy3.7 ==99999999999",
                "pypy3.8 ==99999999999",
                "python 3.9.* *_73_pypy"
              ],
              "license": "MIT",
              "license_family": "MIT",
              "timestamp": 1674061610003,
              "fn": "pypy3.9-7.3.11-h527bfed_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/pypy3.9-7.3.11-h527bfed_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "python_abi",
              "version": "3.9",
              "build": "2_pypy39_pp73",
              "build_number": 2,
              "subdir": "linux-64",
              "md5": "f50e5cc08bbc4870d614b49390231e47",
              "sha256": "e65cc88368c927d6b8013b52e1264c2ea607ad443849cf057d4a3674062a406a",
              "size": 4409,
              "depends": [
                "pypy3.9 7.3.*"
              ],
              "constrains": [
                "python 3.9.* *_73_pypy"
              ],
              "track_features": "pypy",
              "license": "BSD-3-Clause",
              "license_family": "BSD",
              "timestamp": 1647846318034,
              "fn": "python_abi-3.9-2_pypy39_pp73.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/python_abi-3.9-2_pypy39_pp73.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libuuid",
              "version": "2.32.1",
              "build": "h7f98852_1000",
              "build_number": 1000,
              "subdir": "linux-64",
              "md5": "772d69f030955d9646d3d0eaf21d859d",
              "sha256": "54f118845498353c936826f8da79b5377d23032bcac8c4a02de2019e26c3f6b3",
              "size": 28284,
              "depends": [
                "libgcc-ng >=9.3.0"
              ],
              "constrains": [],
              "license": "BSD-3-Clause",
              "license_family": "BSD",
              "timestamp": 1607292654633,
              "fn": "libuuid-2.32.1-h7f98852_1000.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libuuid-2.32.1-h7f98852_1000.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libnsl",
              "version": "2.0.0",
              "build": "h7f98852_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "39b1328babf85c7c3a61636d9cd50206",
              "sha256": "32f4fb94d99946b0dabfbbfd442b25852baf909637f2eed1ffe3baea15d02aad",
              "size": 31236,
              "depends": [
                "libgcc-ng >=9.4.0"
              ],
              "constrains": [],
              "license": "GPL-2.0-only",
              "license_family": "GPL",
              "timestamp": 1633040059627,
              "fn": "libnsl-2.0.0-h7f98852_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libnsl-2.0.0-h7f98852_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "ld_impl_linux-64",
              "version": "2.40",
              "build": "h41732ed_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "7aca3059a1729aa76c597603f10b0dd3",
              "sha256": "f6cc89d887555912d6c61b295d398cff9ec982a3417d38025c45d5dd9b9e79cd",
              "size": 704696,
              "depends": [],
              "constrains": [
                "binutils_impl_linux-64 2.40"
              ],
              "license": "GPL-3.0-only",
              "license_family": "GPL",
              "timestamp": 1674833944779,
              "fn": "ld_impl_linux-64-2.40-h41732ed_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/ld_impl_linux-64-2.40-h41732ed_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "python",
              "version": "3.9.16",
              "build": "h2782a2a_0_cpython",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "95c9b7c96a7fd7342e0c9d0a917b8f78",
              "sha256": "00bcb28a294aa78bf9d2a2ecaae8cb887188eae710f9197d823d36fb8a5d9767",
              "size": 24156844,
              "depends": [
                "bzip2 >=1.0.8,<2.0a0",
                "ld_impl_linux-64 >=2.36.1",
                "libffi >=3.4,<4.0a0",
                "libgcc-ng >=12",
                "libnsl >=2.0.0,<2.1.0a0",
                "libsqlite >=3.40.0,<4.0a0",
                "libuuid >=2.32.1,<3.0a0",
                "libzlib >=1.2.13,<1.3.0a0",
                "ncurses >=6.3,<7.0a0",
                "openssl >=3.0.7,<4.0a0",
                "readline >=8.1.2,<9.0a0",
                "tk >=8.6.12,<8.7.0a0",
                "tzdata",
                "xz >=5.2.6,<6.0a0"
              ],
              "constrains": [
                "python_abi 3.9.* *_cp39"
              ],
              "license": "Python-2.0",
              "timestamp": 1675288704799,
              "fn": "python-3.9.16-h2782a2a_0_cpython.conda",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/python-3.9.16-h2782a2a_0_cpython.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libgfortran5",
              "version": "12.2.0",
              "build": "h337968e_19",
              "build_number": 19,
              "subdir": "linux-64",
              "md5": "164b4b1acaedc47ee7e658ae6b308ca3",
              "sha256": "03ea784edd12037dc3a7a0078ff3f9c3383feabb34d5ba910bb2fd7a21a2d961",
              "size": 1839051,
              "depends": [],
              "constrains": [
                "libgfortran-ng 12.2.0"
              ],
              "license": "GPL-3.0-only WITH GCC-exception-3.1",
              "license_family": "GPL",
              "timestamp": 1666519574393,
              "fn": "libgfortran5-12.2.0-h337968e_19.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libgfortran5-12.2.0-h337968e_19.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libgfortran-ng",
              "version": "12.2.0",
              "build": "h69a702a_19",
              "build_number": 19,
              "subdir": "linux-64",
              "md5": "cd7a806282c16e1f2d39a7e80d3a3e0d",
              "sha256": "c7d061f323e80fbc09564179073d8af303bf69b953b0caddcf79b47e352c746f",
              "size": 22884,
              "depends": [
                "libgfortran5 12.2.0 h337968e_19"
              ],
              "constrains": [],
              "license": "GPL-3.0-only WITH GCC-exception-3.1",
              "license_family": "GPL",
              "timestamp": 1666519651510,
              "fn": "libgfortran-ng-12.2.0-h69a702a_19.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libgfortran-ng-12.2.0-h69a702a_19.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libopenblas",
              "version": "0.3.20",
              "build": "pthreads_h78a6416_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "9b6d0781953c9e353faee494336cc229",
              "sha256": "2e840f4165a62dd826d16e0d51b3928bed46ca33fe943addd52c84f55ccc9551",
              "size": 10574699,
              "depends": [
                "libgcc-ng >=10.3.0",
                "libgfortran-ng",
                "libgfortran5 >=10.3.0"
              ],
              "constrains": [
                "openblas >=0.3.20,<0.3.21.0a0"
              ],
              "license": "BSD-3-Clause",
              "license_family": "BSD",
              "timestamp": 1649198465564,
              "fn": "libopenblas-0.3.20-pthreads_h78a6416_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libopenblas-0.3.20-pthreads_h78a6416_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libblas",
              "version": "3.9.0",
              "build": "15_linux64_openblas",
              "build_number": 15,
              "subdir": "linux-64",
              "md5": "04eb983975a1be3e57d6d667414cd774",
              "sha256": "fad1fafee244f0632b887bc958f48b7ee2aa7949cf647ed641df8bdaa81d5070",
              "size": 12730,
              "depends": [
                "libopenblas >=0.3.20,<0.3.21.0a0",
                "libopenblas >=0.3.20,<1.0a0"
              ],
              "constrains": [
                "liblapacke 3.9.0 15_linux64_openblas",
                "libcblas 3.9.0 15_linux64_openblas",
                "liblapack 3.9.0 15_linux64_openblas",
                "blas * openblas"
              ],
              "license": "BSD-3-Clause",
              "license_family": "BSD",
              "timestamp": 1654549111442,
              "fn": "libblas-3.9.0-15_linux64_openblas.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libblas-3.9.0-15_linux64_openblas.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "liblapack",
              "version": "3.9.0",
              "build": "1_h86c2bf4_netlib",
              "build_number": 1,
              "subdir": "linux-64",
              "md5": "a54aa783802a112f92456a5ffb6ec484",
              "sha256": "ecb57287db051d5f8a450159dbc106905509192ce4fa00359e26de76aba4f73c",
              "size": 3105316,
              "depends": [
                "libblas 3.9.0.*",
                "libgcc-ng >=9.3.0",
                "libgfortran-ng",
                "libgfortran5 >=9.3.0"
              ],
              "constrains": [],
              "track_features": "blas_netlib",
              "license": "BSD-3-Clause",
              "license_family": "BSD",
              "timestamp": 1603052059970,
              "fn": "liblapack-3.9.0-1_h86c2bf4_netlib.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/liblapack-3.9.0-1_h86c2bf4_netlib.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libgfortran4",
              "version": "7.5.0",
              "build": "h14aa051_19",
              "build_number": 19,
              "subdir": "linux-64",
              "md5": "918ebd815b3d8c0491e65dd608e4b917",
              "sha256": "7005aa5acd52e0c9c9fb2c193c5ed5f6c7c6f31306c66c7c2ad45edc9ed67508",
              "size": 1319198,
              "depends": [],
              "constrains": [
                "libgfortran-ng 7.5.0 *_19"
              ],
              "license": "GPL-3.0-only WITH GCC-exception-3.1",
              "license_family": "GPL",
              "timestamp": 1618241984213,
              "fn": "libgfortran4-7.5.0-h14aa051_19.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libgfortran4-7.5.0-h14aa051_19.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libcblas",
              "version": "3.9.0",
              "build": "1_h6e990d7_netlib",
              "build_number": 1,
              "subdir": "linux-64",
              "md5": "e2b74c94192472321dbf6a7e2e131ef5",
              "sha256": "168451e895175ca48d5a13ff6932f2e546a9f2ff880f5cd8133332f89efa0ede",
              "size": 52534,
              "depends": [
                "libblas 3.9.0.*",
                "libgcc-ng >=7.5.0",
                "libgfortran-ng",
                "libgfortran4 >=7.5.0"
              ],
              "constrains": [],
              "track_features": "blas_netlib",
              "license": "BSD-3-Clause",
              "license_family": "BSD",
              "timestamp": 1603051949121,
              "fn": "libcblas-3.9.0-1_h6e990d7_netlib.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libcblas-3.9.0-1_h6e990d7_netlib.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "numpy",
              "version": "1.24.2",
              "build": "py39h60c9533_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "2c79957b070d5e10eaa969c30a302674",
              "sha256": "1858ac0e9790481d64aea079122d52eea4db45640fd98af1005b64e0679cb275",
              "size": 6581101,
              "depends": [
                "libblas >=3.9.0,<4.0a0",
                "libcblas >=3.9.0,<4.0a0",
                "libgcc-ng >=12",
                "liblapack >=3.9.0,<4.0a0",
                "libstdcxx-ng >=12",
                "pypy3.9 >=7.3.11",
                "python >=3.9,<3.10.0a0",
                "python_abi 3.9 *_pypy39_pp73"
              ],
              "constrains": [
                "numpy-base <0a0"
              ],
              "license": "BSD-3-Clause",
              "license_family": "BSD",
              "timestamp": 1675642919186,
              "fn": "numpy-1.24.2-py39h60c9533_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/numpy-1.24.2-py39h60c9533_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            }
          ]"#;

        serde_json::from_str(repodata_json).unwrap()
    }

    fn get_resolved_packages_for_python() -> Vec<RepoDataRecord> {
        let repodata_json = r#"[
            {
              "name": "python",
              "version": "3.11.0",
              "build": "ha86cf86_0_cpython",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "531b2b97ce96cc95a587bdf7c74e31c0",
              "sha256": "60cd4d442f851efd46640f7c212110721921f0ee9c664ea0d1c339567a82d7a3",
              "size": 37622688,
              "depends": [
                "bzip2 >=1.0.8,<2.0a0",
                "ld_impl_linux-64 >=2.36.1",
                "libffi >=3.4.2,<3.5.0a0",
                "libgcc-ng >=12",
                "libnsl >=2.0.0,<2.1.0a0",
                "libsqlite >=3.39.4,<4.0a0",
                "libuuid >=2.32.1,<3.0a0",
                "libzlib >=1.2.13,<1.3.0a0",
                "ncurses >=6.3,<7.0a0",
                "openssl >=3.0.5,<4.0a0",
                "readline >=8.1.2,<9.0a0",
                "tk >=8.6.12,<8.7.0a0",
                "tzdata",
                "xz >=5.2.6,<5.3.0a0"
              ],
              "constrains": [
                "python_abi 3.11.* *_cp311"
              ],
              "license": "Python-2.0",
              "timestamp": 1666680856127,
              "fn": "python-3.11.0-ha86cf86_0_cpython.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/python-3.11.0-ha86cf86_0_cpython.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "xz",
              "version": "5.2.6",
              "build": "h166bdaf_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "2161070d867d1b1204ea749c8eec4ef0",
              "sha256": "03a6d28ded42af8a347345f82f3eebdd6807a08526d47899a42d62d319609162",
              "size": 418368,
              "depends": [
                "libgcc-ng >=12"
              ],
              "constrains": [],
              "license": "LGPL-2.1 and GPL-2.0",
              "timestamp": 1660346797927,
              "fn": "xz-5.2.6-h166bdaf_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/xz-5.2.6-h166bdaf_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libnsl",
              "version": "2.0.0",
              "build": "h7f98852_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "39b1328babf85c7c3a61636d9cd50206",
              "sha256": "32f4fb94d99946b0dabfbbfd442b25852baf909637f2eed1ffe3baea15d02aad",
              "size": 31236,
              "depends": [
                "libgcc-ng >=9.4.0"
              ],
              "constrains": [],
              "license": "GPL-2.0-only",
              "license_family": "GPL",
              "timestamp": 1633040059627,
              "fn": "libnsl-2.0.0-h7f98852_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libnsl-2.0.0-h7f98852_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libzlib",
              "version": "1.2.13",
              "build": "h166bdaf_4",
              "build_number": 4,
              "subdir": "linux-64",
              "md5": "f3f9de449d32ca9b9c66a22863c96f41",
              "sha256": "22f3663bcf294d349327e60e464a51cd59664a71b8ed70c28a9f512d10bc77dd",
              "size": 65503,
              "depends": [
                "libgcc-ng >=12"
              ],
              "constrains": [
                "zlib 1.2.13 *_4"
              ],
              "license": "Zlib",
              "license_family": "Other",
              "timestamp": 1665759624457,
              "fn": "libzlib-1.2.13-h166bdaf_4.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libzlib-1.2.13-h166bdaf_4.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "tk",
              "version": "8.6.12",
              "build": "h27826a3_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "5b8c42eb62e9fc961af70bdd6a26e168",
              "sha256": "032fd769aad9d4cad40ba261ab222675acb7ec951a8832455fce18ef33fa8df0",
              "size": 3456292,
              "depends": [
                "libgcc-ng >=9.4.0",
                "libzlib >=1.2.11,<1.3.0a0"
              ],
              "constrains": [],
              "license": "TCL",
              "license_family": "BSD",
              "timestamp": 1645033615058,
              "fn": "tk-8.6.12-h27826a3_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/tk-8.6.12-h27826a3_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "readline",
              "version": "8.1.2",
              "build": "h0f457ee_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "db2ebbe2943aae81ed051a6a9af8e0fa",
              "sha256": "f5f383193bdbe01c41cb0d6f99fec68e820875e842e6e8b392dbe1a9b6c43ed8",
              "size": 298080,
              "depends": [
                "libgcc-ng >=12",
                "ncurses >=6.3,<7.0a0"
              ],
              "constrains": [],
              "license": "GPL-3.0-only",
              "license_family": "GPL",
              "timestamp": 1654822435090,
              "fn": "readline-8.1.2-h0f457ee_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/readline-8.1.2-h0f457ee_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "ncurses",
              "version": "6.3",
              "build": "h9c3ff4c_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "fb31bcb7af058244479ca635d20f0f4a",
              "sha256": "bcb38449634bfe58e821c28d6814795b5bbad73514f0c7a9af7a710bbffc8243",
              "size": 1036278,
              "depends": [
                "libgcc-ng >=9.4.0"
              ],
              "constrains": [],
              "license": "X11 AND BSD-3-Clause",
              "timestamp": 1641758190772,
              "fn": "ncurses-6.3-h9c3ff4c_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/ncurses-6.3-h9c3ff4c_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libgcc-ng",
              "version": "12.2.0",
              "build": "h65d4601_19",
              "build_number": 19,
              "subdir": "linux-64",
              "md5": "e4c94f80aef025c17ab0828cd85ef535",
              "sha256": "f3899c26824cee023f1e360bd0859b0e149e2b3e8b1668bc6dd04bfc70dcd659",
              "size": 953812,
              "depends": [
                "_libgcc_mutex 0.1 conda_forge",
                "_openmp_mutex >=4.5"
              ],
              "constrains": [
                "libgomp 12.2.0 h65d4601_19"
              ],
              "license": "GPL-3.0-only WITH GCC-exception-3.1",
              "license_family": "GPL",
              "timestamp": 1666519671227,
              "fn": "libgcc-ng-12.2.0-h65d4601_19.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libgcc-ng-12.2.0-h65d4601_19.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "_libgcc_mutex",
              "version": "0.1",
              "build": "conda_forge",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "d7c89558ba9fa0495403155b64376d81",
              "sha256": "fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726",
              "size": 2562,
              "depends": [],
              "constrains": [],
              "license": "None",
              "timestamp": 1578324546067,
              "fn": "_libgcc_mutex-0.1-conda_forge.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "bzip2",
              "version": "1.0.8",
              "build": "h7f98852_4",
              "build_number": 4,
              "subdir": "linux-64",
              "md5": "a1fd65c7ccbf10880423d82bca54eb54",
              "sha256": "cb521319804640ff2ad6a9f118d972ed76d86bea44e5626c09a13d38f562e1fa",
              "size": 495686,
              "depends": [
                "libgcc-ng >=9.3.0"
              ],
              "constrains": [],
              "license": "bzip2-1.0.6",
              "license_family": "BSD",
              "timestamp": 1606604745109,
              "fn": "bzip2-1.0.8-h7f98852_4.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/bzip2-1.0.8-h7f98852_4.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libffi",
              "version": "3.4.2",
              "build": "h9c3ff4c_2",
              "build_number": 2,
              "subdir": "linux-64",
              "md5": "cb832368fd30ed8b58c6750fe8c3bb74",
              "sha256": "382da0f9ffbaecad8385d93112d1e1d4cd30a1df99a8e89c308cdc4add237640",
              "size": 61735,
              "depends": [
                "libgcc-ng >=9.4.0",
                "libstdcxx-ng >=9.4.0"
              ],
              "constrains": [],
              "license": "MIT",
              "license_family": "MIT",
              "timestamp": 1632174087618,
              "fn": "libffi-3.4.2-h9c3ff4c_2.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libffi-3.4.2-h9c3ff4c_2.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "openssl",
              "version": "3.0.8",
              "build": "h0b41bf4_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "e043403cd18faf815bf7705ab6c1e092",
              "sha256": "cd981c5c18463bc7a164fcf45c5cf697d58852b780b4dfa5e83c18c1fda6d7cd",
              "size": 2601535,
              "depends": [
                "ca-certificates",
                "libgcc-ng >=12"
              ],
              "constrains": [],
              "license": "Apache-2.0",
              "license_family": "Apache",
              "timestamp": 1675814291854,
              "fn": "openssl-3.0.8-h0b41bf4_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/openssl-3.0.8-h0b41bf4_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "ld_impl_linux-64",
              "version": "2.40",
              "build": "h41732ed_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "7aca3059a1729aa76c597603f10b0dd3",
              "sha256": "f6cc89d887555912d6c61b295d398cff9ec982a3417d38025c45d5dd9b9e79cd",
              "size": 704696,
              "depends": [],
              "constrains": [
                "binutils_impl_linux-64 2.40"
              ],
              "license": "GPL-3.0-only",
              "license_family": "GPL",
              "timestamp": 1674833944779,
              "fn": "ld_impl_linux-64-2.40-h41732ed_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/ld_impl_linux-64-2.40-h41732ed_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libuuid",
              "version": "2.32.1",
              "build": "h7f98852_1000",
              "build_number": 1000,
              "subdir": "linux-64",
              "md5": "772d69f030955d9646d3d0eaf21d859d",
              "sha256": "54f118845498353c936826f8da79b5377d23032bcac8c4a02de2019e26c3f6b3",
              "size": 28284,
              "depends": [
                "libgcc-ng >=9.3.0"
              ],
              "constrains": [],
              "license": "BSD-3-Clause",
              "license_family": "BSD",
              "timestamp": 1607292654633,
              "fn": "libuuid-2.32.1-h7f98852_1000.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libuuid-2.32.1-h7f98852_1000.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libsqlite",
              "version": "3.40.0",
              "build": "h753d276_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "2e5f9a37d487e1019fd4d8113adb2f9f",
              "sha256": "6008a0b914bd1a3510a3dba38eada93aa0349ebca3a21e5fa276833c8205bf49",
              "size": 810493,
              "depends": [
                "libgcc-ng >=12",
                "libzlib >=1.2.13,<1.3.0a0"
              ],
              "constrains": [],
              "license": "Unlicense",
              "timestamp": 1668697355661,
              "fn": "libsqlite-3.40.0-h753d276_0.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libsqlite-3.40.0-h753d276_0.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "tzdata",
              "version": "2022g",
              "build": "h191b570_0",
              "build_number": 0,
              "subdir": "noarch",
              "md5": "51fc4fcfb19f5d95ffc8c339db5068e8",
              "sha256": "0bfae0b9962bc0dbf79048f9175b913ed4f53c4310d06708dc7acbb290ad82f6",
              "size": 108083,
              "depends": [],
              "constrains": [],
              "noarch": "generic",
              "license": "LicenseRef-Public-Domain",
              "timestamp": 1669765202563,
              "fn": "tzdata-2022g-h191b570_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/noarch/tzdata-2022g-h191b570_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "_openmp_mutex",
              "version": "4.5",
              "build": "1_llvm",
              "build_number": 1,
              "subdir": "linux-64",
              "md5": "fa8d764c883a53d22ce622bea830c818",
              "sha256": "336db438c84eca10d0765ab81bd0bce677dbc0ab03c136ecf27ed05028397660",
              "size": 5187,
              "depends": [
                "_libgcc_mutex 0.1 conda_forge",
                "llvm-openmp >=9.0.1"
              ],
              "constrains": [],
              "license": "BSD-3-Clause",
              "license_family": "BSD",
              "timestamp": 1582045300613,
              "fn": "_openmp_mutex-4.5-1_llvm.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/_openmp_mutex-4.5-1_llvm.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "libstdcxx-ng",
              "version": "12.2.0",
              "build": "h46fd767_18",
              "build_number": 18,
              "subdir": "linux-64",
              "md5": "f19e96f96cc89617da02fed96f28974c",
              "sha256": "bc9180d1d5dcb253d89a03ed4ba30877d43dcd3ab77b2ba7cd0bb648edc6176f",
              "size": 4497047,
              "depends": [],
              "constrains": [],
              "license": "GPL-3.0-only WITH GCC-exception-3.1",
              "license_family": "GPL",
              "timestamp": 1665987640909,
              "fn": "libstdcxx-ng-12.2.0-h46fd767_18.tar.bz2",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/libstdcxx-ng-12.2.0-h46fd767_18.tar.bz2",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "ca-certificates",
              "version": "2022.12.7",
              "build": "ha878542_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "ff9f73d45c4a07d6f424495288a26080",
              "sha256": "8f6c81b0637771ae0ea73dc03a6d30bec3326ba3927f2a7b91931aa2d59b1789",
              "size": 145992,
              "depends": [],
              "constrains": [],
              "license": "ISC",
              "timestamp": 1670457595707,
              "fn": "ca-certificates-2022.12.7-ha878542_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/ca-certificates-2022.12.7-ha878542_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            },
            {
              "name": "llvm-openmp",
              "version": "15.0.7",
              "build": "h0cdce71_0",
              "build_number": 0,
              "subdir": "linux-64",
              "md5": "589c9a3575a050b583241c3d688ad9aa",
              "sha256": "7c67d383a8b1f3e7bf9e046e785325c481f6868194edcfb9d78d261da4ad65d4",
              "size": 3268766,
              "depends": [
                "libzlib >=1.2.13,<1.3.0a0"
              ],
              "constrains": [
                "openmp 15.0.7|15.0.7.*"
              ],
              "license": "Apache-2.0 WITH LLVM-exception",
              "license_family": "APACHE",
              "timestamp": 1673584331056,
              "fn": "llvm-openmp-15.0.7-h0cdce71_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/linux-64/llvm-openmp-15.0.7-h0cdce71_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
            }
          ]"#;

        serde_json::from_str(repodata_json).unwrap()
    }

    fn get_resolved_packages_for_rootless_graph() -> Vec<RepoDataRecord> {
        // Pip depends on Python and Python depends on Pip since Python 3.12
        // Below is a simplified version of their repodata.
        let repodata_json = r#"[
          {
              "build": "pyhd8ed1ab_0",
              "build_number": 0,
              "depends": [
                  "python >=3.7"
              ],
              "license": "MIT",
              "license_family": "MIT",
              "md5": "f586ac1e56c8638b64f9c8122a7b8a67",
              "name": "pip",
              "noarch": "python",
              "sha256": "b7c1c5d8f13e8cb491c4bd1d0d1896a4cf80fc47de01059ad77509112b664a4a",
              "size": 1398245,
              "subdir": "noarch",
              "timestamp": 1706960660581,
              "version": "24.0",
              "fn": "pip-24.0-pyhd8ed1ab_0.conda",
              "url": "https://conda.anaconda.org/conda-forge/noarch/pip-24.0-pyhd8ed1ab_0.conda",
              "channel": "https://conda.anaconda.org/conda-forge/"
          },
          {
            "build": "h1411813_0_cpython",
            "build_number": 0,
            "constrains": [
                "python_abi 3.12.* *_cp312"
            ],
            "depends": [
                "pip"
            ],
            "license": "Python-2.0",
            "md5": "df1448ec6cbf8eceb03d29003cf72ae6",
            "name": "python",
            "sha256": "3b327ffc152a245011011d1d730781577a8274fde1cf6243f073749ead8f1c2a",
            "size": 14557341,
            "subdir": "osx-64",
            "timestamp": 1713208068012,
            "version": "3.12.3",
            "fn": "python-3.12.3-h1411813_0_cpython.conda",
            "url": "https://conda.anaconda.org/conda-forge/osx-64/python-3.12.3-h1411813_0_cpython.conda",
            "channel": "https://conda.anaconda.org/conda-forge/"
        }
      ]"#;

        serde_json::from_str(repodata_json).unwrap()
    }

    fn get_resolved_packages_for_python_pip() -> Vec<RepoDataRecord> {
        let pip = r#"
        [
          {
            "arch": null,
            "build": "pyhd8ed1ab_0",
            "build_number": 0,
            "build_string": "pyhd8ed1ab_0",
            "channel": "https://conda.anaconda.org/conda-forge/noarch",
            "constrains": [],
            "depends": [
              "python >=3.7",
              "setuptools",
              "wheel"
            ],
            "fn": "pip-23.1.2-pyhd8ed1ab_0.conda",
            "license": "MIT",
            "license_family": "MIT",
            "md5": "7288da0d36821349cf1126e8670292df",
            "name": "pip",
            "noarch": "python",
            "platform": null,
            "sha256": "4fe1f47f6eac5b2635a622b6f985640bf835843c1d8d7ccbbae0f7d27cadec92",
            "size": 1367644,
            "subdir": "noarch",
            "timestamp": 1682507713321,
            "track_features": "",
            "url": "https://conda.anaconda.org/conda-forge/noarch/pip-23.1.2-pyhd8ed1ab_0.conda",
            "version": "23.1.2"
          },
          {
            "arch": null,
            "build": "pyhd8ed1ab_0",
            "build_number": 0,
            "build_string": "pyhd8ed1ab_0",
            "channel": "https://conda.anaconda.org/conda-forge/noarch",
            "constrains": [],
            "depends": [
              "python >=3.7"
            ],
            "fn": "wheel-0.40.0-pyhd8ed1ab_0.conda",
            "license": "MIT",
            "md5": "49bb0d9e60ce1db25e151780331bb5f3",
            "name": "wheel",
            "noarch": "python",
            "platform": null,
            "sha256": "79b4d29b0c004014a2abd5fc2c9fcd35cc6256222b960c2a317a27c4b0d8884d",
            "size": 55729,
            "subdir": "noarch",
            "timestamp": 1678812153506,
            "track_features": "",
            "url": "https://conda.anaconda.org/conda-forge/noarch/wheel-0.40.0-pyhd8ed1ab_0.conda",
            "version": "0.40.0"
          },
          {
            "arch": null,
            "build": "pyhd8ed1ab_0",
            "build_number": 0,
            "build_string": "pyhd8ed1ab_0",
            "channel": "https://conda.anaconda.org/conda-forge/noarch",
            "constrains": [],
            "depends": [
              "python >=3.7"
            ],
            "fn": "setuptools-68.0.0-pyhd8ed1ab_0.conda",
            "license": "MIT",
            "license_family": "MIT",
            "md5": "5a7739d0f57ee64133c9d32e6507c46d",
            "name": "setuptools",
            "noarch": "python",
            "platform": null,
            "sha256": "083a0913f5b56644051f31ac40b4eeea762a88c00aa12437817191b85a753cec",
            "size": 463712,
            "subdir": "noarch",
            "timestamp": 1687527994911,
            "track_features": "",
            "url": "https://conda.anaconda.org/conda-forge/noarch/setuptools-68.0.0-pyhd8ed1ab_0.conda",
            "version": "68.0.0"
          }
        ]"#;

        let mut python = get_resolved_packages_for_python();
        let pip: Vec<RepoDataRecord> = serde_json::from_str(pip).unwrap();
        python.extend(pip);
        python
    }
}
