// Benchmark for tar.bz2 extraction with different scenarios and concurrency levels
// Run with: cargo bench --bench extraction_benchmark --package rattler_package_streaming --features reqwest --no-fail-fast

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use reqwest::Client;
use tempfile::TempDir;

/// Test packages across size ranges
struct TestPackage {
    name: &'static str,
    url: &'static str,
    size_category: &'static str,
}

const TEST_PACKAGES: &[TestPackage] = &[
    // Small: 100KB-500KB
    TestPackage {
        name: "zlib",
        url: "https://conda.anaconda.org/conda-forge/win-64/zlib-1.2.8-vc10_0.tar.bz2",
        size_category: "small-100KB",
    },
    TestPackage {
        name: "pytweening",
        url: "https://conda.anaconda.org/conda-forge/noarch/pytweening-1.0.4-pyhd8ed1ab_0.tar.bz2",
        size_category: "small-18KB",
    },
    // Medium: 500KB-2MB
    TestPackage {
        name: "mock",
        url: "https://conda.anaconda.org/conda-forge/win-64/mock-2.0.0-py37_1000.tar.bz2",
        size_category: "medium-500KB",
    },
    // Large: 2MB-10MB
    TestPackage {
        name: "mamba",
        url: "https://conda.anaconda.org/conda-forge/win-64/mamba-1.0.0-py38hecfeebb_2.tar.bz2",
        size_category: "large-2MB",
    },
    TestPackage {
        name: "micromamba",
        url: "https://conda.anaconda.org/conda-forge/win-64/micromamba-1.1.0-0.tar.bz2",
        size_category: "xlarge-8MB",
    },
];

#[derive(Debug, Clone)]
struct BenchmarkResults {
    scenario: String,
    concurrency: usize,
    total_time_ms: u64,
    throughput_pkg_per_sec: f64,
    individual_times_ms: Vec<u64>,
}

impl BenchmarkResults {
    fn avg_time_ms(&self) -> f64 {
        self.individual_times_ms.iter().sum::<u64>() as f64 / self.individual_times_ms.len() as f64
    }

    fn min_time_ms(&self) -> u64 {
        *self.individual_times_ms.iter().min().unwrap_or(&0)
    }

    fn max_time_ms(&self) -> u64 {
        *self.individual_times_ms.iter().max().unwrap_or(&0)
    }
}

/// Download all test packages to disk for pure extraction benchmark
async fn download_test_packages(
    cache_dir: &std::path::Path,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let client = Client::new();
    let mut paths = Vec::new();

    println!("Downloading test packages...");
    for pkg in TEST_PACKAGES {
        let file_name = pkg.url.split('/').next_back().unwrap();
        let file_path = cache_dir.join(file_name);

        if file_path.exists() {
            println!("  Using cached {} ({})", pkg.name, pkg.size_category);
        } else {
            println!("  Downloading {} ({})", pkg.name, pkg.size_category);
            let response = client.get(pkg.url).send().await?;
            let bytes = response.bytes().await?;
            std::fs::write(&file_path, bytes)?;
        }

        paths.push(file_path);
    }

    Ok(paths)
}

/// Generic benchmark runner that spawns tasks and collects results
async fn run_benchmark<F, Fut>(
    scenario: &str,
    concurrency: usize,
    task_fn: F,
) -> Result<BenchmarkResults, Box<dyn std::error::Error>>
where
    F: Fn(usize) -> Fut,
    Fut: std::future::Future<Output = Result<Duration, String>> + Send + 'static,
{
    let mut tasks = Vec::new();
    let start = Instant::now();

    for i in 0..concurrency {
        tasks.push(tokio::spawn(task_fn(i)));
    }

    let mut individual_times = Vec::new();
    for task in tasks {
        let duration = task.await.map_err(|e| format!("Task failed: {e:?}"))??;
        individual_times.push(duration.as_millis() as u64);
    }

    let total_time = start.elapsed();
    let throughput = concurrency as f64 / total_time.as_secs_f64();

    Ok(BenchmarkResults {
        scenario: scenario.to_string(),
        concurrency,
        total_time_ms: total_time.as_millis() as u64,
        throughput_pkg_per_sec: throughput,
        individual_times_ms: individual_times,
    })
}

/// Benchmark extraction from pre-downloaded packages
async fn benchmark_extraction(
    scenario: &str,
    package_paths: &[PathBuf],
    concurrency: usize,
) -> Result<BenchmarkResults, Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let package_paths = package_paths.to_vec();

    run_benchmark(scenario, concurrency, move |i| {
        let pkg_path = package_paths[i % package_paths.len()].clone();
        let dest = temp_dir.path().join(format!("extract_{i}"));

        async move {
            std::fs::create_dir_all(&dest).map_err(|e| format!("Failed to create dir: {e:?}"))?;

            let task_start = Instant::now();
            rattler_package_streaming::tokio::fs::extract(&pkg_path, &dest, None)
                .await
                .map_err(|e| format!("Extraction failed: {e:?}"))?;
            Ok(task_start.elapsed())
        }
    })
    .await
}

/// Benchmark download + extract
async fn benchmark_download_and_extract(
    concurrency: usize,
) -> Result<BenchmarkResults, Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let client = reqwest_middleware::ClientWithMiddleware::from(Client::new());

    run_benchmark("Download+Extract", concurrency, move |i| {
        let pkg = &TEST_PACKAGES[i % TEST_PACKAGES.len()];
        let url = pkg.url.to_string();
        let dest = temp_dir.path().join(format!("extract_{i}"));
        let client = client.clone();

        async move {
            std::fs::create_dir_all(&dest).map_err(|e| format!("Failed to create dir: {e:?}"))?;

            let task_start = Instant::now();
            rattler_package_streaming::reqwest::tokio::extract(
                client,
                url.parse().unwrap(),
                &dest,
                None,
                None,
                None,
            )
            .await
            .map_err(|e| format!("Download+Extract failed: {e:?}"))?;
            Ok(task_start.elapsed())
        }
    })
    .await
}

fn print_results(results: &BenchmarkResults) {
    println!(
        "\n{} - Concurrency: {}",
        results.scenario, results.concurrency
    );
    println!("  Total time: {}ms", results.total_time_ms);
    println!(
        "  Throughput: {:.2} packages/sec",
        results.throughput_pkg_per_sec
    );
    println!("  Avg per package: {:.2}ms", results.avg_time_ms());
    println!(
        "  Min/Max: {}ms / {}ms",
        results.min_time_ms(),
        results.max_time_ms()
    );
}

fn save_results_to_file(all_results: &[BenchmarkResults], filename: &str) -> std::io::Result<()> {
    let mut file = std::fs::File::create(filename)?;

    writeln!(file, "# Benchmark Results\n")?;
    writeln!(file, "| Scenario | Concurrency | Total Time (ms) | Throughput (pkg/s) | Avg (ms) | Min (ms) | Max (ms) |")?;
    writeln!(file, "|----------|-------------|-----------------|-------------------|----------|----------|----------|")?;

    for result in all_results {
        writeln!(
            file,
            "| {} | {} | {} | {:.2} | {:.2} | {} | {} |",
            result.scenario,
            result.concurrency,
            result.total_time_ms,
            result.throughput_pkg_per_sec,
            result.avg_time_ms(),
            result.min_time_ms(),
            result.max_time_ms()
        )?;
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Tar.bz2 Extraction Benchmark ===\n");

    // Setup: Download packages for extraction benchmarks
    let cache_dir = std::env::temp_dir().join("rattler_bench_cache");
    std::fs::create_dir_all(&cache_dir)?;
    let package_paths = download_test_packages(&cache_dir).await?;

    let mut all_results = Vec::new();

    // Run benchmarks at different concurrency levels
    for concurrency in [8, 16, 32] {
        println!("\n=== Concurrency Level: {concurrency} ===");

        // Scenario 1: Pure extraction
        println!("\nRunning Scenario 1: Pure Extraction...");
        let result = benchmark_extraction("Pure Extraction", &package_paths, concurrency).await?;
        print_results(&result);
        all_results.push(result);

        // Scenario 2: Download + Extract
        println!("\nRunning Scenario 2: Download + Extract...");
        let result = benchmark_download_and_extract(concurrency).await?;
        print_results(&result);
        all_results.push(result);

        // Scenario 3: Mixed workload
        println!("\nRunning Scenario 3: Mixed Workload...");
        let result = benchmark_extraction("Mixed Workload", &package_paths, concurrency).await?;
        print_results(&result);
        all_results.push(result);
    }

    // Save results
    save_results_to_file(&all_results, "benchmark_results.md")?;
    println!("\n\nResults saved to benchmark_results.md");

    Ok(())
}
