use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use anyhow::{Context, Result};
use fs_err as fs;
use parking_lot::Mutex;
use rayon::prelude::*;
use serde::Serialize;
use tracing::{debug, info, warn};

/// Statistics about the cache cleanup operation
#[derive(Debug, Clone, Serialize)]
pub struct CleanupStats {
    /// Number of packages removed
    pub packages_removed: usize,
    /// Total space freed in bytes
    pub space_freed: u64,
    /// Number of packages kept
    pub packages_kept: usize,
    /// List of removed package paths
    pub removed_packages: Vec<PathBuf>,
    /// Total size of cache before cleanup
    pub total_cache_size: u64,
    /// Total size of cache after cleanup
    pub total_cache_size_after: u64,
    /// Number of packages linked to environments
    pub packages_in_use: usize,
    /// Number of packages removed by age
    pub removed_by_age: usize,
    /// Number of packages removed by space constraints
    pub removed_by_space: usize,
    /// Packages that failed to remove
    pub failed_removals: Vec<(PathBuf, String)>,
    /// Time taken for cleanup operation
    pub cleanup_duration: Duration,
    /// Number of high priority packages preserved
    pub high_priority_preserved: usize,
}

/// Priority level for cached packages
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PackagePriority {
    Low,
    Normal,
    High,
}

impl Default for PackagePriority {
    fn default() -> Self {
        Self::Normal
    }
}

/// Options for cache cleanup
#[derive(Debug, Clone)]
pub struct CleanupOptions {
    /// Maximum age of cached packages (packages older than this will be removed)
    pub max_age: Option<Duration>,
    /// Whether to perform a dry run (don't actually delete files)
    pub dry_run: bool,
    /// Minimum free space to keep in bytes (will remove oldest packages until this much space is free)
    pub min_free_space: Option<u64>,
    /// Whether to remove packages not linked to any environment
    pub remove_unlinked: bool,
    /// Package priorities (packages with high priority are preserved unless space is critically low)
    pub package_priorities: HashMap<String, PackagePriority>,
    /// Whether to use parallel processing for large directories
    pub parallel_processing: bool,
    /// Minimum number of packages to trigger parallel processing
    pub parallel_threshold: usize,
}

impl Default for CleanupOptions {
    fn default() -> Self {
        Self {
            max_age: Some(Duration::from_secs(30 * 24 * 60 * 60)), // 30 days
            dry_run: false,
            min_free_space: None,
            remove_unlinked: true,
            package_priorities: HashMap::new(),
            parallel_processing: true,
            parallel_threshold: 1000,
        }
    }
}

/// Progress reporter for cleanup operations
pub trait CleanupProgress: Send + Sync {
    fn on_scan_progress(&self, scanned: usize, total: Option<usize>);
    fn on_remove_progress(&self, removed: usize, total: usize);
    fn on_complete(&self, stats: &CleanupStats);
}

/// Information about a cached package
#[derive(Debug)]
struct PackageInfo {
    path: PathBuf,
    size: u64,
    last_accessed: SystemTime,
    is_linked: bool,
    priority: PackagePriority,
}

/// Check if a package is linked to any of the given environments
fn is_package_linked(package_path: &PathBuf, environments: &[PathBuf]) -> bool {
    // Read the package's info/paths.json to get its files
    let paths_json = package_path.join("info/paths.json");
    if !paths_json.exists() {
        return false;
    }

    match fs::read_to_string(&paths_json) {
        Ok(content) => {
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(json) => {
                    if let Some(paths) = json.get("paths").and_then(|p| p.as_array()) {
                        // Get all file paths from the package
                        let package_files: HashSet<String> = paths
                            .iter()
                            .filter_map(|p| p.get("_path").and_then(|p| p.as_str()))
                            .map(|s| s.to_string())
                            .collect();

                        // Check each environment
                        for env in environments {
                            // Check if any package file exists in this environment
                            for file in &package_files {
                                if env.join(file).exists() {
                                    return true;
                                }
                            }
                        }
                    }
                }
                Err(e) => warn!("Failed to parse paths.json for {}: {}", package_path.display(), e),
            }
        }
        Err(e) => warn!("Failed to read paths.json for {}: {}", package_path.display(), e),
    }
    false
}

/// Get package name from path
fn get_package_name(path: &PathBuf) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|s| s.to_string())
}

/// Clean up the package cache according to the specified options
pub async fn cleanup_cache(
    cache_dir: PathBuf,
    options: CleanupOptions,
    environments: Option<&[PathBuf]>,
    progress: Option<Arc<dyn CleanupProgress>>,
) -> Result<CleanupStats> {
    let start_time = Instant::now();
    let mut stats = CleanupStats {
        packages_removed: 0,
        space_freed: 0,
        packages_kept: 0,
        removed_packages: Vec::new(),
        total_cache_size: 0,
        total_cache_size_after: 0,
        packages_in_use: 0,
        removed_by_age: 0,
        removed_by_space: 0,
        failed_removals: Vec::new(),
        cleanup_duration: Duration::default(),
        high_priority_preserved: 0,
    };

    // Get list of all packages in cache with their info
    let mut packages = Vec::new();
    let mut scanned = 0;
    let total_entries = fs::read_dir(&cache_dir)?.count();

    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            scanned += 1;
            if let Some(progress) = &progress {
                progress.on_scan_progress(scanned, Some(total_entries));
            }

            if path.is_dir() {
                if let Ok(metadata) = fs::metadata(&path) {
                    stats.total_cache_size += metadata.len();
                    let is_linked = if let Some(envs) = environments {
                        let linked = is_package_linked(&path, envs);
                        if linked {
                            stats.packages_in_use += 1;
                        }
                        linked
                    } else {
                        false
                    };

                    // Get package priority
                    let priority = get_package_name(&path)
                        .and_then(|name| options.package_priorities.get(&name))
                        .copied()
                        .unwrap_or_default();

                    packages.push(PackageInfo {
                        path,
                        size: metadata.len(),
                        last_accessed: metadata.accessed().unwrap_or(SystemTime::UNIX_EPOCH),
                        is_linked,
                        priority,
                    });
                }
            }
        }
    }

    // Sort packages by priority (high to low) and then by last access time (oldest first)
    packages.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.last_accessed.cmp(&b.last_accessed)));

    let now = SystemTime::now();
    let stats = Arc::new(Mutex::new(stats));

    // Process packages in parallel if enabled and above threshold
    if options.parallel_processing && packages.len() >= options.parallel_threshold {
        packages.par_iter().enumerate().for_each(|(i, package)| {
            process_package(
                package,
                &options,
                now,
                &stats,
                i,
                packages.len(),
                progress.as_deref(),
            );
        });
    } else {
        for (i, package) in packages.iter().enumerate() {
            process_package(
                package,
                &options,
                now,
                &stats,
                i,
                packages.len(),
                progress.as_deref(),
            );
        }
    }

    let mut stats = Arc::try_unwrap(stats)
        .expect("All threads should be done with stats")
        .into_inner();

    // Remove packages to free up space if needed
    if let Some(min_free_space) = options.min_free_space {
        if let Ok(fs_stats) = fs2::available_space(&cache_dir) {
            if fs_stats < min_free_space {
                let space_needed = min_free_space - fs_stats;
                let mut space_freed = 0;

                for package in packages {
                    if space_freed >= space_needed {
                        break;
                    }

                    // Only remove high priority packages if absolutely necessary
                    if package.priority == PackagePriority::High && space_freed + package.size < space_needed {
                        continue;
                    }

                    if !stats.removed_packages.contains(&package.path) && !package.is_linked {
                        if !options.dry_run {
                            if let Err(e) = fs::remove_dir_all(&package.path) {
                                debug!("Failed to remove package {}: {}", package.path.display(), e);
                                stats.failed_removals.push((package.path.clone(), e.to_string()));
                                continue;
                            }
                        }
                        stats.packages_removed += 1;
                        stats.removed_by_space += 1;
                        stats.space_freed += package.size;
                        space_freed += package.size;
                        stats.removed_packages.push(package.path);
                        debug!("Removed package to free space: {}", package.path.display());
                    }
                }
            }
        }
    }

    // Calculate final statistics
    stats.total_cache_size_after = stats.total_cache_size - stats.space_freed;
    stats.cleanup_duration = start_time.elapsed();

    info!(
        "Cache cleanup complete: removed {}/{} packages ({} by age, {} by space), freed {} bytes. {} packages in use, {} high priority preserved. Duration: {:?}",
        stats.packages_removed,
        stats.packages_removed + stats.packages_kept,
        stats.removed_by_age,
        stats.removed_by_space,
        stats.space_freed,
        stats.packages_in_use,
        stats.high_priority_preserved,
        stats.cleanup_duration,
    );

    if !stats.failed_removals.is_empty() {
        warn!(
            "Failed to remove {} packages",
            stats.failed_removals.len()
        );
    }

    if let Some(progress) = progress {
        progress.on_complete(&stats);
    }

    Ok(stats)
}

fn process_package(
    package: &PackageInfo,
    options: &CleanupOptions,
    now: SystemTime,
    stats: &Arc<Mutex<CleanupStats>>,
    current: usize,
    total: usize,
    progress: Option<&dyn CleanupProgress>,
) {
    let mut stats = stats.lock();

    if package.is_linked {
        stats.packages_kept += 1;
        return;
    }

    if package.priority == PackagePriority::High {
        stats.high_priority_preserved += 1;
        stats.packages_kept += 1;
        return;
    }

    if let Some(max_age) = options.max_age {
        if let Ok(age) = now.duration_since(package.last_accessed) {
            if age > max_age || (options.remove_unlinked && !package.is_linked) {
                if !options.dry_run {
                    if let Err(e) = fs::remove_dir_all(&package.path) {
                        debug!("Failed to remove package {}: {}", package.path.display(), e);
                        stats.failed_removals.push((package.path.clone(), e.to_string()));
                        return;
                    }
                }
                stats.packages_removed += 1;
                stats.removed_by_age += 1;
                stats.space_freed += package.size;
                stats.removed_packages.push(package.path.clone());
                debug!("Removed old package: {}", package.path.display());

                if let Some(progress) = progress {
                    progress.on_remove_progress(current, total);
                }
            } else {
                stats.packages_kept += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use std::time::SystemTime;

    fn create_test_package(root: &PathBuf, name: &str, age: Duration) -> PathBuf {
        let pkg_dir = root.join(name);
        fs::create_dir_all(&pkg_dir).unwrap();
        
        // Create info/paths.json
        let info_dir = pkg_dir.join("info");
        fs::create_dir_all(&info_dir).unwrap();
        let paths_json = serde_json::json!({
            "paths": [
                {"_path": "bin/test"},
                {"_path": "lib/test.so"}
            ]
        });
        fs::write(
            info_dir.join("paths.json"),
            serde_json::to_string_pretty(&paths_json).unwrap(),
        ).unwrap();

        // Set access time
        let access_time = SystemTime::now() - age;
        filetime::set_file_atime(&pkg_dir, filetime::FileTime::from_system_time(access_time)).unwrap();

        pkg_dir
    }

    #[tokio::test]
    async fn test_cleanup_by_age() {
        let cache_dir = tempdir().unwrap();
        let cache_path = cache_dir.path().to_path_buf();

        // Create test packages
        let old_pkg = create_test_package(&cache_path, "old-pkg", Duration::from_secs(40 * 24 * 60 * 60));
        let new_pkg = create_test_package(&cache_path, "new-pkg", Duration::from_secs(10 * 24 * 60 * 60));

        let options = CleanupOptions {
            max_age: Some(Duration::from_secs(30 * 24 * 60 * 60)),
            dry_run: false,
            min_free_space: None,
            remove_unlinked: true,
            package_priorities: HashMap::new(),
            parallel_processing: false,
            parallel_threshold: 1000,
        };

        let stats = cleanup_cache(cache_path, options, None, None).await.unwrap();

        assert_eq!(stats.packages_removed, 1);
        assert_eq!(stats.packages_kept, 1);
        assert_eq!(stats.removed_by_age, 1);
        assert!(stats.removed_packages.contains(&old_pkg));
        assert!(!stats.removed_packages.contains(&new_pkg));
    }

    #[tokio::test]
    async fn test_cleanup_respects_linked_packages() {
        let cache_dir = tempdir().unwrap();
        let cache_path = cache_dir.path().to_path_buf();
        let env_dir = tempdir().unwrap();
        let env_path = env_dir.path().to_path_buf();

        // Create test packages
        let old_pkg = create_test_package(&cache_path, "old-pkg", Duration::from_secs(40 * 24 * 60 * 60));
        
        // Create environment with linked package
        fs::create_dir_all(env_path.join("bin")).unwrap();
        fs::write(env_path.join("bin/test"), "test").unwrap();

        let options = CleanupOptions {
            max_age: Some(Duration::from_secs(30 * 24 * 60 * 60)),
            dry_run: false,
            min_free_space: None,
            remove_unlinked: true,
            package_priorities: HashMap::new(),
            parallel_processing: false,
            parallel_threshold: 1000,
        };

        let stats = cleanup_cache(cache_path, options, Some(&[env_path]), None).await.unwrap();

        assert_eq!(stats.packages_removed, 0);
        assert_eq!(stats.packages_kept, 1);
        assert_eq!(stats.packages_in_use, 1);
        assert!(old_pkg.exists());
    }

    #[tokio::test]
    async fn test_dry_run() {
        let cache_dir = tempdir().unwrap();
        let cache_path = cache_dir.path().to_path_buf();

        // Create test package
        let old_pkg = create_test_package(&cache_path, "old-pkg", Duration::from_secs(40 * 24 * 60 * 60));

        let options = CleanupOptions {
            max_age: Some(Duration::from_secs(30 * 24 * 60 * 60)),
            dry_run: true,
            min_free_space: None,
            remove_unlinked: true,
            package_priorities: HashMap::new(),
            parallel_processing: false,
            parallel_threshold: 1000,
        };

        let stats = cleanup_cache(cache_path, options, None, None).await.unwrap();

        assert_eq!(stats.packages_removed, 1);
        assert!(stats.removed_packages.contains(&old_pkg));
        // In dry run mode, the package should still exist
        assert!(old_pkg.exists());
    }

    #[tokio::test]
    async fn test_cleanup_with_priorities() {
        let cache_dir = tempdir().unwrap();
        let cache_path = cache_dir.path().to_path_buf();

        // Create test packages with different priorities
        let high_pkg = create_test_package(&cache_path, "high-pkg", Duration::from_secs(40 * 24 * 60 * 60));
        let normal_pkg = create_test_package(&cache_path, "normal-pkg", Duration::from_secs(40 * 24 * 60 * 60));
        let low_pkg = create_test_package(&cache_path, "low-pkg", Duration::from_secs(40 * 24 * 60 * 60));

        let mut priorities = HashMap::new();
        priorities.insert("high-pkg".to_string(), PackagePriority::High);
        priorities.insert("low-pkg".to_string(), PackagePriority::Low);

        let options = CleanupOptions {
            max_age: Some(Duration::from_secs(30 * 24 * 60 * 60)),
            dry_run: false,
            min_free_space: None,
            remove_unlinked: true,
            package_priorities: priorities,
            parallel_processing: false,
            parallel_threshold: 1000,
        };

        let stats = cleanup_cache(cache_path, options, None, None).await.unwrap();

        assert_eq!(stats.high_priority_preserved, 1);
        assert!(high_pkg.exists());
        assert!(!low_pkg.exists());
        assert!(stats.removed_packages.contains(&low_pkg));
        assert!(stats.cleanup_duration.as_secs_f32() > 0.0);
    }

    #[tokio::test]
    async fn test_parallel_cleanup() {
        let cache_dir = tempdir().unwrap();
        let cache_path = cache_dir.path().to_path_buf();

        // Create many test packages
        for i in 0..1500 {
            create_test_package(
                &cache_path,
                &format!("pkg-{}", i),
                Duration::from_secs(40 * 24 * 60 * 60),
            );
        }

        let options = CleanupOptions {
            max_age: Some(Duration::from_secs(30 * 24 * 60 * 60)),
            dry_run: false,
            min_free_space: None,
            remove_unlinked: true,
            package_priorities: HashMap::new(),
            parallel_processing: true,
            parallel_threshold: 1000,
        };

        let stats = cleanup_cache(cache_path, options, None, None).await.unwrap();

        assert_eq!(stats.packages_removed, 1500);
        assert!(stats.cleanup_duration.as_secs_f32() > 0.0);
    }
} 