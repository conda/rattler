use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rattler_conda_types::Version;

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("parse simple version", |b| {
        b.iter(|| black_box("3.11.4").parse::<Version>());
    });
    c.bench_function("parse complex version", |b| {
        b.iter(|| black_box("1!1.0b2.post345.dev456+3.2.20.rc3").parse::<Version>());
    });
    c.bench_function("parse logical constraint", |b| {
        b.iter(|| black_box(">=3.1").parse::<Version>());
    });
    c.bench_function("parse wildcard constraint", |b| {
        b.iter(|| black_box("3.1.*").parse::<Version>());
    });
    c.bench_function("parse simple version spec", |b| {
        b.iter(|| black_box(">=3.1").parse::<Version>());
    });
    c.bench_function("parse complex version spec", |b| {
        b.iter(|| black_box("(>=2.1.0,<3.0)|(~=3.2.1,~3.2.2.1)|(==4.1)").parse::<Version>());
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
