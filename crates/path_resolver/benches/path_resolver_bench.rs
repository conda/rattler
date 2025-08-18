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
            path.push(format!("dir_{}", d));
        }

        // Add a file at the end
        path.push(format!("file_{}.txt", i));
        paths.push(path);
    }

    paths
}

fn generate_conflicting_paths(base_size: usize) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(base_size);

    // Create paths that will conflict with each other
    for i in 0..base_size / 2 {
        paths.push(PathBuf::from(format!("shared/file_{}.txt", i)));
        paths.push(PathBuf::from(format!("shared/file_{}.txt", i))); // Duplicate for conflict
    }

    paths
}

#[divan::bench(args = [10, 100, 1000, 5000, 10000])]
fn insert_package_no_conflicts(bencher: Bencher<'_, '_>, size: usize) {
    let paths = generate_test_paths(size, 3);
    let package_name: PackageName = "test_package".to_string();

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
        let _ = resolver.insert_package("package1".to_string(), black_box(&paths1));

        // Insert second package with conflicts
        let conflicts = resolver.insert_package("package2".to_string(), black_box(&paths2));
        black_box(conflicts)
    });
}

#[divan::bench(args = [5, 10, 20, 50, 100])]
fn insert_multiple_packages(bencher: Bencher<'_, '_>, num_packages: usize) {
    let paths_per_package = generate_test_paths(50, 2);

    bencher.bench_local(|| {
        let mut resolver = black_box(PathResolver::new());

        for i in 0..num_packages {
            let package_name = format!("package_{}", i);
            let conflicts = resolver.insert_package(black_box(package_name), black_box(&paths_per_package));
            black_box(conflicts);
        }

        resolver
    });
}

#[divan::bench(args = [1, 2, 3, 4, 5, 8, 12])]
fn insert_package_varying_depth(bencher: Bencher<'_, '_>, depth: usize) {
    let paths = generate_test_paths(100, depth);
    let package_name: PackageName = "test_package".to_string();

    bencher.bench_local(|| {
        let mut resolver = black_box(PathResolver::new());
        let conflicts = resolver.insert_package(black_box(package_name.clone()), black_box(&paths));
        black_box(conflicts)
    });
}

#[divan::bench]
fn insert_package_realistic_scenario(bencher: Bencher<'_, '_>) {
    // Simulate a realistic package with mixed file types and directory structures
    let mut paths = Vec::new();

    // Add some top-level files
    paths.push(PathBuf::from("README.md"));
    paths.push(PathBuf::from("LICENSE"));
    paths.push(PathBuf::from("setup.py"));

    // Add library files
    for i in 0..20 {
        paths.push(PathBuf::from(format!("lib/module_{}.py", i)));
    }

    // Add binary files
    for i in 0..5 {
        paths.push(PathBuf::from(format!("bin/tool_{}", i)));
    }

    // Add documentation
    for i in 0..10 {
        paths.push(PathBuf::from(format!("docs/page_{}.md", i)));
    }

    // Add test files
    for i in 0..15 {
        paths.push(PathBuf::from(format!("tests/test_{}.py", i)));
    }

    let package_name: PackageName = "realistic_package".to_string();

    bencher.bench_local(|| {
        let mut resolver = black_box(PathResolver::new());
        let conflicts = resolver.insert_package(black_box(package_name.clone()), black_box(&paths));
        black_box(conflicts)
    });
}

#[divan::bench(args = [1000, 5000, 10000, 25000])]
fn insert_package_heavy_stress(bencher: Bencher<'_, '_>, size: usize) {
    let paths = generate_test_paths(size, 5);
    let package_name: PackageName = "heavy_package".to_string();

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
            let package_name = format!("heavy_package_{}", i);
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
        let _ = resolver.insert_package("package1".to_string(), black_box(&conflicting_paths[0..size/2]));

        // Insert second package with maximum conflicts
        let conflicts = resolver.insert_package("package2".to_string(), black_box(&conflicting_paths));
        black_box(conflicts)
    });
}

#[divan::bench]
fn insert_package_enterprise_simulation(bencher: Bencher<'_, '_>) {
    // Simulate a large enterprise package with complex directory structure
    let mut paths = Vec::new();

    // Root config files
    for config in ["package.json", "Cargo.toml", "setup.py", "requirements.txt", "Dockerfile", "docker-compose.yml"] {
        paths.push(PathBuf::from(config));
    }

    // Source code with nested modules
    for module in 0..100 {
        for file in 0..20 {
            paths.push(PathBuf::from(format!("src/module_{}/submodule_{}/file_{}.rs", module, file % 5, file)));
        }
    }

    // Test files
    for test in 0..200 {
        paths.push(PathBuf::from(format!("tests/unit/test_{}.rs", test)));
        paths.push(PathBuf::from(format!("tests/integration/integration_test_{}.rs", test)));
    }

    // Documentation
    for doc in 0..50 {
        paths.push(PathBuf::from(format!("docs/api/module_{}.md", doc)));
        paths.push(PathBuf::from(format!("docs/guides/guide_{}.md", doc)));
    }

    // Assets and resources
    for asset in 0..100 {
        paths.push(PathBuf::from(format!("assets/images/img_{}.png", asset)));
        paths.push(PathBuf::from(format!("assets/fonts/font_{}.ttf", asset)));
        paths.push(PathBuf::from(format!("resources/config/config_{}.yaml", asset)));
    }

    // Build artifacts simulation
    for artifact in 0..300 {
        paths.push(PathBuf::from(format!("target/debug/deps/lib_{}.rlib", artifact)));
        paths.push(PathBuf::from(format!("target/release/build/build_{}/output", artifact)));
    }

    let package_name: PackageName = "enterprise_package".to_string();

    bencher.bench_local(|| {
        let mut resolver = black_box(PathResolver::new());
        let conflicts = resolver.insert_package(black_box(package_name.clone()), black_box(&paths));
        black_box(conflicts)
    });
}
