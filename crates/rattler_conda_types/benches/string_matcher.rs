use std::{hint::black_box, str::FromStr};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rattler_conda_types::StringMatcher;

fn string_matcher_benchmarks(c: &mut Criterion) {
    let inputs = [
        "py39h1234567_0",
        "py310h1234567_0",
        "PY39H1234567_0",
        "h1234567_openblas",
        "h1234567_OPENBLAS",
        "h1234567_mkl",
        "py39_openblas",
        "",
    ];
    let mut group = c.benchmark_group("StringMatcher");
    for pattern in ["*", "py39*", "*openblas", "py*blas"] {
        // Compare both representations in one process, with construction
        // excluded. The glob variant uses the original general matcher.
        for (strategy, matcher) in [
            (
                "glob",
                StringMatcher::Glob(Box::new(glob::Pattern::new(pattern).unwrap())),
            ),
            ("compiled", StringMatcher::from_str(pattern).unwrap()),
        ] {
            group.bench_with_input(
                BenchmarkId::new(pattern, strategy),
                &matcher,
                |b, matcher| {
                    b.iter(|| {
                        let matcher = black_box(matcher);
                        for input in black_box(&inputs) {
                            black_box(matcher.matches(black_box(input)));
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

criterion_group!(benches, string_matcher_benchmarks);
criterion_main!(benches);
