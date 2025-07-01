use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum EntryType {
    File,
    Directory,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub entry_type: EntryType,
    pub package_index: usize,
    pub package_name: String,
}

#[derive(Debug, Clone)]
pub struct Conflict {
    pub path: PathBuf,
    pub winner: Entry,
    pub losers: Vec<Entry>,
    pub conflict_type: ConflictType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConflictType {
    DirectConflict,
    BlockedByAncestor,
}

#[derive(Debug, Default, Clone)]
struct TreeNode {
    entries: Vec<Entry>,
    children: HashMap<String, TreeNode>,
    winner: Option<Entry>,
    is_blocked: bool,
}

impl TreeNode {
    fn insert_entry(&mut self, components: &[&str], entry: Entry) {
        if components.is_empty() {
            self.entries.push(entry);
            return;
        }

        let (&first, rest) = components.split_first().unwrap();
        self.children
            .entry(first.to_string())
            .or_default()
            .insert_entry(rest, entry);
    }

    fn resolve_conflicts(&mut self, parent_blocked: bool, conflicts: &mut Vec<Conflict>) -> bool {
        if parent_blocked {
            self.is_blocked = true;
            self.mark_blocked_conflicts(conflicts);
            return true;
        }

        // Resolve conflicts - only create conflicts for actual incompatible entries
        if self.entries.len() > 1 {
            self.entries.sort_by_key(|e| e.package_index);

            // Check if we have any files (which conflict with everything)
            let has_files = self.entries.iter().any(|e| e.entry_type == EntryType::File);

            if has_files {
                // Files conflict with everything - last package wins
                let winner = self.entries.last().unwrap().clone();
                let losers = self.entries[..self.entries.len() - 1].to_vec();

                conflicts.push(Conflict {
                    path: winner.path.clone(),
                    winner: winner.clone(),
                    losers,
                    conflict_type: ConflictType::DirectConflict,
                });

                self.winner = Some(winner);
            } else {
                // All directories - no conflict, just pick any one (they're equivalent)
                self.winner = Some(self.entries.last().unwrap().clone());
            }
        } else if let Some(entry) = self.entries.first() {
            self.winner = Some(entry.clone());
        }

        let blocks_children = self
            .winner
            .as_ref()
            .map_or(false, |w| w.entry_type == EntryType::File);

        // Recursively resolve children
        for child in self.children.values_mut() {
            child.resolve_conflicts(blocks_children, conflicts);
        }

        blocks_children
    }

    fn mark_blocked_conflicts(&mut self, conflicts: &mut Vec<Conflict>) {
        for entry in &self.entries {
            conflicts.push(Conflict {
                path: entry.path.clone(),
                winner: entry.clone(),
                losers: vec![entry.clone()],
                conflict_type: ConflictType::BlockedByAncestor,
            });
        }

        for child in self.children.values_mut() {
            child.is_blocked = true;
            child.mark_blocked_conflicts(conflicts);
        }
    }

    fn collect_winners(&self, layout: &mut HashMap<PathBuf, Entry>) {
        if !self.is_blocked {
            if let Some(winner) = &self.winner {
                layout.insert(winner.path.clone(), winner.clone());
            }
        }

        for child in self.children.values() {
            child.collect_winners(layout);
        }
    }

    fn priorities_aux(&self, priorities: &mut HashMap<String, usize>) {
        for entry in &self.entries {
            priorities
                .entry(entry.package_name.as_str().to_owned())
                .or_insert(entry.package_index);
        }

        for child in self.children.values() {
            child.priorities_aux(priorities);
        }
    }

    fn priorities(&self) -> HashMap<String, usize> {
        let mut priorities = HashMap::new();
        self.priorities_aux(&mut priorities);
        priorities
    }

    fn reprioritize(&mut self, priorities: &HashMap<String, usize>) {
        for entry in &mut self.entries {
            if let Some(&new_priority) = priorities.get(entry.package_name.as_str()) {
                entry.package_index = new_priority;
            }
        }

        for child in self.children.values_mut() {
            child.reprioritize(priorities);
        }
    }

    fn remove_package(&mut self, package_name: &str) -> bool {
        self.entries.retain(|e| e.package_name != package_name);
        self.children
            .retain(|_, child| child.remove_package(package_name));

        // Now we reset conflict resolution state completely, but it
        // should be possible to keep parts of it, excluding given
        // `package_name`.
        self.winner = None;
        self.is_blocked = false;

        !self.entries.is_empty() || !self.children.is_empty()
    }
}

#[derive(Debug, Default, Clone)]
pub struct PackagePathResolver {
    root: TreeNode,
    conflicts: Vec<Conflict>,
    final_layout: HashMap<PathBuf, Entry>,
}

impl PackagePathResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register package for conflict resolution.
    ///
    /// Package index denotes priority of a package.
    pub fn add_package(
        &mut self,
        package_name: &str,
        package_index: usize,
        paths: &[(&Path, EntryType)],
    ) {
        let mut entries_to_add = Vec::new();

        for (path_str, entry_type) in paths {
            let path = PathBuf::from(path_str);

            // Create implicit directories for files
            if *entry_type == EntryType::File {
                self.collect_implicit_directories(
                    &path,
                    package_name,
                    package_index,
                    &mut entries_to_add,
                );
            }

            entries_to_add.push(Entry {
                path: path.clone(),
                entry_type: entry_type.clone(),
                package_index,
                package_name: package_name.to_string(),
            });
        }

        // Insert all entries
        for entry in entries_to_add {
            let components: Vec<&str> = entry
                .path
                .components()
                .map(|c| c.as_os_str().to_str().unwrap())
                .collect();
            self.root.insert_entry(&components, entry.clone());
        }
    }

    /// Remove all paths of the packages in the consideration.
    ///
    /// If there was some conflict or layout calculated we recalculate
    /// it after removal of a package.
    pub fn remove_package(&mut self, package_name: &str) {
        let previously_resolved = !self.conflicts.is_empty() || !self.final_layout.is_empty();

        let keep_root = self.root.remove_package(package_name);

        // TODO: Just remove given package from consideration in conflicts and restructure .
        self.conflicts = vec![];
        self.final_layout = HashMap::new();

        if !keep_root {
            self.root = TreeNode::default();
            return;
        }

        if previously_resolved {
            self.resolve_conflicts();
        }
    }

    /// Returns unsorted iterator where each item is pair of package
    /// name and it's priority index.
    pub fn priorities(&self) -> HashMap<String, usize> {
        self.root.priorities()
    }

    /// Changes indices (priorities) of given packages by their names.
    pub fn reprioritize(&mut self, priorities: &HashMap<String, usize>) {
        self.root.reprioritize(priorities);
    }

    fn collect_implicit_directories(
        &self,
        file_path: &Path,
        package_name: &str,
        package_index: usize,
        entries: &mut Vec<Entry>,
    ) {
        let mut current = file_path.parent();
        let mut dirs_to_create = Vec::new();

        while let Some(parent) = current {
            if parent == Path::new("") {
                break;
            }
            dirs_to_create.push(parent);
            current = parent.parent();
        }

        // Create directories from root to leaf
        for &dir_path in dirs_to_create.iter().rev() {
            if !entries
                .iter()
                .any(|e| e.path == dir_path && e.package_name == package_name)
            {
                entries.push(Entry {
                    path: dir_path.to_path_buf(),
                    entry_type: EntryType::Directory,
                    package_index,
                    package_name: package_name.to_string(),
                });
            }
        }
    }

    /// Run conflict resolution algorithm.
    pub fn resolve_conflicts(&mut self) {
        self.conflicts.clear();
        self.root.resolve_conflicts(false, &mut self.conflicts);

        self.final_layout.clear();
        self.root.collect_winners(&mut self.final_layout);
    }

    /// Return final layout of a directory.
    pub fn get_final_layout(&self) -> &HashMap<PathBuf, Entry> {
        &self.final_layout
    }

    /// Resolve conflicts must be called in order to receive non-empty slice.
    pub fn get_conflicts(&self) -> &[Conflict] {
        &self.conflicts
    }

    pub fn print_tree(&self) {
        self.print_node(&self.root, "", 0);
    }

    fn print_node(&self, node: &TreeNode, path: &str, depth: usize) {
        let indent = "  ".repeat(depth);
        let status = match (node.is_blocked, node.winner.is_some()) {
            (true, _) => " [BLOCKED]",
            (false, true) => " [WINNER]",
            _ => "",
        };

        if depth > 0 {
            println!("{}{}{}", indent, path, status);
        }

        for (name, child) in &node.children {
            let child_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", path, name)
            };
            self.print_node(child, &child_path, depth + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_directories_dont_conflict() {
        let mut resolver = PackagePathResolver::new();
        resolver.add_package("pkg1", 0, &[(Path::new("share"), EntryType::Directory)]);
        resolver.add_package("pkg2", 1, &[(Path::new("share"), EntryType::Directory)]);
        resolver.resolve_conflicts();

        let conflicts = resolver.get_conflicts();
        assert_eq!(conflicts.len(), 0); // No conflicts for directory vs directory

        let layout = resolver.get_final_layout();
        assert!(layout.contains_key(&PathBuf::from("share")));
    }

    #[test]
    fn test_file_vs_file_conflicts() {
        let mut resolver = PackagePathResolver::new();
        resolver.add_package("pkg1", 0, &[(Path::new("config.txt"), EntryType::File)]);
        resolver.add_package("pkg2", 1, &[(Path::new("config.txt"), EntryType::File)]);
        resolver.resolve_conflicts();

        let conflicts = resolver.get_conflicts();
        assert_eq!(conflicts.len(), 1); // Should have one conflict
        assert_eq!(conflicts[0].conflict_type, ConflictType::DirectConflict);
    }

    #[test]
    fn test_file_vs_directory_conflicts() {
        let mut resolver = PackagePathResolver::new();
        resolver.add_package("pkg1", 0, &[(Path::new("config"), EntryType::Directory)]);
        resolver.add_package("pkg2", 1, &[(Path::new("config"), EntryType::File)]);
        resolver.resolve_conflicts();

        let conflicts = resolver.get_conflicts();
        assert_eq!(conflicts.len(), 1); // Should have one conflict
        assert_eq!(conflicts[0].winner.entry_type, EntryType::File); // File wins
    }

    #[test]
    fn test_file_blocks_directory_tree() {
        let mut resolver = PackagePathResolver::new();

        resolver.add_package(
            "pkg1",
            0,
            &[
                (Path::new("config/"), EntryType::Directory),
                (Path::new("config/app/"), EntryType::Directory),
                (Path::new("config/app/main.conf"), EntryType::File),
                (Path::new("config/db.conf"), EntryType::File),
            ],
        );

        resolver.add_package("pkg2", 1, &[(Path::new("config"), EntryType::File)]);

        resolver.resolve_conflicts();

        let conflicts = resolver.get_conflicts();

        // Should have one direct conflict and multiple blocked entries
        let direct_conflicts: Vec<_> = conflicts
            .iter()
            .filter(|c| c.conflict_type == ConflictType::DirectConflict)
            .collect();
        assert_eq!(direct_conflicts.len(), 1);

        let blocked_conflicts: Vec<_> = conflicts
            .iter()
            .filter(|c| c.conflict_type == ConflictType::BlockedByAncestor)
            .collect();
        assert!(blocked_conflicts.len() >= 2); // app/, main.conf, db.conf blocked

        let layout = resolver.get_final_layout();
        assert_eq!(layout.len(), 1);
        assert!(layout.contains_key(&PathBuf::from("config")));
    }

    #[test]
    fn test_deep_blocking() {
        let mut resolver = PackagePathResolver::new();

        resolver.add_package(
            "pkg1",
            1,
            &[
                (Path::new("a/b/c/d/file.txt"), EntryType::File),
                (Path::new("a/b/other.txt"), EntryType::File),
            ],
        );

        resolver.add_package(
            "pkg2",
            0,
            &[
                (Path::new("a/b"), EntryType::File), // Blocks everything under a/b/
            ],
        );

        resolver.resolve_conflicts();

        let layout = resolver.get_final_layout();
        assert!(layout.contains_key(&PathBuf::from("a/b")));
        assert!(!layout.contains_key(&PathBuf::from("a/b/c/d/file.txt")));
        assert!(!layout.contains_key(&PathBuf::from("a/b/other.txt")));
    }

    #[test]
    fn test_no_blocking_with_directories() {
        let mut resolver = PackagePathResolver::new();

        resolver.add_package("pkg1", 0, &[(Path::new("share/"), EntryType::Directory)]);

        resolver.add_package(
            "pkg2",
            1,
            &[
                (Path::new("share/docs/"), EntryType::Directory),
                (Path::new("share/docs/readme.txt"), EntryType::File),
            ],
        );

        resolver.resolve_conflicts();

        let layout = resolver.get_final_layout();
        assert_eq!(layout.len(), 3); // All entries should be present
        assert!(layout.contains_key(&PathBuf::from("share/")));
        assert!(layout.contains_key(&PathBuf::from("share/docs/")));
        assert!(layout.contains_key(&PathBuf::from("share/docs/readme.txt")));
    }

    // ================================================================
    #[test]
    fn test_file_vs_file_conflicts_remove_package() {
        let mut resolver = PackagePathResolver::new();
        resolver.add_package("pkg1", 0, &[(Path::new("config.txt"), EntryType::File)]);
        resolver.add_package("pkg2", 1, &[(Path::new("config.txt"), EntryType::File)]);
        resolver.remove_package("pkg2");
        resolver.resolve_conflicts();

        let conflicts = resolver.get_conflicts();
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_file_vs_directory_conflicts_remove_package() {
        let mut resolver = PackagePathResolver::new();
        resolver.add_package("pkg1", 0, &[(Path::new("config"), EntryType::Directory)]);
        resolver.add_package("pkg2", 1, &[(Path::new("config"), EntryType::File)]);
        resolver.remove_package("pkg2");
        resolver.resolve_conflicts();

        let conflicts = resolver.get_conflicts();
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_file_blocks_directory_tree_remove_package() {
        let mut resolver = PackagePathResolver::new();

        resolver.add_package(
            "pkg1",
            0,
            &[
                (Path::new("config/"), EntryType::Directory),
                (Path::new("config/app/"), EntryType::Directory),
                (Path::new("config/app/main.conf"), EntryType::File),
                (Path::new("config/db.conf"), EntryType::File),
            ],
        );

        resolver.add_package("pkg2", 1, &[(Path::new("config"), EntryType::File)]);

        resolver.remove_package("pkg1");

        resolver.resolve_conflicts();

        let conflicts = resolver.get_conflicts();

        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_deep_blocking_remove_package() {
        let mut resolver = PackagePathResolver::new();

        resolver.add_package(
            "pkg1",
            0,
            &[
                (Path::new("a/b/c/d/file.txt"), EntryType::File),
                (Path::new("a/b/other.txt"), EntryType::File),
            ],
        );

        resolver.add_package(
            "pkg2",
            1,
            &[
                (Path::new("a/b"), EntryType::File), // Blocks everything under a/b/
            ],
        );

        resolver.add_package(
            "pkg3",
            2,
            &[
                (Path::new("a/b"), EntryType::File), // Blocks everything under a/b/
            ],
        );

        resolver.remove_package("pkg2");
        resolver.remove_package("pkg3");

        resolver.resolve_conflicts();

        let layout = resolver.get_final_layout();
        assert!(layout.contains_key(&PathBuf::from("a/b/c/d/file.txt")));
        assert!(layout.contains_key(&PathBuf::from("a/b/other.txt")));
    }

    proptest! {
        #[test]
        fn proptest_remove_package_eliminates_all_entries(
            // Generate between 1 and 20 arbitrary paths,
            // each as a non‐empty Vec of 1–3 lowercase segments "a".."z",
            // and a random EntryType (File or Directory).
            entries in prop::collection::vec(
                (
                    prop::collection::vec("[a-z]{1,5}", 1..4),
                    prop_oneof![Just(EntryType::File), Just(EntryType::Directory)]
                ),
                1..20
            )
        ) {
            // Build the resolver and add all these entries under "pkg"
            let mut resolver = PackagePathResolver::new();
            let pkg_paths: Vec<(PathBuf, EntryType)> = entries
                .iter()
                .map(|(segs, et)| {
                    let s = segs.join("/");
                    (PathBuf::from(&s), et.clone())
                })
                .collect();
            let pkg_paths = pkg_paths.iter().map(|p| {
                (p.0.as_path(), p.1.clone())
            }).collect::<Vec<_>>();
            resolver.add_package("pkg", 0, &pkg_paths);

            // Now remove "pkg" and re‐resolve
            resolver.remove_package("pkg");
            resolver.resolve_conflicts();

            // The final layout must be empty
            let layout = resolver.get_final_layout();
            prop_assert!(layout.is_empty());
        }
    }

    fn add_simple_package(
        resolver: &mut PackagePathResolver,
        package_name: &str,
        package_index: usize,
    ) {
        resolver.add_package(
            package_name,
            package_index,
            &[(Path::new("a/"), EntryType::File)],
        );
    }

    // TODO: Write more tests, as we know it won't work right if we have colliding package names.
    // Probably we should move name management into structure itself to avoid that.
    // Also, it won't work if list paths is empty, since in this case package won't be even stored in the TreeNode.
    #[test]
    fn test_priorities() {
        let mut resolver = PackagePathResolver::new();

        let initial_priorities = {
            let mut inner = HashMap::new();
            inner.insert("pkg1".to_string(), 0);
            inner.insert("pkg2".to_string(), 1);
            inner.insert("pkg3".to_string(), 2);
            inner
        };

        for (key, &value) in initial_priorities.iter() {
            add_simple_package(&mut resolver, key, value);
        }

        assert_eq!(initial_priorities, resolver.priorities());

        let new_priorities = {
            let mut inner = HashMap::new();
            inner.insert("pkg1".to_string(), 2);
            inner.insert("pkg2".to_string(), 1);
            inner.insert("pkg3".to_string(), 0);
            inner
        };

        resolver.reprioritize(&new_priorities);

        assert_eq!(new_priorities, resolver.priorities());
    }

    #[test]
    fn playing_around() {
        let mut resolver = PackagePathResolver::new();
        add_simple_package(&mut resolver, "pkg1", 0);
        add_simple_package(&mut resolver, "pkg1", 1);

        dbg!(&resolver);
        dbg!(resolver.get_final_layout());
        resolver.resolve_conflicts();
        dbg!(resolver.get_conflicts());
        dbg!(resolver.get_final_layout());
    }
}
