//! Benchmark: concurrent vs serial package cache fetch.
//!
//! Run with: `cargo bench -p rattler_cache --bench concurrent_cache_lock`

use std::path::{Path, PathBuf};

use criterion::{criterion_group, criterion_main, Criterion};
use rattler_cache::package_cache::PackageCache;
use tempfile::tempdir;
use tokio::runtime::Runtime;

fn benchmark_package_paths() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data");
    vec![
        root.join("clobber/clobber-python-0.1.0-cpython.conda"),
        root.join("clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2"),
        root.join("clobber/clobber-2-0.1.0-h4616a5c_0.tar.bz2"),
        root.join("clobber/clobber-3-0.1.0-h4616a5c_0.tar.bz2"),
        root.join("packages/empty-0.1.0-h4616a5c_0.conda"),
    ]
}

fn criterion_benchmark(c: &mut Criterion) {
    let package_paths = benchmark_package_paths();
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("cache_lock");
    group.sample_size(10);

    group.bench_function("concurrent_per_package_lock", |b| {
        b.iter(|| {
            let cache = PackageCache::new(tempdir().unwrap().path());
            let paths = package_paths.clone();
            rt.block_on(async {
                let handles: Vec<_> = paths
                    .iter()
                    .map(|path| {
                        let cache = cache.clone();
                        let path = path.clone();
                        tokio::spawn(async move {
                            cache.get_or_fetch_from_path(&path, None).await
                        })
                    })
                    .collect();
                for handle in handles {
                    handle.await.unwrap().unwrap();
                }
            })
        });
    });

    group.bench_function("serial_simulating_global_lock", |b| {
        b.iter(|| {
            let cache = PackageCache::new(tempdir().unwrap().path());
            let paths = package_paths.clone();
            rt.block_on(async {
                for path in &paths {
                    cache.get_or_fetch_from_path(path, None).await.unwrap();
                }
            })
        });
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
