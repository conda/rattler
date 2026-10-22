//! Deciding which package wins when multiple packages install the same path, for a prefix that
//! is installed in one go.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
};

use rattler_conda_types::PackageName;

use crate::install::clobber_registry::ClobberedPath;

/// The result of [`resolve_clobbers`].
#[derive(Debug, Clone, Default)]
pub struct ClobberResolution {
    /// For every package (in the order they were passed in), the paths that lose against a path
    /// of another package.
    pub losing_paths: Vec<HashSet<PathBuf>>,

    /// The paths that are installed by more than one package, with the package that wins.
    pub clobbered_paths: HashMap<PathBuf, ClobberedPath>,
}

#[derive(Default)]
struct Node {
    children: BTreeMap<OsString, Node>,
    /// The packages that install a file (or symlink) exactly at this path.
    terminals: Vec<usize>,
    /// The package with the highest priority that installs this path or anything below it.
    highest: usize,
}

/// Decides for every path that is installed by more than one package which package wins.
///
/// `packages` are the packages with the paths they install, in increasing priority (usually the
/// topological order of the environment). This mirrors the final layout of the regular installer:
///
/// - if multiple packages install a file at the same path, the package with the highest priority
///   wins.
/// - if one package installs a file at a path where another package installs a directory, the
///   package with the highest priority among all packages installing something at or below that
///   path decides whether the file or the directory wins.
pub fn resolve_clobbers<'a, P: IntoIterator<Item = &'a Path>>(
    packages: impl IntoIterator<Item = (&'a PackageName, P)>,
) -> ClobberResolution {
    let mut root = Node::default();
    let mut names = Vec::new();
    for (index, (name, paths)) in packages.into_iter().enumerate() {
        names.push(name);
        for path in paths {
            let mut node = &mut root;
            node.highest = index;
            for component in path.components() {
                node = node
                    .children
                    .entry(component.as_os_str().to_owned())
                    .or_default();
                node.highest = index;
            }
            if node.terminals.last() != Some(&index) {
                node.terminals.push(index);
            }
        }
    }

    let mut resolution = ClobberResolution {
        losing_paths: vec![HashSet::new(); names.len()],
        clobbered_paths: HashMap::new(),
    };
    let mut path = PathBuf::new();
    resolve(&root, &mut path, &names, &mut resolution);
    resolution
}

fn resolve(
    node: &Node,
    path: &mut PathBuf,
    names: &[&PackageName],
    resolution: &mut ClobberResolution,
) {
    let winner = node.highest;
    if node.terminals.contains(&winner) {
        // The file of the winner shadows everything else at and below this path.
        let mut losers = Vec::new();
        for &terminal in &node.terminals {
            if terminal != winner {
                resolution.losing_paths[terminal].insert(path.clone());
                losers.push(terminal);
            }
        }
        for (component, child) in &node.children {
            path.push(component);
            lose_all(child, path, &mut losers, resolution);
            path.pop();
        }
        if !losers.is_empty() {
            losers.sort_unstable();
            losers.dedup();
            resolution.clobbered_paths.insert(
                path.clone(),
                ClobberedPath {
                    package: names[winner].clone(),
                    other_packages: losers.into_iter().map(|i| names[i].clone()).collect(),
                },
            );
        }
        return;
    }

    // The winner installs a directory here, so files at this path lose.
    if !node.terminals.is_empty() {
        for &terminal in &node.terminals {
            resolution.losing_paths[terminal].insert(path.clone());
        }
        resolution.clobbered_paths.insert(
            path.clone(),
            ClobberedPath {
                package: names[winner].clone(),
                other_packages: node.terminals.iter().map(|&i| names[i].clone()).collect(),
            },
        );
    }
    for (component, child) in &node.children {
        path.push(component);
        resolve(child, path, names, resolution);
        path.pop();
    }
}

fn lose_all(
    node: &Node,
    path: &mut PathBuf,
    losers: &mut Vec<usize>,
    resolution: &mut ClobberResolution,
) {
    for &terminal in &node.terminals {
        resolution.losing_paths[terminal].insert(path.clone());
        losers.push(terminal);
    }
    for (component, child) in &node.children {
        path.push(component);
        lose_all(child, path, losers, resolution);
        path.pop();
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rattler_conda_types::PackageName;

    use super::resolve_clobbers;

    fn names(n: usize) -> Vec<PackageName> {
        (0..n)
            .map(|i| PackageName::new_unchecked(format!("pkg-{i}")))
            .collect()
    }

    #[test]
    fn last_package_wins_file_conflicts() {
        let names = names(3);
        let resolution = resolve_clobbers([
            (&names[0], vec![Path::new("bin/a"), Path::new("bin/b")]),
            (&names[1], vec![Path::new("bin/a")]),
            (&names[2], vec![Path::new("bin/c")]),
        ]);
        assert!(resolution.losing_paths[0].contains(Path::new("bin/a")));
        assert!(!resolution.losing_paths[0].contains(Path::new("bin/b")));
        assert!(resolution.losing_paths[1].is_empty());
        assert!(resolution.losing_paths[2].is_empty());
        let clobbered = &resolution.clobbered_paths[Path::new("bin/a")];
        assert_eq!(clobbered.package, names[1]);
        assert_eq!(clobbered.other_packages, vec![names[0].clone()]);
        assert_eq!(resolution.clobbered_paths.len(), 1);
    }

    #[test]
    fn file_wins_over_directory_of_lower_priority() {
        let names = names(2);
        let resolution = resolve_clobbers([
            (
                &names[0],
                vec![Path::new("lib/foo/a"), Path::new("lib/foo/b")],
            ),
            (&names[1], vec![Path::new("lib/foo")]),
        ]);
        assert!(resolution.losing_paths[0].contains(Path::new("lib/foo/a")));
        assert!(resolution.losing_paths[0].contains(Path::new("lib/foo/b")));
        assert!(resolution.losing_paths[1].is_empty());
    }

    #[test]
    fn directory_wins_over_file_of_lower_priority() {
        let names = names(2);
        let resolution = resolve_clobbers([
            (&names[0], vec![Path::new("lib/foo")]),
            (&names[1], vec![Path::new("lib/foo/a")]),
        ]);
        assert!(resolution.losing_paths[0].contains(Path::new("lib/foo")));
        assert!(resolution.losing_paths[1].is_empty());
    }
}
