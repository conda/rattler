use std::collections::HashMap;
use std::path::Path;

use criterion::{criterion_group, criterion_main, Criterion};
use rattler_conda_types::{Channel, MatchSpec, ParseMatchSpecOptions, Platform};
use rattler_repodata_gateway::{Gateway, RepoData};
use url::Url;

fn create_gateway(cache_dir: &Path) -> Gateway {
    Gateway::builder()
        .with_cache_dir(cache_dir)
        .with_channel_config(rattler_repodata_gateway::ChannelConfig {
            default: rattler_repodata_gateway::SourceConfig {
                sharded_enabled: true,
                ..rattler_repodata_gateway::SourceConfig::default()
            },
            per_channel: HashMap::default(),
        })
        .finish()
}

fn channel() -> Channel {
    Channel::from_url(Url::parse("https://conda.anaconda.org/conda-forge").unwrap())
}

fn specs() -> Vec<MatchSpec> {
    vec![MatchSpec::from_str("rubin-env", ParseMatchSpecOptions::default()).unwrap()]
}

fn bench_rubin_env(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let channel = channel();
    let specs = specs();

    // Persistent cache dir for warm-cache benchmarks
    let warm_dir = tempfile::tempdir().unwrap();
    let warm_gateway = create_gateway(warm_dir.path());

    // Warm up: populate the cache
    let records = rt.block_on(async {
        warm_gateway
            .query(
                vec![channel.clone()],
                [Platform::Linux64, Platform::NoArch],
                specs.clone(),
            )
            .recursive(true)
            .await
            .unwrap()
    });
    let total: usize = records.iter().map(RepoData::len).sum();
    eprintln!("Warm-up: loaded {total} records");

    let mut group = c.benchmark_group("gateway_load");
    group.sample_size(10);

    // Warm disk cache: on-disk shards are cached but a fresh Gateway is created
    // each iteration, forcing deserialization from disk (no in-memory cache).
    // This is what users actually experience on `rattler create`.
    group.bench_function("rubin-env/warm_disk_cache", |b| {
        b.iter(|| {
            let fresh_gateway = create_gateway(warm_dir.path());
            rt.block_on(async {
                let records = fresh_gateway
                    .query(
                        vec![channel.clone()],
                        [Platform::Linux64, Platform::NoArch],
                        specs.clone(),
                    )
                    .recursive(true)
                    .await
                    .unwrap();
                let total: usize = records.iter().map(RepoData::len).sum();
                std::hint::black_box(total)
            })
        });
    });

    // In-memory cache: same Gateway instance, records already in CoalescedMap.
    // Measures pure query/dependency-resolution overhead without any I/O.
    group.bench_function("rubin-env/in_memory", |b| {
        b.iter(|| {
            rt.block_on(async {
                let records = warm_gateway
                    .query(
                        vec![channel.clone()],
                        [Platform::Linux64, Platform::NoArch],
                        specs.clone(),
                    )
                    .recursive(true)
                    .await
                    .unwrap();
                let total: usize = records.iter().map(RepoData::len).sum();
                std::hint::black_box(total)
            })
        });
    });

    // Cold cache: fresh cache dir each iteration, forces fetch + write + read.
    // Setup and teardown (tempdir drop) are outside the measured region.
    group.bench_function("rubin-env/cold_cache", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let gw = create_gateway(dir.path());
                (dir, gw)
            },
            |(_dir, cold_gateway)| {
                rt.block_on(async {
                    let records = cold_gateway
                        .query(
                            vec![channel.clone()],
                            [Platform::Linux64, Platform::NoArch],
                            specs.clone(),
                        )
                        .recursive(true)
                        .await
                        .unwrap();
                    let total: usize = records.iter().map(RepoData::len).sum();
                    std::hint::black_box(total)
                })
            },
            criterion::BatchSize::PerIteration,
        );
    });

    group.finish();
}

criterion_group!(benches, bench_rubin_env);
criterion_main!(benches);
