#![deny(missing_docs)]
//! Provides a trie-based data structure to track and resolve relative path ownership across multiple packages.
//!
//! For details see methods of `PathResolver`.
use std::{
    collections::BTreeSet,
    ffi::OsString,
    io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use fs_err as fs;
use fxhash::{FxHashMap as HashMap, FxHashSet as HashSet};
use indexmap::IndexSet;
use itertools::Itertools;

/// Type to represent path owner. Using Rc<String> to avoid cloning overhead while maintaining ownership semantics.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageName(Arc<String>);

impl PackageName {
    /// Create a new `PackageName` from a string-like value.
    pub fn new(s: impl Into<String>) -> Self {
        Self(Arc::new(s.into()))
    }

    /// Get the package name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for PackageName {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for PackageName {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&String> for PackageName {
    fn from(s: &String) -> Self {
        Self::new(s)
    }
}

impl AsRef<str> for PackageName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<Path> for PackageName {
    fn as_ref(&self) -> &Path {
        let s: &str = self.as_ref();
        Path::new(s)
    }
}

impl std::fmt::Display for PackageName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for PackageName {
    type Target = str;
    
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Vector of files that we want to move to clobbers directory.
pub type ToClobbers = Vec<(PathBuf, PackageName)>;
/// Vector of files that we want to move from clobbers directory.
pub type FromClobbers = Vec<(PathBuf, PackageName)>;
/// Changes that we have to do to keep on-disk state in tact with what we have in-memory.
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
struct PathTrieNode {
    /// All tags that touch this prefix *or* any descendant.
    prefixes: HashSet<PackageName>,
    /// Tags that have a file exactly at this node.
    terminals: HashSet<PackageName>,
    /// Child components.
    children: HashMap<OsString, PathTrieNode>,
}

/// A trie of relative file-paths, tagged by package name (in insertion order).
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct PathResolver {
    root: PathTrieNode,
    packages: IndexSet<PackageName>,
}

impl PathResolver {
    /// Create an empty trie.
    pub fn new() -> Self {
        Self {
            root: PathTrieNode::default(),
            packages: IndexSet::new(),
        }
    }

    /// Return slice to packages. Note that it is in the order of decreasing priority.
    pub fn packages(&self) -> &IndexSet<PackageName> {
        &self.packages
    }

    /// Insert a file path under `package_name`.
    fn insert_file_owned<P: AsRef<Path>>(&mut self, path: P, package: PackageName) {
        let path = path.as_ref();
        assert!(
            path.is_relative(),
            "All inserted paths must be relative; got {path:?}"
        );

        let mut node = &mut self.root;
        for comp in path.components().map(|c| c.as_os_str().to_os_string()) {
            node.prefixes.insert(package.clone());
            node = node.children.entry(comp).or_default();
        }
        node.prefixes.insert(package.clone());
        node.terminals.insert(package);
    }


    /// Get a mutable reference to the node at `path`, if it exists.
    fn get_node_mut<'a>(&'a mut self, path: &Path) -> Option<&'a mut PathTrieNode> {
        let mut cur = &mut self.root;
        for comp in path.components().map(Component::as_os_str) {
            cur = cur.children.get_mut(comp)?;
        }
        Some(cur)
    }

    /// Propagate a `package_name` into every descendant's `prefixes` set.
    fn propagate_prefix(node: &mut PathTrieNode, package_name: PackageName) {
        node.prefixes.insert(package_name.clone());
        for child in node.children.values_mut() {
            Self::propagate_prefix(child, package_name.clone());
        }
    }

    /// Insert a package files; return the new paths that conflict
    /// with what was already in the trie before this call.
    ///
    /// 1. **File -> File** at `p`: return `p`.
    /// 2. **Directory -> File** at `p`: return just `p`.
    /// 3. **File -> Directory** under some existing file `f`: return the new file’s `p`.
    /// 4. **Directory -> Directory**: no conflict.
    pub fn insert_package<P: AsRef<Path>>(
        &mut self,
        package: PackageName,
        paths: &[P],
    ) -> Vec<PathBuf> {
        // Record insertion order for future reprioritize.
        self.packages.insert(package.clone());

        let mut conflicts = BTreeSet::default();
        // Which of these paths were *directories* on the old trie?
        let mut dir_inserts = Vec::new();

        // 1) detect conflicts against the existing trie
        for p in paths {
            let p = p.as_ref();

            // Single trie traversal for all conflict checks
            let mut current_node = &self.root;
            let mut found_node = None;
            let mut has_conflict = false;

            // Single-pass approach: use peekable iterator to detect last component
            let mut components = p.components().peekable();

            // Navigate to the target node, checking for prefix conflicts along the way
            while let Some(component) = components.next() {
                let comp = component.as_os_str();
                let is_last = components.peek().is_none();

                match current_node.children.get(comp) {
                    Some(node) => {
                        // Check for File vs Directory conflict (file exists at prefix)
                        if !is_last && !node.terminals.is_empty() {
                            conflicts.insert(p.to_path_buf());
                            has_conflict = true;
                            break;
                        }

                        current_node = node;

                        // If this is the final component, save the node
                        if is_last {
                            found_node = Some(node);
                        }
                    }
                    None => break, // Path doesn't exist in trie
                }
            }

            // Check conflicts at the target node (if we found it and no prefix conflict)
            if !has_conflict {
                if let Some(n) = found_node {
                    // File vs File conflict
                    if !n.terminals.is_empty() {
                        conflicts.insert(p.to_path_buf());
                        continue;
                    }
                    // Directory vs File conflict
                    if !n.children.is_empty() {
                        let pbuf = p.to_path_buf();
                        conflicts.insert(pbuf.clone());
                        dir_inserts.push(pbuf);
                    }
                }
            }
        }

        // 2) actually insert all files
        for p in paths {
            self.insert_file_owned(p, package.clone());
        }

        // 3) propagate directory inserts into descendants
        for pbuf in dir_inserts {
            if let Some(n) = self.get_node_mut(&pbuf) {
                Self::propagate_prefix(n, package.clone());
            }
        }

        conflicts.into_iter().collect()
    }

    /// Unregister all paths belonging to `package`, then prune empty
    /// branches.
    ///
    /// Returns a change vectors.
    pub fn unregister_package<N: Into<PackageName>>(&mut self, package: N) -> Changes {
        fn collect_next_candidate_paths(
            package: PackageName,
            candidates: &indexmap::set::Slice<PackageName>,
            n: &PathTrieNode,
            to_add: &mut Vec<(PathBuf, PackageName)>,
            path: &mut PathBuf,
            under_removed: bool,
        ) {
            // Determine if this node is part of the removed package's coverage.
            let removed_here = n.terminals.contains(&package);
            let prefix_here = n.prefixes.contains(&package);
            // Active if we're under a removed file or at a prefix of a removed package.
            let active = under_removed || removed_here || prefix_here;
            if !active {
                return;
            }

            // If any candidate has a file here, take the highest-priority one.
            for candidate in candidates {
                if n.terminals.contains(candidate) {
                    to_add.push((path.clone(), candidate.clone()));
                    break;
                }
            }

            // Prepare flag for deeper recursion: once under a removed file, stay under.
            let next_under = under_removed || removed_here;
            // Recurse into all child nodes under this covered subtree.
            for (comp, child) in &n.children {
                path.push(comp);
                collect_next_candidate_paths(package.clone(), candidates, child, to_add, path, next_under);
                path.pop();
            }
        }

        fn prune(n: &mut PathTrieNode) -> bool {
            n.children.retain(|_, c| !prune(c));
            n.prefixes.is_empty() && n.terminals.is_empty() && n.children.is_empty()
        }

        fn rm(n: &mut PathTrieNode, pkg: PackageName) {
            n.prefixes.remove(&pkg);
            n.terminals.remove(&pkg);
            for child in n.children.values_mut() {
                rm(child, pkg.clone());
            }
        }

        let package = package.into();

        if !self.packages.contains(&package) {
            return Default::default();
        }

        let mut from_clobbers = vec![];
        let to_clobbers = vec![];

        if let Some(candidates) = self
            .packages
            .get_index_of(&package)
            .and_then(|idx| self.packages.get_range(idx + 1..))
        {
            collect_next_candidate_paths(
                package.clone(),
                candidates,
                &self.root,
                &mut from_clobbers,
                &mut PathBuf::new(),
                false,
            );
        }

        self.packages.shift_remove(&package);

        rm(&mut self.root, package);

        self.root.children.retain(|_, c| !prune(c));

        (to_clobbers, from_clobbers)
    }

    /// Recompute active files based on old (insertion) vs new priority.
    pub fn reprioritize_packages(&mut self, new_order: Vec<PackageName>) -> Changes {
        fn rank<'a>(
            order: impl IntoIterator<Item = &'a PackageName>,
        ) -> HashMap<&'a PackageName, usize> {
            order.into_iter().enumerate().map(|(i, v)| (v, i)).collect()
        }

        fn collect_removed_subtree(
            n: &PathTrieNode,
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
            n: &PathTrieNode,
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
            n: &PathTrieNode,
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
            let is_reordering = 'reorder: {
                if self.packages.len() != new_order.len() {
                    break 'reorder false;
                }
                let self_pkg_set: HashSet<&PackageName> = self.packages.iter().collect();
                let new_pkg_set: HashSet<&PackageName> = new_order.iter().collect();
                self_pkg_set == new_pkg_set
            };

            assert!(
                is_reordering,
                "Expected just reordering, got something else.
Old:
{:#?}
New:
{:#?}
",
                self.packages
                    .iter()
                    .cloned()
                    .sorted()
                    .collect::<Vec<PackageName>>(),
                new_order
                    .iter()
                    .cloned()
                    .sorted()
                    .collect::<Vec<PackageName>>()
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
        self.packages.extend(new_order.into_iter().rev());

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
            let dst = clobbers_dir.join::<&Path>(pkg.as_ref()).join(p);
            if src.exists() && !dst.exists() {
                mv(src, dst)?;
            }
        }

        for (p, pkg) in from_clobbers {
            let src = clobbers_dir.join::<&Path>(pkg.as_ref()).join(p);
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

        for comp in path.as_ref().components().map(Component::as_os_str) {
            cur = cur.children.get(comp)?;
        }

        Some(&cur.prefixes)
    }

    /// Who owns exactly this file?
    pub fn packages_for_exact<P: AsRef<Path>>(&self, path: P) -> Option<&HashSet<PackageName>> {
        Self::get_node(&self.root, path.as_ref()).map(|n| &n.terminals)
    }

    /// List global file-vs-file conflicts (>1 owners).
    pub fn find_conflicts(&self) -> Vec<(PathBuf, Vec<PackageName>)> {
        fn dfs(n: &PathTrieNode, cur: &mut PathBuf, out: &mut Vec<(PathBuf, Vec<PackageName>)>) {
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
    fn get_node<'a>(root: &'a PathTrieNode, path: &Path) -> Option<&'a PathTrieNode> {
        let mut cur = root;
        for comp in path.components().map(Component::as_os_str) {
            cur = cur.children.get(comp)?;
        }
        Some(cur)
    }

    /// Collect all paths where multiple packages wrote the same file,
    /// returning a map from each path to its final owner and overridden packages.
    pub fn collect_clobbered_paths(&self) -> HashMap<PathBuf, ClobberedPath> {
        fn dfs(
            node: &PathTrieNode,
            path: &mut PathBuf,
            packages: &IndexSet<PackageName>,
            results: &mut HashMap<PathBuf, ClobberedPath>,
        ) {
            if !node.terminals.is_empty() {
                // Determine the winning package by insertion priority
                if let Some(winner) = packages
                    .iter()
                    .find(|pkg| node.terminals.contains(pkg))
                {
                    // Collect all other packages that wrote to this path
                    let others: Vec<PackageName> = node
                        .terminals
                        .iter()
                        .filter(|&p| (p.as_ref() as &Path) != (winner.as_ref() as &Path))
                        .cloned()
                        .collect();
                    if !others.is_empty() {
                        results.insert(
                            path.clone(),
                            ClobberedPath {
                                winner: winner.clone(),
                                losers: others,
                            },
                        );
                    }
                }
            }
            for (comp, child) in &node.children {
                path.push(comp);
                dfs(child, path, packages, results);
                path.pop();
            }
        }

        let mut results = HashMap::default();
        dfs(
            &self.root,
            &mut PathBuf::new(),
            &self.packages,
            &mut results,
        );
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
        let mut resolver = PathResolver::new();
        assert!(resolver
            .insert_package("pkg1".into(), &["foo.txt"])
            .is_empty());
        let conflicts = resolver.insert_package("pkg2".into(), &["foo.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("foo.txt")]);
    }

    #[test]
    fn test_insert_nested_file_vs_file_conflict() {
        let mut resolver = PathResolver::new();
        assert!(resolver
            .insert_package("pkg1".into(), &["foo/bar.txt"])
            .is_empty());
        let conflicts = resolver.insert_package("pkg2".into(), &["foo/bar.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("foo/bar.txt")]);
    }

    #[test]
    fn test_insert_dir_vs_file_conflict() {
        let mut resolver = PathResolver::new();
        assert!(resolver
            .insert_package("pkg1".into(), &["foo/bar.txt", "foo/baz.txt"])
            .is_empty());
        let mut conflicts = resolver.insert_package("pkg2".into(), &["foo"]);
        conflicts.sort();
        assert_eq!(conflicts, vec![PathBuf::from("foo")]);
    }

    #[test]
    fn test_insert_file_vs_dir_conflict() {
        let mut resolver = PathResolver::new();
        assert!(resolver.insert_package("pkg1".into(), &["foo"]).is_empty());
        let conflicts = resolver.insert_package("pkg2".into(), &["foo/bar.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("foo/bar.txt")]);
    }

    #[test]
    fn test_no_conflict_on_sibling() {
        let mut resolver = PathResolver::new();
        assert!(resolver.insert_package("a".into(), &["a/x"]).is_empty());
        assert!(resolver.insert_package("b".into(), &["b/y"]).is_empty());
    }

    #[test]
    fn test_no_conflict_on_dir_sibling() {
        let mut resolver = PathResolver::new();
        assert!(resolver.insert_package("a".into(), &["a/x"]).is_empty());
        assert!(resolver.insert_package("b".into(), &["a/y"]).is_empty());
    }

    #[test]
    fn test_unregister_package() {
        let mut resolver = PathResolver::new();
        let paths = ["foo.txt", "foo/bar.txt"];
        assert!(resolver.insert_package("pkg".into(), &paths).is_empty());
        resolver.unregister_package("pkg");
        assert!(resolver
            .packages_for_exact("foo.txt")
            .is_none_or(HashSet::is_empty));
        assert!(resolver.packages_for_prefix("foo").is_none());
    }

    #[test]
    fn test_reprioritize_noop() {
        let mut resolver = PathResolver::new();
        resolver.insert_package("pkg1".into(), &["a.txt"]);
        resolver.insert_package("pkg2".into(), &["b.txt"]);
        let (removed, added) = resolver.reprioritize_packages(vec!["pkg1".into(), "pkg2".into()]);
        assert!(removed.is_empty());
        assert!(added.is_empty());
    }

    #[test]
    fn test_reprioritize_file_vs_file() {
        let mut resolver = PathResolver::new();
        resolver.insert_package("pkg1".into(), &["foo.txt"]);
        resolver.insert_package("pkg2".into(), &["foo.txt"]);
        let (removed, added) = resolver.reprioritize_packages(vec!["pkg1".into(), "pkg2".into()]);
        assert_eq!(
            removed,
            vec![(PathBuf::from("foo.txt"), "pkg1".into())]
        );
        assert_eq!(added, vec![(PathBuf::from("foo.txt"), "pkg2".into())]);
    }

    #[test]
    fn test_reprioritize_file_vs_dir() {
        let mut resolver = PathResolver::new();
        resolver.insert_package("pkg1".into(), &["foo"]);
        resolver.insert_package("pkg2".into(), &["foo/bar.txt"]);
        let (removed, added) = resolver.reprioritize_packages(vec!["pkg1".into(), "pkg2".into()]);
        assert_eq!(removed, vec![(PathBuf::from("foo"), "pkg1".into())]);
        assert_eq!(
            added,
            vec![(PathBuf::from("foo/bar.txt"), "pkg2".into())]
        );
    }

    #[test]
    fn test_reprioritize_dir_vs_file() {
        let mut resolver = PathResolver::new();
        resolver.insert_package("pkg1".into(), &["foo/bar.txt"]);
        resolver.insert_package("pkg2".into(), &["foo"]);
        let (removed, added) = resolver.reprioritize_packages(vec!["pkg1".into(), "pkg2".into()]);
        assert_eq!(
            removed,
            vec![(PathBuf::from("foo/bar.txt"), "pkg1".into())]
        );
        assert_eq!(added, vec![(PathBuf::from("foo"), "pkg2".into())]);
    }

    #[test]
    fn test_reprioritize_dir_vs_dir() {
        let mut resolver = PathResolver::new();
        resolver.insert_package("pkg1".into(), &["foo/bar1.txt", "foo/bar2.txt"]);
        let mut conflict = resolver.insert_package("pkg2".into(), &["foo/bar2.txt"]);
        conflict.sort();
        assert_eq!(conflict, vec![PathBuf::from("foo/bar2.txt")]);
        let (removed, added) = resolver.reprioritize_packages(vec!["pkg1".into(), "pkg2".into()]);
        // pkg1 was winner and now pkg2 is a winner
        assert_eq!(
            removed,
            vec![(PathBuf::from("foo/bar2.txt"), "pkg1".into())],
        );
        assert_eq!(
            added,
            vec![(PathBuf::from("foo/bar2.txt"), "pkg2".into())]
        );
    }

    #[test]
    fn test_reprioritize_file_vs_dir_vs_dir_with_permuted_insertion_order() {
        let priority_order = vec!["pkg1".into(), "pkg2".into(), "pkg3".into()];

        // 1
        let pkgs: &[(PackageName, &[&str])] = &[
            ("pkg1".into(), &["foo"]),
            ("pkg2".into(), &["foo/bar1.txt", "foo/bar2.txt"]),
            ("pkg3".into(), &["foo/bar2.txt"]),
        ];

        let mut resolver = PathResolver::new();
        for (pkg_name, paths) in pkgs {
            resolver.insert_package(pkg_name.clone(), paths);
        }

        let (removed, mut added) = resolver.reprioritize_packages(priority_order.clone());
        assert_eq!(removed, vec![(PathBuf::from("foo"), "pkg1".into())],);
        added.sort();
        assert_eq!(
            added,
            vec![
                (PathBuf::from("foo/bar1.txt"), "pkg2".into()),
                (PathBuf::from("foo/bar2.txt"), "pkg3".into())
            ]
        );

        // 2
        let pkgs: &[(String, &[&str])] = &[
            ("pkg1".into(), &["foo"]),
            ("pkg3".into(), &["foo/bar2.txt"]),
            ("pkg2".into(), &["foo/bar1.txt", "foo/bar2.txt"]),
        ];

        let mut resolver = PathResolver::new();
        for (pkg_name, paths) in pkgs {
            resolver.insert_package(pkg_name.into(), paths);
        }

        let (removed, mut added) = resolver.reprioritize_packages(priority_order.clone());
        assert_eq!(removed, vec![(PathBuf::from("foo"), "pkg1".into())],);
        added.sort();
        assert_eq!(
            added,
            vec![
                (PathBuf::from("foo/bar1.txt"), "pkg2".into()),
                (PathBuf::from("foo/bar2.txt"), "pkg3".into())
            ]
        );

        // 3
        let pkgs: &[(String, &[&str])] = &[
            ("pkg2".into(), &["foo/bar1.txt", "foo/bar2.txt"]),
            ("pkg1".into(), &["foo"]),
            ("pkg3".into(), &["foo/bar2.txt"]),
        ];

        let mut resolver = PathResolver::new();
        for (pkg_name, paths) in pkgs {
            resolver.insert_package(pkg_name.into(), paths);
        }

        let (removed, added) = resolver.reprioritize_packages(priority_order.clone());
        assert_eq!(
            removed,
            vec![(PathBuf::from("foo/bar2.txt"), "pkg2".into())],
        );
        assert_eq!(
            added,
            vec![(PathBuf::from("foo/bar2.txt"), "pkg3".into())]
        );

        // 4
        let pkgs: &[(String, &[&str])] = &[
            ("pkg2".into(), &["foo/bar1.txt", "foo/bar2.txt"]),
            ("pkg3".into(), &["foo/bar2.txt"]),
            ("pkg1".into(), &["foo"]),
        ];

        let mut resolver = PathResolver::new();
        for (pkg_name, paths) in pkgs {
            resolver.insert_package(pkg_name.into(), paths);
        }

        let (removed, added) = resolver.reprioritize_packages(priority_order.clone());
        assert_eq!(
            removed,
            vec![(PathBuf::from("foo/bar2.txt"), "pkg2".into())],
        );
        assert_eq!(
            added,
            vec![(PathBuf::from("foo/bar2.txt"), "pkg3".into())]
        );

        // 5
        let pkgs: &[(String, &[&str])] = &[
            ("pkg3".into(), &["foo/bar2.txt"]),
            ("pkg1".into(), &["foo"]),
            ("pkg2".into(), &["foo/bar1.txt", "foo/bar2.txt"]),
        ];

        let mut resolver = PathResolver::new();
        for (pkg_name, paths) in pkgs {
            resolver.insert_package(pkg_name.into(), paths);
        }

        let (removed, added) = resolver.reprioritize_packages(priority_order.clone());
        assert!(removed.is_empty());
        assert!(added.is_empty());

        // 6
        let pkgs: &[(String, &[&str])] = &[
            ("pkg3".into(), &["foo/bar2.txt"]),
            ("pkg2".into(), &["foo/bar1.txt", "foo/bar2.txt"]),
            ("pkg1".into(), &["foo"]),
        ];

        let mut resolver = PathResolver::new();
        for (pkg_name, paths) in pkgs {
            resolver.insert_package(pkg_name.into(), paths);
        }

        let (removed, added) = resolver.reprioritize_packages(priority_order.clone());
        assert!(removed.is_empty());
        assert!(added.is_empty());
    }

    #[test]
    fn test_tags_queries() {
        let mut resolver = PathResolver::new();
        resolver.insert_package("pkg".into(), &["d1/f1.txt", "d1/f2.txt", "d2/f3.txt"]);
        let p1 = resolver.packages_for_prefix("d1").unwrap();
        assert_eq!(p1.len(), 1);
        assert!(p1.contains(&"pkg".into()));
        let e = resolver.packages_for_exact("d1/f2.txt").unwrap();
        assert_eq!(e.len(), 1);
        assert!(e.contains(&"pkg".into()));
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

        let mut resolver = PathResolver::new();
        resolver.insert_package("pkg1".into(), &["foo.txt"]);
        resolver.insert_package("pkg2".into(), &["foo.txt"]);
        let (to_clobbers, from_clobbers) =
            resolver.reprioritize_packages(vec!["pkg1".into(), "pkg2".into()]);

        PathResolver::sync_clobbers(target_prefix, &clobbers, &to_clobbers, &from_clobbers)
            .unwrap();

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
        let mut resolver = PathResolver::new();
        assert!(resolver.insert_package("pkg1".into(), &["foo"]).is_empty());
        resolver.insert_package("pkg2".into(), &["foo/bar.txt"]);
        let moves = resolver.unregister_package("pkg1");
        assert_eq!(
            moves,
            (vec![], vec![(PathBuf::from("foo/bar.txt"), "pkg2".into())])
        );
    }

    #[test]
    fn test_collect_clobbered_no_conflict() {
        let mut resolver = PathResolver::new();
        assert!(resolver
            .insert_package("pkg1".into(), &["a.txt"])
            .is_empty());

        let clobbered = resolver.collect_clobbered_paths();
        assert!(clobbered.is_empty());
    }

    #[test]
    fn test_collect_clobbered_simple_conflict() {
        let mut resolver = PathResolver::new();
        assert!(resolver
            .insert_package("pkg1".into(), &["file.txt"])
            .is_empty());
        let conflicts = resolver.insert_package("pkg2".into(), &["file.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("file.txt")]);

        let clobbered = resolver.collect_clobbered_paths();
        assert_eq!(clobbered.len(), 1);
        let path = PathBuf::from("file.txt");
        let entry = clobbered.get(&path).expect("file.txt should be present");
        assert_eq!(entry.winner, "pkg1".into());
        assert_eq!(entry.losers, vec!["pkg2".into()]);
    }

    #[test]
    fn test_collect_clobbered_multiple_conflicts() {
        let mut resolver = PathResolver::new();
        assert!(resolver
            .insert_package("pkg1".into(), &["dup.txt"])
            .is_empty());
        let conflicts = resolver.insert_package("pkg2".into(), &["dup.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("dup.txt")]);
        let conflicts = resolver.insert_package("pkg3".into(), &["dup.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("dup.txt")]);

        let clobbered = resolver.collect_clobbered_paths();
        assert_eq!(clobbered.len(), 1);
        let path = PathBuf::from("dup.txt");
        let entry = clobbered.get(&path).expect("dup.txt should be present");
        assert_eq!(entry.winner, "pkg1".into());
        let mut others = entry.losers.clone();
        others.sort();
        assert_eq!(others, vec!["pkg2".into(), "pkg3".into()]);
    }

    #[test]
    fn test_collect_clobbered_multiple_files() {
        let mut resolver = PathResolver::new();
        assert!(resolver
            .insert_package("pkg1".into(), &["a.txt", "b.txt"])
            .is_empty());
        let conflicts = resolver.insert_package("pkg2".into(), &["a.txt"]);
        assert_eq!(conflicts, vec![PathBuf::from("a.txt")]);

        let clobbered = resolver.collect_clobbered_paths();
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
        let mut resolver = PathResolver::new();
        assert!(resolver.insert_package("pkg1".into(), &["dir"]).is_empty());
        let conflicts = resolver.insert_package("pkg2".into(), &["dir"]);
        assert_eq!(conflicts, vec![PathBuf::from("dir")]);

        let clobbered = resolver.collect_clobbered_paths();
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

#[cfg(test)]
mod props {
    use super::*;

    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use proptest::prelude::*;
    use proptest::sample::subsequence;
    use proptest::string::string_regex;

    /// Filesystem path trie.
    #[derive(Clone, Debug)]
    enum Node {
        File,
        Dir(BTreeMap<String, Node>),
    }

    fn collect_paths(node: &Node, cur: &Path, out: &mut Vec<PathBuf>) {
        match node {
            Node::File => out.push(cur.to_path_buf()),
            Node::Dir(children) => {
                for (seg, child) in children {
                    let mut next = cur.to_path_buf();
                    next.push(seg);
                    collect_paths(child, &next, out);
                }
            }
        }
    }

    // TODO: Add trie with non-empty paths for more property-based tests.
    /// Strategy to build random path trie.
    fn path_trie() -> impl Strategy<Value = Node> {
        // atomic leaf
        let leaf = Just(Node::File).boxed();
        // directory nodes built from smaller tries
        let dir = |inner: BoxedStrategy<Node>| {
            prop::collection::btree_map(
                // unique segment names
                string_regex("[a-z]{1,1}").unwrap(),
                inner,
                1..=5,
            )
            .prop_map(Node::Dir)
            .boxed()
        };

        leaf.prop_recursive(5, 64, 5, dir)
    }

    /// Strategy yielding a vector of `(PackageName, Vec<PathBuf>)`,
    fn arb_package_set() -> impl Strategy<Value = Vec<(String, Vec<PathBuf>)>> {
        // 1) pick 1–5 distinct package names
        let names = subsequence(&["pkg1", "pkg2", "pkg3", "pkg4", "pkg5", "pkg6"], 1..=5)
            .prop_map(|v| v.into_iter().map(str::to_string).collect::<Vec<_>>());

        names.prop_flat_map(move |pkgs| {
            // draw one independent non_empty_trie per package
            let tries = prop::collection::vec(path_trie(), pkgs.len());

            (Just(pkgs.clone()), tries).prop_map(move |(pkgs, trees)| {
                pkgs.into_iter()
                    .zip(trees)
                    .map(|(pkg, tree)| {
                        let mut paths = Vec::new();
                        collect_paths(&tree, &PathBuf::new(), &mut paths);
                        (pkg, paths)
                    })
                    .collect()
            })
        })
    }

    /// Strategy yielding a `PathResolver` with some packages already inserted,
    /// together with the `Vec<PackageName>` in the order they were inserted.
    fn arb_resolver() -> impl Strategy<Value = (PathResolver, Vec<PackageName>)> {
        arb_package_set().prop_map(|pkg_set| {
            let mut resolver = PathResolver::new();
            // keep track of the order in which we insert packages
            let mut initial_order = Vec::with_capacity(pkg_set.len());

            for (package, paths) in pkg_set {
                // Insert each package (ignoring any spurious conflicts)
                let pkg: PackageName = package.into();
                let _ = resolver.insert_package(pkg.clone(), &paths);
                initial_order.push(pkg);
            }

            (resolver, initial_order)
        })
    }

    proptest! {
        #[test]
        fn identity_no_changes((mut resolver, packages) in arb_resolver()) {
            let (removed, added) = resolver.reprioritize_packages(packages.into_iter().rev().collect());
            prop_assert!(removed.is_empty());
            prop_assert!(added.is_empty());
        }

        #[test]
        fn reprioritize_updates_order((mut resolver, packages) in arb_resolver()) {
            let new_order: Vec<_> = packages.iter().rev().cloned().collect();
            let (_removed, _added) = resolver.reprioritize_packages(new_order.clone());
            let current_order: Vec<_> = resolver.packages.iter().rev().cloned().collect();
            prop_assert_eq!(current_order, new_order);
        }

        #[test]
        fn idempotent_after_reprioritize((mut resolver, packages) in arb_resolver()) {
            let new_order: Vec<_> = packages.clone();
            let _first = resolver.reprioritize_packages(new_order.clone());
            let (removed2, added2) = resolver.reprioritize_packages(new_order);
            prop_assert!(removed2.is_empty());
            prop_assert!(added2.is_empty());
        }
    }
}
