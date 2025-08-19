use divan::{black_box, Bencher};
use path_resolver::{PackageName, PathResolver};
use std::path::PathBuf;

fn main() {
    divan::main();
}

fn generate_test_paths(size: usize, depth: usize) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(size);

    for i in 0..size {
        let mut path = PathBuf::new();

        // Create nested directory structure
        for d in 0..depth {
            path.push(format!("dir_{d}"));
        }

        // Add a file at the end
        path.push(format!("file_{i}.txt"));
        paths.push(path);
    }

    paths
}

fn generate_conflicting_paths(base_size: usize) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(base_size);

    // Create paths that will conflict with each other
    for i in 0..base_size / 2 {
        paths.push(PathBuf::from(format!("shared/file_{i}.txt")));
        paths.push(PathBuf::from(format!("shared/file_{i}.txt"))); // Duplicate for conflict
    }

    paths
}

#[divan::bench(args = [10, 100, 1000, 5000, 10000])]
fn insert_package_no_conflicts(bencher: Bencher<'_, '_>, size: usize) {
    let paths = generate_test_paths(size, 3);
    let package_name: PackageName = "test_package".into();

    bencher.bench_local(|| {
        let mut resolver = black_box(PathResolver::new());
        let conflicts = resolver.insert_package(black_box(package_name.clone()), black_box(&paths));
        black_box(conflicts)
    });
}

#[divan::bench(args = [10, 100, 500, 2000, 5000])]
fn insert_package_with_conflicts(bencher: Bencher<'_, '_>, size: usize) {
    let paths1 = generate_test_paths(size / 2, 2);
    let paths2 = generate_conflicting_paths(size);

    bencher.bench_local(|| {
        let mut resolver = black_box(PathResolver::new());

        // Insert first package
        let _ = resolver.insert_package("package1".into(), black_box(&paths1));

        // Insert second package with conflicts
        let conflicts = resolver.insert_package("package2".into(), black_box(&paths2));
        black_box(conflicts)
    });
}

#[divan::bench(args = [5, 10, 20, 50, 100])]
fn insert_multiple_packages(bencher: Bencher<'_, '_>, num_packages: usize) {
    let paths_per_package = generate_test_paths(50, 2);

    bencher.bench_local(|| {
        let mut resolver = black_box(PathResolver::new());

        for i in 0..num_packages {
            let package_name: PackageName = format!("package_{i}").into();
            let conflicts = resolver.insert_package(black_box(package_name), black_box(&paths_per_package));
            black_box(conflicts);
        }

        resolver
    });
}

#[divan::bench(args = [1, 2, 3, 4, 5, 8, 12])]
fn insert_package_varying_depth(bencher: Bencher<'_, '_>, depth: usize) {
    let paths = generate_test_paths(100, depth);
    let package_name: PackageName = "test_package".into();

    bencher.bench_local(|| {
        let mut resolver = black_box(PathResolver::new());
        let conflicts = resolver.insert_package(black_box(package_name.clone()), black_box(&paths));
        black_box(conflicts)
    });
}

#[divan::bench(args = [1000, 5000, 10000, 25000])]
fn insert_package_heavy_stress(bencher: Bencher<'_, '_>, size: usize) {
    let paths = generate_test_paths(size, 5);
    let package_name: PackageName = "heavy_package".into();

    bencher.bench_local(|| {
        let mut resolver = black_box(PathResolver::new());
        let conflicts = resolver.insert_package(black_box(package_name.clone()), black_box(&paths));
        black_box(conflicts)
    });
}

#[divan::bench(args = [100, 500, 1000])]
fn insert_multiple_heavy_packages(bencher: Bencher<'_, '_>, num_packages: usize) {
    let paths_per_package = generate_test_paths(200, 3);

    bencher.bench_local(|| {
        let mut resolver = black_box(PathResolver::new());

        for i in 0..num_packages {
            let package_name: PackageName = format!("heavy_package_{i}").into();
            let conflicts = resolver.insert_package(black_box(package_name), black_box(&paths_per_package));
            black_box(conflicts);
        }

        resolver
    });
}

#[divan::bench(args = [1000, 5000, 10000])]
fn insert_package_massive_conflicts(bencher: Bencher<'_, '_>, size: usize) {
    // Create paths where every file will conflict
    let mut conflicting_paths = Vec::with_capacity(size);
    for i in 0..size {
        conflicting_paths.push(PathBuf::from(format!("shared_file_{}.txt", i % 100))); // Reuse filenames to force conflicts
    }

    bencher.bench_local(|| {
        let mut resolver = black_box(PathResolver::new());

        // Insert first package
        let _ = resolver.insert_package("package1".into(), black_box(&conflicting_paths[0..size/2]));

        // Insert second package with maximum conflicts
        let conflicts = resolver.insert_package("package2".into(), black_box(&conflicting_paths));
        black_box(conflicts)
    });
}
