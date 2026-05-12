use criterion::{Criterion, criterion_group, criterion_main};
use std::time::Duration;

/// Build a path to a file in the `input/` test data directory.
fn input_dir() -> String {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
    format!("{}/input", manifest_dir)
}

fn bench_full_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_pipeline");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    // Auto-discover all PDFs in `input/`.
    let mut pdfs: Vec<_> = std::fs::read_dir(input_dir())
        .expect("input/ directory must exist")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("pdf") {
                Some(path)
            } else {
                None
            }
        })
        .collect();
    pdfs.sort();

    for pdf_path in &pdfs {
        let name = pdf_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let path_str = pdf_path.to_str().unwrap();

        group.bench_function(name, |b| {
            b.iter(|| {
                let result =
                    blueprint::run_exhaustive_to_merged(criterion::black_box(path_str)).unwrap();
                criterion::black_box(result);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_full_pipeline);
criterion_main!(benches);
