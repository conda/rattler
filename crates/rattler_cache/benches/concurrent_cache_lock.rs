// Run: cargo bench -p rattler_cache --bench concurrent_cache_lock
use std::path::{Path, PathBuf};
use criterion::{criterion_group, criterion_main, Criterion};
use rattler_cache::package_cache::PackageCache;
use tempfile::tempdir;
use tokio::runtime::Runtime;

fn paths() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-data");
    vec![
        root.join("clobber/clobber-python-0.1.0-cpython.conda"),
        root.join("clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2"),
        root.join("clobber/clobber-2-0.1.0-h4616a5c_0.tar.bz2"),
        root.join("clobber/clobber-3-0.1.0-h4616a5c_0.tar.bz2"),
        root.join("packages/empty-0.1.0-h4616a5c_0.conda"),
    ]
}

fn bench(c: &mut Criterion) {
    let paths = paths();
    let rt = Runtime::new().unwrap();
    let mut g = c.benchmark_group("cache_lock");
    g.sample_size(10);

    g.bench_function("concurrent_per_package_lock", |b| {
        b.iter(|| {
            let cache = PackageCache::new(tempdir().unwrap().path());
            let paths = paths.clone();
            rt.block_on(async {
                let handles: Vec<_> = paths.iter().map(|p| {
                    let c = cache.clone();
                    let p = p.clone();
                    tokio::spawn(async move { c.get_or_fetch_from_path(&p, None).await })
                }).collect();
                for h in handles { h.await.unwrap().unwrap(); }
            })
        });
    });

    g.bench_function("serial_simulating_global_lock", |b| {
        b.iter(|| {
            let cache = PackageCache::new(tempdir().unwrap().path());
            let paths = paths.clone();
            rt.block_on(async {
                for p in &paths { cache.get_or_fetch_from_path(p, None).await.unwrap(); }
            })
        });
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
