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

#[derive(Debug, Default)]
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

        let (first, rest) = components.split_first().unwrap();
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
}

#[derive(Debug, Default)]
pub struct PackageResolver {
    root: TreeNode,
    conflicts: Vec<Conflict>,
    final_layout: HashMap<PathBuf, Entry>,
}

impl PackageResolver {
    pub fn new() -> Self {
        Self::default()
    }

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

    pub fn remove_package(&mut self, package_name: &str) {
        fn remove_in_tree(node: &mut TreeNode, package_name: &str) {
            node.entries = node
                .entries
                .iter()
                .filter(|e| e.package_name != package_name)
                .cloned()
                .collect();
            for (_key, tree) in node.children.iter_mut() {
                remove_in_tree(tree, package_name)
            }
        }
        remove_in_tree(&mut self.root, package_name)
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

    pub fn resolve_conflicts(&mut self) {
        self.conflicts.clear();
        self.root.resolve_conflicts(false, &mut self.conflicts);

        self.final_layout.clear();
        self.root.collect_winners(&mut self.final_layout);
    }

    pub fn get_final_layout(&self) -> &HashMap<PathBuf, Entry> {
        &self.final_layout
    }

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
        let mut resolver = PackageResolver::new();
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
        let mut resolver = PackageResolver::new();
        resolver.add_package("pkg1", 0, &[(Path::new("config.txt"), EntryType::File)]);
        resolver.add_package("pkg2", 1, &[(Path::new("config.txt"), EntryType::File)]);
        resolver.resolve_conflicts();

        let conflicts = resolver.get_conflicts();
        assert_eq!(conflicts.len(), 1); // Should have one conflict
        assert_eq!(conflicts[0].conflict_type, ConflictType::DirectConflict);
    }

    #[test]
    fn test_file_vs_directory_conflicts() {
        let mut resolver = PackageResolver::new();
        resolver.add_package("pkg1", 0, &[(Path::new("config"), EntryType::Directory)]);
        resolver.add_package("pkg2", 1, &[(Path::new("config"), EntryType::File)]);
        resolver.resolve_conflicts();

        let conflicts = resolver.get_conflicts();
        assert_eq!(conflicts.len(), 1); // Should have one conflict
        assert_eq!(conflicts[0].winner.entry_type, EntryType::File); // File wins
    }

    #[test]
    fn test_file_blocks_directory_tree() {
        let mut resolver = PackageResolver::new();

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
        let mut resolver = PackageResolver::new();

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

        resolver.resolve_conflicts();

        let layout = resolver.get_final_layout();
        assert!(layout.contains_key(&PathBuf::from("a/b")));
        assert!(!layout.contains_key(&PathBuf::from("a/b/c/d/file.txt")));
        assert!(!layout.contains_key(&PathBuf::from("a/b/other.txt")));
    }

    #[test]
    fn test_no_blocking_with_directories() {
        let mut resolver = PackageResolver::new();

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
        let mut resolver = PackageResolver::new();
        resolver.add_package("pkg1", 0, &[(Path::new("config.txt"), EntryType::File)]);
        resolver.add_package("pkg2", 1, &[(Path::new("config.txt"), EntryType::File)]);
        resolver.remove_package("pkg2");
        resolver.resolve_conflicts();

        let conflicts = resolver.get_conflicts();
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_file_vs_directory_conflicts_remove_package() {
        let mut resolver = PackageResolver::new();
        resolver.add_package("pkg1", 0, &[(Path::new("config"), EntryType::Directory)]);
        resolver.add_package("pkg2", 1, &[(Path::new("config"), EntryType::File)]);
        resolver.remove_package("pkg2");
        resolver.resolve_conflicts();

        let conflicts = resolver.get_conflicts();
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_file_blocks_directory_tree_remove_package() {
        let mut resolver = PackageResolver::new();

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
        let mut resolver = PackageResolver::new();

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
            let mut resolver = PackageResolver::new();
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
}
