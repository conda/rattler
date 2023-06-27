use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rattler_conda_types::Version;
use std::str::FromStr;

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("parse simple version", |b| {
        b.iter(|| black_box("3.11.4").parse::<Version>())
    });
    c.bench_function("parse complex version", |b| {
        b.iter(|| black_box("1!1.0b2.post345.dev456+3.2.20.rc3").parse::<Version>())
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
