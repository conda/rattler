//! Convenience wrapper around `ignore` that provides glob matching with intuitive semantics.
//!
//! This module provides [`GlobSet`], which matches files using gitignore-style patterns but with
//! behavioral tweaks that make it more intuitive for typical use cases.
//!
//! # Behavioral Differences from Standard Gitignore
//!
//! ## Pattern Rebasing
//!
//! Patterns containing `..` components (e.g., `../src/*.rs`) are automatically rebased to work
//! from a common ancestor directory. This allows patterns to reference files outside the immediate
//! search root while still using a single efficient directory walker.
//!
//! For example, searching from `/project/subdir` with patterns `["../src/*.rs", "*.txt"]`:
//! - The walker starts from `/project` (the **effective walk root**)
//! - `../src/*.rs` becomes `src/*.rs`
//! - `*.txt` becomes `subdir/*.txt`
//!
//! See the [`walk_root`] module for implementation details.
//!
//! ## Global Exclusions
//!
//! Negated patterns starting with `**/` (e.g., `!**/build.rs`) are treated as global exclusions
//! and skip rebasing. This ensures `!**/build.rs` excludes every `build.rs` file regardless of
//! where the effective root ends up.
//!
//! ## Anchored Literals
//!
//! Plain file names without glob metacharacters (e.g., `config.toml`) are anchored to the search
//! root, matching only at that location rather than anywhere in the tree. This differs from
//! standard gitignore behavior where unanchored patterns match at any depth.
//!
//! Similarly, negated literals (e.g., `!config.toml`) only exclude the file at the root, not
//! copies in subdirectories.

mod walk;
mod walk_root;

use std::path::{Path, PathBuf};

use thiserror::Error;

use walk_root::{WalkRoot, WalkRootsError};

/// A glob set implemented using the `ignore` crate (globset + fast walker).
pub struct GlobSet {
    /// Include patterns (gitignore-style), without leading '!'.
    walk_roots: WalkRoot,
}

/// Errors that can occur when creating or walking a glob set.
#[derive(Error, Debug)]
pub enum GlobSetError {
    /// Failed to build the glob override patterns.
    #[error("failed to build globs")]
    BuildOverrides(#[source] ignore::Error),

    /// An error occurred while walking the directory tree.
    #[error("walk error at {0}")]
    Walk(PathBuf, #[source] ignore::Error),

    /// An error occurred while building the walk roots from glob patterns.
    #[error(transparent)]
    WalkRoots(#[from] WalkRootsError),
}

impl GlobSet {
    /// Create a new [`GlobSet`] from a list of patterns. Leading '!' indicates exclusion.
    ///
    /// # Errors
    /// Returns a [`GlobSetError`] if the glob patterns are invalid.
    pub fn create<'t>(globs: impl IntoIterator<Item = &'t str>) -> Result<GlobSet, GlobSetError> {
        Ok(GlobSet {
            walk_roots: WalkRoot::build(globs)?,
        })
    }

    /// Walks files matching all include/exclude patterns using a single parallel walker.
    /// Returns a flat Vec of results to keep lifetimes simple and predictable.
    pub fn collect_matching(&self, root_dir: &Path) -> Result<Vec<ignore::DirEntry>, GlobSetError> {
        if self.walk_roots.is_empty() {
            return Ok(vec![]);
        }

        let rebased = self.walk_roots.rebase(root_dir)?;
        walk::walk_globs(&rebased.root, &rebased.globs)
    }
}

#[cfg(test)]
mod tests {
    use super::GlobSet;
    use fs_err::{self as fs, File};
    use insta::assert_yaml_snapshot;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn relative_path(path: &Path, root: &Path) -> PathBuf {
        if let Ok(rel) = path.strip_prefix(root) {
            return rel.to_path_buf();
        }
        if let Some(parent) = root.parent() {
            if let Ok(rel) = path.strip_prefix(parent) {
                return std::path::Path::new("..").join(rel);
            }
        }
        path.to_path_buf()
    }

    fn sorted_paths(entries: Vec<ignore::DirEntry>, root: &std::path::Path) -> Vec<String> {
        let mut paths: Vec<_> = entries
            .into_iter()
            .map(|entry| {
                relative_path(entry.path(), root)
                    .display()
                    .to_string()
                    .replace('\\', "/")
            })
            .collect();
        paths.sort();
        paths
    }

    // Test out a normal non-reseated globbing approach
    #[test]
    fn collect_matching_inclusion_exclusion() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();

        File::create(root_path.join("include1.txt")).unwrap();
        File::create(root_path.join("include2.log")).unwrap();
        File::create(root_path.join("exclude.txt")).unwrap();
        fs::create_dir(root_path.join("subdir")).unwrap();
        File::create(root_path.join("subdir/include_subdir.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["**/*.txt", "!exclude.txt"]).unwrap();
        let entries = glob_set.collect_matching(root_path).unwrap();

        let paths = sorted_paths(entries, root_path);
        assert_yaml_snapshot!(paths, @r###"
        - include1.txt
        - subdir/include_subdir.txt
        "###);
    }

    // Check some general globbing support and make sure the correct things do not match
    #[test]
    fn collect_matching_relative_globs() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();
        let search_root = root_path.join("workspace");
        fs::create_dir(&search_root).unwrap();

        fs::create_dir(root_path.join("subdir")).unwrap();
        File::create(root_path.join("subdir/some_inner_source.cpp")).unwrap();
        File::create(root_path.join("subdir/dont-match.txt")).unwrap();
        File::create(search_root.join("match.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["../**/*.cpp", "*.txt"]).unwrap();
        let entries = glob_set.collect_matching(&search_root).unwrap();

        let paths = sorted_paths(entries, &search_root);
        assert_yaml_snapshot!(paths, @r###"
        - "../subdir/some_inner_source.cpp"
        - match.txt
        "###);
    }

    // Check that single matching file glob works with rebasing
    #[test]
    fn collect_matching_file_glob() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        File::create(root_path.join("pixi.toml")).unwrap();

        let glob_set = GlobSet::create(vec!["pixi.toml", "../*.cpp"]).unwrap();
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @"- pixi.toml");
    }

    // Check that global ignores !**/ patterns ignore everything even if the root has been
    // rebased to a parent folder, this is just a convenience assumed to be preferable
    // from a user standpoint
    #[test]
    fn check_global_ignore_ignores() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        File::create(root_path.join("pixi.toml")).unwrap();
        File::create(root_path.join("foo.txt")).unwrap();
        // This would be picked up otherwise
        File::create(temp_dir.path().join("foo.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["pixi.toml", "!**/foo.txt"]).unwrap();
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @"- pixi.toml");
    }

    // Check that we can ignore a subset of file when using the rebasing
    // So we want to match all `.txt` and `*.toml` files except in the root location
    // where want to exclude `foo.txt`
    #[test]
    fn check_subset_ignore() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        File::create(root_path.join("pixi.toml")).unwrap();
        // This should not be picked up
        File::create(root_path.join("foo.txt")).unwrap();
        // But because of the non-global ignore this should be
        File::create(temp_dir.path().join("foo.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["../*.{toml,txt}", "!foo.txt"]).unwrap();
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @r###"
        - "../foo.txt"
        - pixi.toml
        "###);
    }

    #[test]
    fn check_we_ignore_hidden_files() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        let hidden_pixi_folder = root_path.join(".pixi");

        fs::create_dir(&hidden_pixi_folder).unwrap();
        // This should not be picked up
        File::create(hidden_pixi_folder.join("foo_hidden.txt")).unwrap();
        // But because of the non-global ignore this should be
        File::create(root_path.as_path().join("foo_public.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["*.txt"]).unwrap();
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @"- foo_public.txt");
    }

    #[test]
    fn check_hidden_folders_are_included() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        let hidden_pixi_folder = root_path.join(".pixi");

        let hidden_foobar_folder = root_path.join(".foobar");

        let hidden_recursive_folder = root_path
            .join("recursive")
            .join("foobar")
            .join(".deep_hidden");

        fs::create_dir(&hidden_pixi_folder).unwrap();
        fs::create_dir(&hidden_foobar_folder).unwrap();
        fs::create_dir_all(&hidden_recursive_folder).unwrap();

        File::create(hidden_pixi_folder.join("foo_hidden.txt")).unwrap();
        File::create(hidden_foobar_folder.as_path().join("foo_from_foobar.txt")).unwrap();
        File::create(hidden_foobar_folder.as_path().join("build.txt")).unwrap();

        File::create(hidden_recursive_folder.join("foo_from_deep_hidden.txt")).unwrap();

        File::create(root_path.as_path().join("some_text.txt")).unwrap();
        let glob_set = GlobSet::create(vec![
            "**",
            ".foobar/foo_from_foobar.txt",
            "**/.deep_hidden/**",
        ])
        .unwrap();

        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @r#"
        - ".foobar/foo_from_foobar.txt"
        - recursive/foobar/.deep_hidden/foo_from_deep_hidden.txt
        - some_text.txt
        "#);
    }

    #[test]
    fn check_hidden_folder_is_whitelisted_with_star() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        let hidden_pixi_folder = root_path.join(".pixi").join("subdir");

        fs::create_dir_all(&hidden_pixi_folder).unwrap();

        File::create(hidden_pixi_folder.join("foo_hidden.txt")).unwrap();

        File::create(root_path.as_path().join("some_text.txt")).unwrap();
        let glob_set = GlobSet::create(vec![".pixi/subdir/**"]).unwrap();

        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @r###"- ".pixi/subdir/foo_hidden.txt""###);
    }

    #[test]
    fn check_hidden_folders_are_not_included() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        let hidden_pixi_folder = root_path.join(".pixi");

        fs::create_dir(&hidden_pixi_folder).unwrap();

        File::create(hidden_pixi_folder.join("foo_hidden.txt")).unwrap();

        File::create(root_path.as_path().join("some_text.txt")).unwrap();
        // We want to match everything except hidden folders
        let glob_set = GlobSet::create(vec!["**"]).unwrap();

        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @"- some_text.txt");
    }

    /// Because we are using ignore which uses gitignore style parsing of globs we need to do some extra processing
    /// to make this more like unix globs in this case we check this explicitly here
    #[test]
    fn single_file_match() {
        let temp_dir = tempdir().unwrap();
        let workspace = temp_dir.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let subdir = workspace.join("subdir");
        fs::create_dir(&subdir).unwrap();

        File::create(subdir.join("pixi.toml")).unwrap();

        let glob_set = GlobSet::create(vec!["pixi.toml"]).unwrap();
        let entries = glob_set.collect_matching(&workspace).unwrap();

        let paths = sorted_paths(entries, &workspace);
        assert_yaml_snapshot!(paths, @"[]");
    }
}
