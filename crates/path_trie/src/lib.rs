use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Component, Path, PathBuf},
};

use fs_err as fs;
use indexmap::IndexSet;
use itertools::Itertools;

pub type PackageName = String;

pub type ToClobbers = Vec<(PathBuf, String)>;
pub type FromClobbers = Vec<(PathBuf, String)>;
pub type Changes = (ToClobbers, FromClobbers);

/// Struct to represent a path clobbered by multiple packages.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClobberedPath {
    /// The name of the package that ultimately owns the file.
    pub winner: PackageName,

    /// Other packages whose files were overridden at this path.
    pub losers: Vec<PackageName>,
}

/// One node in the path-component trie.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
struct Node {
    /// All tags that touch this prefix *or* any descendant.
    prefixes: HashSet<PackageName>,
    /// Tags that have a file exactly at this node.
    terminals: HashSet<PackageName>,
    /// Child components.
    children: HashMap<String, Node>,
}

/// A trie of relative file-paths, tagged by package name (in insertion order).
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct PathTrie {
    root: Node,
    pub packages: IndexSet<String>,
}

impl PathTrie {
    /// Create an empty trie.
    pub fn new() -> Self {
        Self {
            root: Node::default(),
            packages: IndexSet::new(),
        }
    }

    /// Insert a file path under `package_name`.
    fn insert_file<P: AsRef<Path>>(&mut self, path: P, package_name: &str) {
        let path = path.as_ref();
        assert!(
            path.is_relative(),
            "All inserted paths must be relative; got {path:?}"
        );
        let mut node = &mut self.root;
        for comp in path.components().filter_map(|c| {
            if let Component::Normal(os) = c {
                os.to_str().map(ToOwned::to_owned)
            } else {
                None
            }
        }) {
            node.prefixes.insert(package_name.to_owned());
            node = node.children.entry(comp).or_default();
        }
        node.prefixes.insert(package_name.to_owned());
        node.terminals.insert(package_name.to_owned());
    }

    /// Get a mutable reference to the node at `path`, if it exists.
    fn get_node_mut<'a>(&'a mut self, path: &Path) -> Option<&'a mut Node> {
        let mut cur = &mut self.root;
        for comp in path.components().filter_map(|c| {
            if let Component::Normal(os) = c {
                os.to_str().map(ToOwned::to_owned)
            } else {
                None
            }
        }) {
            cur = cur.children.get_mut(&comp)?;
        }
        Some(cur)
    }

    /// Propagate a `package_name` into every descendant's `prefixes` set.
    fn propagate_prefix(node: &mut Node, package_name: &str) {
        node.prefixes.insert(package_name.to_owned());
        for child in node.children.values_mut() {
            Self::propagate_prefix(child, package_name);
        }
    }

    /// Insert a package files; return the new paths that conflict
    /// with what was already in the trie before this call.
    ///
    /// 1. **File vs File** at `p`: return `p`.
    /// 2. **Directory vs File** at `p`: return just `p`.
    /// 3. **File vs Directory** under some existing file `f`: return the new file’s `p`.
    /// 4. **Directory vs Directory**: no conflict.
    pub fn insert_package<P: AsRef<Path>>(
        &mut self,
        package: PackageName,
        paths: &[P],
    ) -> Vec<PathBuf> {
        // Record insertion order for future reprioritize.
        self.packages.insert(package.clone());

        let mut conflicts = HashSet::new();
        // Which of these paths were *directories* on the old trie?
        let mut dir_inserts = Vec::new();

        // 1) detect conflicts against the existing trie
        for p in paths {
            let p = p.as_ref();
            let pbuf = p.to_path_buf();

            // File vs File?
            if let Some(n) = Self::get_node(&self.root, &pbuf) {
                if !n.terminals.is_empty() {
                    conflicts.insert(pbuf.clone());
                    continue;
                }
            }
            // Directory vs File?
            if let Some(n) = Self::get_node(&self.root, &pbuf) {
                if !n.children.is_empty() {
                    conflicts.insert(pbuf.clone());
                    // Mark this as a directory-insert so we later propagate
                    dir_inserts.push(pbuf.clone());
                    continue;
                }
            }
            // File vs Directory under some prefix?
            let mut prefix = PathBuf::new();
            for comp in p.components().filter_map(|c| {
                if let Component::Normal(os) = c {
                    os.to_str().map(ToOwned::to_owned)
                } else {
                    None
                }
            }) {
                prefix.push(&comp);
                if prefix == pbuf {
                    break;
                }
                if let Some(n) = Self::get_node(&self.root, &prefix) {
                    if !n.terminals.is_empty() {
                        conflicts.insert(pbuf.clone());
                        break;
                    }
                }
            }
        }

        // 2) actually insert all files
        for p in paths {
            self.insert_file(p, &package);
        }

        // 3) propagate directory inserts into descendants
        for pbuf in dir_inserts {
            if let Some(n) = self.get_node_mut(&pbuf) {
                Self::propagate_prefix(n, &package);
            }
        }

        let mut out: Vec<_> = conflicts.into_iter().collect();
        out.sort();
        out
    }

    /// Unregister all paths belonging to `package`, then prune empty
    /// branches.
    ///
    /// Returns a change vectors.
    pub fn unregister_package(&mut self, package: &str) -> Changes {
        fn collect_next_candidate_paths(
            package: &str,
            candidates: &indexmap::set::Slice<String>,
            n: &Node,
            to_add: &mut Vec<(PathBuf, String)>,
            path: &Path,
            under_removed: bool,
        ) {
            // Determine if this node is part of the removed package's coverage.
            let removed_here = n.terminals.contains(package);
            let prefix_here = n.prefixes.contains(package);
            // Active if we're under a removed file or at a prefix of a removed package.
            let active = under_removed || removed_here || prefix_here;
            if !active {
                return;
            }

            // If any candidate has a file here, take the highest-priority one.
            for candidate in candidates {
                if n.terminals.contains(candidate) {
                    to_add.push((path.to_path_buf(), candidate.clone()));
                    break;
                }
            }

            // Prepare flag for deeper recursion: once under a removed file, stay under.
            let next_under = under_removed || removed_here;
            // Recurse into all child nodes under this covered subtree.
            for (comp, child) in &n.children {
                let mut new_path = path.to_path_buf();
                new_path.push(comp);
                collect_next_candidate_paths(
                    package, candidates, child, to_add, &new_path, next_under,
                );
            }
        }

        fn prune(n: &mut Node) -> bool {
            n.children.retain(|_, c| !prune(c));
            n.prefixes.is_empty() && n.terminals.is_empty() && n.children.is_empty()
        }

        fn rm(n: &mut Node, pkg: &str) {
            n.prefixes.remove(pkg);
            n.terminals.remove(pkg);
            for child in n.children.values_mut() {
                rm(child, pkg);
            }
        }

        if !self.packages.contains(package) {
            return Default::default();
        }

        let mut from_clobbers = vec![];
        let to_clobbers = vec![];

        if let Some(candidates) = self
            .packages
            .get_index_of(package)
            .and_then(|idx| self.packages.get_range(idx + 1..))
        {
            collect_next_candidate_paths(
                package,
                candidates,
                &self.root,
                &mut from_clobbers,
                Path::new(""),
                false,
            );
        }

        self.packages.shift_remove(package);

        rm(&mut self.root, package);

        self.root.children.retain(|_, c| !prune(c));

        (to_clobbers, from_clobbers)
    }

    /// Recompute active files based on old (insertion) vs new priority.
    pub fn reprioritize_packages(&mut self, new_order: &[PackageName]) -> Changes {
        fn rank<'a>(
            order: impl IntoIterator<Item = &'a PackageName>,
        ) -> HashMap<&'a PackageName, usize> {
            order.into_iter().enumerate().map(|(i, v)| (v, i)).collect()
        }

        fn collect_removed_subtree(
            n: &Node,
            cur: &mut PathBuf,
            old_rank: &HashMap<&PackageName, usize>,
            to_clobbers: &mut ToClobbers,
        ) {
            let old_winner = n
                .prefixes
                .iter()
                .max_by_key(|p| old_rank.get(p).copied().unwrap_or(usize::MAX));
            if let Some(p) = old_winner {
                if n.terminals.contains(p) {
                    to_clobbers.push((cur.clone(), p.clone()));
                }
            }
            for (comp, child) in &n.children {
                cur.push(comp);
                collect_removed_subtree(child, cur, old_rank, to_clobbers);
                cur.pop();
            }
        }

        fn collect_new_winners(
            n: &Node,
            cur: &mut PathBuf,
            new_rank: &HashMap<&PackageName, usize>,
            from_clobbers: &mut FromClobbers,
        ) {
            let new_winner = n
                .prefixes
                .iter()
                .max_by_key(|p| new_rank.get(p).copied().unwrap_or(usize::MAX));
            if let Some(p) = new_winner {
                if n.terminals.contains(p) {
                    from_clobbers.push((cur.clone(), p.clone()));
                    return; // don't descend, file shadows
                }
                for (comp, child) in &n.children {
                    cur.push(comp);
                    collect_new_winners(child, cur, new_rank, from_clobbers);
                    cur.pop();
                }
            }
        }

        fn dfs(
            n: &Node,
            cur: &mut PathBuf,
            old_rank: &HashMap<&PackageName, usize>,
            new_rank: &HashMap<&PackageName, usize>,
            to_clobbers: &mut ToClobbers,
            from_clobbers: &mut FromClobbers,
        ) {
            let old_winner = n
                .prefixes
                .iter()
                .max_by_key(|p| old_rank.get(p).copied().unwrap_or(usize::MAX));
            let new_winner = n
                .prefixes
                .iter()
                .max_by_key(|p| new_rank.get(p).copied().unwrap_or(usize::MAX));

            let old_is_file = old_winner.is_some_and(|p| n.terminals.contains(p));
            let new_is_file = new_winner.is_some_and(|p| n.terminals.contains(p));

            match (old_is_file, new_is_file) {
                (true, true) => {
                    let old = old_winner.unwrap();
                    let new = new_winner.unwrap();
                    if old != new {
                        to_clobbers.push((cur.clone(), old.clone()));
                        from_clobbers.push((cur.clone(), new.clone()));
                    }
                    // don't descend
                }
                (false, true) => {
                    let new = new_winner.unwrap();
                    from_clobbers.push((cur.clone(), new.clone()));
                    collect_removed_subtree(n, cur, old_rank, to_clobbers);
                    // don't descend
                }
                (true, false) => {
                    let old = old_winner.unwrap();
                    to_clobbers.push((cur.clone(), old.clone()));
                    collect_new_winners(n, cur, new_rank, from_clobbers);
                    // don't descend further: new_winners handles it
                }
                (false, false) => {
                    // Recurse normally
                    for (comp, child) in &n.children {
                        cur.push(comp);
                        dfs(child, cur, old_rank, new_rank, to_clobbers, from_clobbers);
                        cur.pop();
                    }
                }
            }
        }

        {
            let sorted_packages = self
                .packages
                .iter()
                .cloned()
                .sorted()
                .collect::<Vec<PackageName>>();

            let new_sorted_packages = new_order
                .iter()
                .cloned()
                .sorted()
                .collect::<Vec<PackageName>>();

            assert_eq!(
                &sorted_packages, &new_sorted_packages,
                "Expected just reordering, got something else.
Old:
{sorted_packages:#?}
New:
{new_sorted_packages:#?}
"
            );
        }

        // Package with highest priority will have biggest number.
        let old_rank = rank(self.packages.iter().rev());
        let new_rank = rank(new_order.iter());

        let mut to_clobbers = Vec::new();
        let mut from_clobbers = Vec::new();

        let mut buf = PathBuf::new();
        dfs(
            &self.root,
            &mut buf,
            &old_rank,
            &new_rank,
            &mut to_clobbers,
            &mut from_clobbers,
        );

        self.packages.clear();
        for pkg in new_order.iter().rev() {
            self.packages.insert(pkg.clone());
        }

        (to_clobbers, from_clobbers)
    }

    /// Move files on-disk:
    ///
    /// - For each `(p,pkg)` in `to_clobbers`:  `base/p` → `clobbers/pkg/p` if `base/p` exists and dest doesn’t.
    /// - For each `(p,pkg)` in `from_clobers`: `clobbers/pkg/p` → `base/p` if source exists and dest doesn’t.
    pub fn sync_clobbers(
        target_prefix: &Path,
        clobbers_dir: &Path,
        to_clobbers: &[(PathBuf, PackageName)],
        from_clobbers: &[(PathBuf, PackageName)],
    ) -> io::Result<()> {
        fn mv(src: PathBuf, dst: PathBuf) -> io::Result<()> {
            tracing::trace!("Moving from {} to {}", src.display(), dst.display());
            if let Some(p) = dst.parent() {
                fs::create_dir_all(p)?;
            }
            fs::rename(src, dst)
        }
        for (p, pkg) in to_clobbers {
            let src = target_prefix.join(p);
            let dst = clobbers_dir.join(pkg).join(p);
            if src.exists() && !dst.exists() {
                mv(src, dst)?;
            }
        }
        for (p, pkg) in from_clobbers {
            let src = clobbers_dir.join(pkg).join(p);
            let dst = target_prefix.join(p);
            if src.exists() && !dst.exists() {
                mv(src, dst)?;
            }
        }
        Ok(())
    }

    /// Which packages own this prefix?
    pub fn packages_for_prefix<P: AsRef<Path>>(&self, path: P) -> Option<&HashSet<PackageName>> {
        let mut cur = &self.root;
        for comp in path.as_ref().components().filter_map(|c| match c {
            Component::Normal(os) => os.to_str().map(ToOwned::to_owned),
            _ => None,
        }) {
            cur = cur.children.get(&comp)?;
        }
        Some(&cur.prefixes)
    }

    /// Who owns exactly this file?
    pub fn packages_for_exact<P: AsRef<Path>>(&self, path: P) -> Option<&HashSet<PackageName>> {
        Self::get_node(&self.root, path.as_ref()).map(|n| &n.terminals)
    }

    /// List global file-vs-file conflicts (>1 owners).
    pub fn find_conflicts(&self) -> Vec<(PathBuf, Vec<PackageName>)> {
        fn dfs(n: &Node, cur: &mut PathBuf, out: &mut Vec<(PathBuf, Vec<PackageName>)>) {
            if n.terminals.len() > 1 {
                out.push((cur.clone(), n.terminals.iter().cloned().collect()));
            }
            for (c, child) in &n.children {
                cur.push(c);
                dfs(child, cur, out);
                cur.pop();
            }
        }

        let mut out = Vec::new();
        let mut buf = PathBuf::new();
        dfs(&self.root, &mut buf, &mut out);
        out
    }

    /// Internal: get an immutable node for exact path.
    fn get_node<'a>(root: &'a Node, path: &Path) -> Option<&'a Node> {
        let mut cur = root;
        for comp in path.components().filter_map(|c| match c {
            Component::Normal(os) => os.to_str().map(ToOwned::to_owned),
            _ => None,
        }) {
            cur = cur.children.get(&comp)?;
        }
        Some(cur)
    }

    /// Collect all paths where multiple packages wrote the same file,
    /// returning a map from each path to its final owner and overridden packages.
    pub fn collect_clobbered_paths(&self) -> HashMap<PathBuf, ClobberedPath> {
        fn dfs(
            node: &Node,
            path: &Path,
            packages: &IndexSet<String>,
            results: &mut HashMap<PathBuf, ClobberedPath>,
        ) {
            if !node.terminals.is_empty() {
                // Determine the winning package by insertion priority
                if let Some(winner) = packages
                    .iter()
                    .find(|pkg| node.terminals.contains(pkg.as_str()))
                {
                    // Collect all other packages that wrote to this path
                    let others: Vec<PackageName> = node
                        .terminals
                        .iter()
                        .filter(|&p| p != winner)
                        .cloned()
                        .collect();
                    if !others.is_empty() {
                        results.insert(
                            path.to_path_buf(),
                            ClobberedPath {
                                winner: winner.clone(),
                                losers: others,
                            },
                        );
                    }
                }
            }
            for (comp, child) in &node.children {
                let mut new_path = path.to_path_buf();
                new_path.push(comp);
                dfs(child, &new_path, packages, results);
            }
        }

        let mut results = HashMap::new();
        dfs(&self.root, Path::new(""), &self.packages, &mut results);
        results
    }
}

// TODO: Write property based tests.
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        fs::File,
        io::{Read, Write},
        path::PathBuf,
    };
    use tempfile::TempDir;

    #[test]
    fn test_insert_file_vs_file_conflict() {
        let mut trie = PathTrie::new();
        assert!(trie.insert_package("pkg1".into(), &["foo.txt"]).is_empty());
        let conflicts = trie.insert_package("pkg2".into(), &["foo.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("foo.txt")]);
    }

    #[test]
    fn test_insert_nested_file_vs_file_conflict() {
        let mut trie = PathTrie::new();
        assert!(trie
            .insert_package("pkg1".into(), &["foo/bar.txt"])
            .is_empty());
        let conflicts = trie.insert_package("pkg2".into(), &["foo/bar.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("foo/bar.txt")]);
    }

    #[test]
    fn test_insert_dir_vs_file_conflict() {
        let mut trie = PathTrie::new();
        assert!(trie
            .insert_package("pkg1".into(), &["foo/bar.txt", "foo/baz.txt"])
            .is_empty());
        let mut conflicts = trie.insert_package("pkg2".into(), &["foo"]);
        conflicts.sort();
        assert_eq!(conflicts, vec![PathBuf::from("foo")]);
    }

    #[test]
    fn test_insert_file_vs_dir_conflict() {
        let mut trie = PathTrie::new();
        assert!(trie.insert_package("pkg1".into(), &["foo"]).is_empty());
        let conflicts = trie.insert_package("pkg2".into(), &["foo/bar.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("foo/bar.txt")]);
    }

    #[test]
    fn test_no_conflict_on_sibling() {
        let mut trie = PathTrie::new();
        assert!(trie.insert_package("a".into(), &["a/x"]).is_empty());
        assert!(trie.insert_package("b".into(), &["b/y"]).is_empty());
    }

    #[test]
    fn test_no_conflict_on_dir_sibling() {
        let mut trie = PathTrie::new();
        assert!(trie.insert_package("a".into(), &["a/x"]).is_empty());
        assert!(trie.insert_package("b".into(), &["a/y"]).is_empty());
    }

    #[test]
    fn test_unregister_package() {
        let mut trie = PathTrie::new();
        let paths = ["foo.txt", "foo/bar.txt"];
        assert!(trie.insert_package("pkg".into(), &paths).is_empty());
        trie.unregister_package("pkg");
        assert!(trie
            .packages_for_exact("foo.txt")
            .is_none_or(HashSet::is_empty));
        assert!(trie.packages_for_prefix("foo").is_none());
    }

    #[test]
    fn test_reprioritize_noop() {
        let mut trie = PathTrie::new();
        trie.insert_package("pkg1".into(), &["a.txt"]);
        trie.insert_package("pkg2".into(), &["b.txt"]);
        let (removed, added) = trie.reprioritize_packages(&["pkg2".into(), "pkg1".into()]);
        assert!(removed.is_empty());
        assert!(added.is_empty());
    }

    #[test]
    fn test_reprioritize_file_vs_file() {
        let mut trie = PathTrie::new();
        trie.insert_package("pkg1".into(), &["foo.txt"]);
        trie.insert_package("pkg2".into(), &["foo.txt"]);
        let (removed, added) = trie.reprioritize_packages(&["pkg1".into(), "pkg2".into()]);
        assert_eq!(
            removed,
            vec![(PathBuf::from("foo.txt"), "pkg1".to_string())]
        );
        assert_eq!(added, vec![(PathBuf::from("foo.txt"), "pkg2".to_string())]);
    }

    #[test]
    fn test_reprioritize_file_vs_dir() {
        let mut trie = PathTrie::new();
        trie.insert_package("pkg1".into(), &["foo"]);
        trie.insert_package("pkg2".into(), &["foo/bar.txt"]);
        let (removed, added) = trie.reprioritize_packages(&["pkg1".into(), "pkg2".into()]);
        assert_eq!(removed, vec![(PathBuf::from("foo"), "pkg1".to_string())]);
        assert_eq!(
            added,
            vec![(PathBuf::from("foo/bar.txt"), "pkg2".to_string())]
        );
    }

    #[test]
    fn test_reprioritize_dir_vs_file() {
        let mut trie = PathTrie::new();
        trie.insert_package("pkg1".into(), &["foo/bar.txt"]);
        trie.insert_package("pkg2".into(), &["foo"]);
        let (removed, added) = trie.reprioritize_packages(&["pkg1".into(), "pkg2".into()]);
        assert_eq!(
            removed,
            vec![(PathBuf::from("foo/bar.txt"), "pkg1".to_string())]
        );
        assert_eq!(added, vec![(PathBuf::from("foo"), "pkg2".to_string())]);
    }

    #[test]
    fn test_reprioritize_dir_vs_dir() {
        let mut trie = PathTrie::new();
        trie.insert_package("pkg1".into(), &["foo/bar1.txt", "foo/bar2.txt"]);
        let mut conflict = trie.insert_package("pkg2".into(), &["foo/bar2.txt"]);
        conflict.sort();
        assert_eq!(conflict, vec![PathBuf::from("foo/bar2.txt")]);
        let (removed, added) = trie.reprioritize_packages(&["pkg1".into(), "pkg2".into()]);
        // pkg1 was winner and now pkg2 is a winner
        assert_eq!(
            removed,
            vec![(PathBuf::from("foo/bar2.txt"), "pkg1".into())],
        );
        assert_eq!(
            added,
            vec![(PathBuf::from("foo/bar2.txt"), "pkg2".to_string())]
        );
    }

    #[test]
    fn test_reprioritize_file_vs_dir_vs_dir_with_permuted_insertion_order() {
        let priority_order = &["pkg1".to_string(), "pkg2".into(), "pkg3".into()];

        // 1
        let pkgs: &[(String, &[&str])] = &[
            ("pkg1".into(), &["foo"]),
            ("pkg2".into(), &["foo/bar1.txt", "foo/bar2.txt"]),
            ("pkg3".into(), &["foo/bar2.txt"]),
        ];

        let mut trie = PathTrie::new();
        for (pkg_name, paths) in pkgs {
            trie.insert_package(pkg_name.into(), paths);
        }

        let (removed, mut added) = trie.reprioritize_packages(priority_order);
        assert_eq!(removed, vec![(PathBuf::from("foo"), "pkg1".into())],);
        added.sort();
        assert_eq!(
            added,
            vec![
                (PathBuf::from("foo/bar1.txt"), "pkg2".to_string()),
                (PathBuf::from("foo/bar2.txt"), "pkg3".to_string())
            ]
        );

        // 2
        let pkgs: &[(String, &[&str])] = &[
            ("pkg1".into(), &["foo"]),
            ("pkg3".into(), &["foo/bar2.txt"]),
            ("pkg2".into(), &["foo/bar1.txt", "foo/bar2.txt"]),
        ];

        let mut trie = PathTrie::new();
        for (pkg_name, paths) in pkgs {
            trie.insert_package(pkg_name.into(), paths);
        }

        let (removed, mut added) = trie.reprioritize_packages(priority_order);
        assert_eq!(removed, vec![(PathBuf::from("foo"), "pkg1".into())],);
        added.sort();
        assert_eq!(
            added,
            vec![
                (PathBuf::from("foo/bar1.txt"), "pkg2".to_string()),
                (PathBuf::from("foo/bar2.txt"), "pkg3".to_string())
            ]
        );

        // 3
        let pkgs: &[(String, &[&str])] = &[
            ("pkg2".into(), &["foo/bar1.txt", "foo/bar2.txt"]),
            ("pkg1".into(), &["foo"]),
            ("pkg3".into(), &["foo/bar2.txt"]),
        ];

        let mut trie = PathTrie::new();
        for (pkg_name, paths) in pkgs {
            trie.insert_package(pkg_name.into(), paths);
        }

        let (removed, added) = trie.reprioritize_packages(priority_order);
        assert_eq!(
            removed,
            vec![(PathBuf::from("foo/bar2.txt"), "pkg2".into())],
        );
        assert_eq!(
            added,
            vec![(PathBuf::from("foo/bar2.txt"), "pkg3".to_string())]
        );

        // 4
        let pkgs: &[(String, &[&str])] = &[
            ("pkg2".into(), &["foo/bar1.txt", "foo/bar2.txt"]),
            ("pkg3".into(), &["foo/bar2.txt"]),
            ("pkg1".into(), &["foo"]),
        ];

        let mut trie = PathTrie::new();
        for (pkg_name, paths) in pkgs {
            trie.insert_package(pkg_name.into(), paths);
        }

        let (removed, added) = trie.reprioritize_packages(priority_order);
        assert_eq!(
            removed,
            vec![(PathBuf::from("foo/bar2.txt"), "pkg2".into())],
        );
        assert_eq!(
            added,
            vec![(PathBuf::from("foo/bar2.txt"), "pkg3".to_string())]
        );

        // 5
        let pkgs: &[(String, &[&str])] = &[
            ("pkg3".into(), &["foo/bar2.txt"]),
            ("pkg1".into(), &["foo"]),
            ("pkg2".into(), &["foo/bar1.txt", "foo/bar2.txt"]),
        ];

        let mut trie = PathTrie::new();
        for (pkg_name, paths) in pkgs {
            trie.insert_package(pkg_name.into(), paths);
        }

        let (removed, added) = trie.reprioritize_packages(priority_order);
        assert!(removed.is_empty());
        assert!(added.is_empty());

        // 6
        let pkgs: &[(String, &[&str])] = &[
            ("pkg3".into(), &["foo/bar2.txt"]),
            ("pkg2".into(), &["foo/bar1.txt", "foo/bar2.txt"]),
            ("pkg1".into(), &["foo"]),
        ];

        let mut trie = PathTrie::new();
        for (pkg_name, paths) in pkgs {
            trie.insert_package(pkg_name.into(), paths);
        }

        let (removed, added) = trie.reprioritize_packages(priority_order);
        assert!(removed.is_empty());
        assert!(added.is_empty());
    }

    #[test]
    fn test_tags_queries() {
        let mut trie = PathTrie::new();
        trie.insert_package("pkg".into(), &["d1/f1.txt", "d1/f2.txt", "d2/f3.txt"]);
        let p1 = trie.packages_for_prefix("d1").unwrap();
        assert_eq!(p1.len(), 1);
        assert!(p1.contains("pkg"));
        let e = trie.packages_for_exact("d1/f2.txt").unwrap();
        assert_eq!(e.len(), 1);
        assert!(e.contains("pkg"));
    }

    #[test]
    fn test_sync_clobbers_file_vs_file() {
        let tmp = TempDir::new().unwrap();
        let target_prefix = tmp.path();
        let clobbers = tmp.path().join("__clobbers__");
        fs::create_dir_all(target_prefix).unwrap();
        fs::create_dir_all(clobbers.join("pkg2")).unwrap();

        File::create(target_prefix.join("foo.txt"))
            .unwrap()
            .write_all(b"pkg1")
            .unwrap();
        File::create(clobbers.join("pkg2").join("foo.txt"))
            .unwrap()
            .write_all(b"pkg2")
            .unwrap();

        let mut trie = PathTrie::new();
        trie.insert_package("pkg1".into(), &["foo.txt"]);
        trie.insert_package("pkg2".into(), &["foo.txt"]);
        let (to_clobbers, from_clobbers) =
            trie.reprioritize_packages(&["pkg1".into(), "pkg2".into()]);

        PathTrie::sync_clobbers(target_prefix, &clobbers, &to_clobbers, &from_clobbers).unwrap();

        let mut buf = String::new();
        File::open(target_prefix.join("foo.txt"))
            .unwrap()
            .read_to_string(&mut buf)
            .unwrap();
        assert_eq!(buf, "pkg2");

        let mut buf = String::new();
        File::open(clobbers.join("pkg1").join("foo.txt"))
            .unwrap()
            .read_to_string(&mut buf)
            .unwrap();
        assert_eq!(buf, "pkg1");
    }

    // TODO: Write more tests for unregister.
    #[test]
    fn test_insert_file_vs_dir_conflict_unregister() {
        let mut trie = PathTrie::new();
        assert!(trie.insert_package("pkg1".into(), &["foo"]).is_empty());
        trie.insert_package("pkg2".into(), &["foo/bar.txt"]);
        let moves = trie.unregister_package("pkg1");
        assert_eq!(
            moves,
            (vec![], vec![(PathBuf::from("foo/bar.txt"), "pkg2".into())])
        );
    }

    #[test]
    fn test_collect_clobbered_no_conflict() {
        let mut trie = PathTrie::new();
        assert!(trie.insert_package("pkg1".into(), &["a.txt"]).is_empty());

        let clobbered = trie.collect_clobbered_paths();
        assert!(clobbered.is_empty());
    }

    #[test]
    fn test_collect_clobbered_simple_conflict() {
        let mut trie = PathTrie::new();
        assert!(trie.insert_package("pkg1".into(), &["file.txt"]).is_empty());
        let conflicts = trie.insert_package("pkg2".into(), &["file.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("file.txt")]);

        let clobbered = trie.collect_clobbered_paths();
        assert_eq!(clobbered.len(), 1);
        let path = PathBuf::from("file.txt");
        let entry = clobbered.get(&path).expect("file.txt should be present");
        assert_eq!(entry.winner, "pkg1".to_string());
        assert_eq!(entry.losers, vec!["pkg2".to_string()]);
    }

    #[test]
    fn test_collect_clobbered_multiple_conflicts() {
        let mut trie = PathTrie::new();
        assert!(trie.insert_package("pkg1".into(), &["dup.txt"]).is_empty());
        let conflicts = trie.insert_package("pkg2".into(), &["dup.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("dup.txt")]);
        let conflicts = trie.insert_package("pkg3".into(), &["dup.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("dup.txt")]);

        let clobbered = trie.collect_clobbered_paths();
        assert_eq!(clobbered.len(), 1);
        let path = PathBuf::from("dup.txt");
        let entry = clobbered.get(&path).expect("dup.txt should be present");
        assert_eq!(entry.winner, "pkg1".to_string());
        let mut others = entry.losers.clone();
        others.sort();
        assert_eq!(others, vec!["pkg2".to_string(), "pkg3".to_string()]);
    }

    #[test]
    fn test_collect_clobbered_multiple_files() {
        let mut trie = PathTrie::new();
        assert!(trie
            .insert_package("pkg1".into(), &["a.txt", "b.txt"])
            .is_empty());
        let conflicts = trie.insert_package("pkg2".into(), &["a.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("a.txt")]);

        let clobbered = trie.collect_clobbered_paths();
        assert_eq!(clobbered.len(), 1);
        let path = PathBuf::from("a.txt");
        let entry = clobbered.get(&path).expect("a.txt should be clobbered");
        assert_eq!(
            entry,
            &ClobberedPath {
                winner: "pkg1".into(),
                losers: vec!["pkg2".into()]
            }
        );
    }

    #[test]
    fn test_collect_clobbered_directory_conflict() {
        let mut trie = PathTrie::new();
        assert!(trie.insert_package("pkg1".into(), &["dir"]).is_empty());
        let conflicts = trie.insert_package("pkg2".into(), &["dir"]);
        assert_eq!(conflicts, vec![PathBuf::from("dir")]);

        let clobbered = trie.collect_clobbered_paths();
        assert_eq!(clobbered.len(), 1);
        let path = PathBuf::from("dir");
        let entry = clobbered.get(&path).expect("dir should be clobbered");
        assert_eq!(
            entry,
            &ClobberedPath {
                winner: "pkg1".into(),
                losers: vec!["pkg2".into()]
            }
        );
    }
}
