//! Benchmarks for the correlation service and client order ID generation.
//!
//! These benchmarks measure the performance of critical paths in the correlation
//! service, which is responsible for matching order responses to pending requests.
//!
//! Run with: `cargo bench -p lib-core`

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use lib_core::{generate_client_order_id, CorrelationService};
use std::time::Duration;

/// Benchmark client order ID generation.
fn bench_generate_client_order_id(c: &mut Criterion) {
    c.bench_function("generate_client_order_id", |b| {
        b.iter(|| black_box(generate_client_order_id()));
    });
}

/// Benchmark registering pending requests.
fn bench_register_request(c: &mut Criterion) {
    let service = CorrelationService::new();
    let mut counter = 0u64;

    c.bench_function("register_request", |b| {
        b.iter(|| {
            counter += 1;
            let _rx = service.register_request(black_box(counter));
        });
    });
}

/// Benchmark pending count check.
fn bench_pending_count(c: &mut Criterion) {
    let service = CorrelationService::new();

    // Pre-register some pending orders
    for i in 0..1000 {
        let _rx = service.register_request(i);
    }

    c.bench_function("pending_count", |b| {
        b.iter(|| black_box(service.pending_count()));
    });
}

/// Benchmark service creation.
fn bench_service_creation(c: &mut Criterion) {
    c.bench_function("service_creation", |b| {
        b.iter(|| black_box(CorrelationService::new()));
    });
}

/// Benchmark concurrent registrations.
fn bench_concurrent_registrations(c: &mut Criterion) {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut group = c.benchmark_group("concurrent_registrations");

    for num_ops in [10, 100, 1000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(num_ops),
            num_ops,
            |b, &num_ops| {
                let service = CorrelationService::new();
                let counter = Arc::new(AtomicU64::new(0));

                b.iter(|| {
                    // Simulate concurrent registrations by doing many in sequence
                    // (actual concurrency would require tokio runtime in benchmark)
                    for _ in 0..num_ops {
                        let id = counter.fetch_add(1, Ordering::Relaxed);
                        let _rx = service.register_request(id);
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark with varying service sizes (pre-populated).
fn bench_service_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("service_scaling");
    group.measurement_time(Duration::from_secs(3));

    for size in [100, 1000, 10000].iter() {
        let service = CorrelationService::new();

        // Pre-populate the service
        for i in 0..*size as u64 {
            let _rx = service.register_request(i);
        }

        let base_id = *size as u64;

        group.bench_with_input(
            BenchmarkId::new("register_with_pending", size),
            size,
            |b, _| {
                let mut counter = base_id;
                b.iter(|| {
                    counter += 1;
                    let _rx = service.register_request(black_box(counter));
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("pending_count_with_size", size),
            size,
            |b, _| {
                b.iter(|| black_box(service.pending_count()));
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_generate_client_order_id,
    bench_register_request,
    bench_pending_count,
    bench_service_creation,
    bench_concurrent_registrations,
    bench_service_scaling,
);

criterion_main!(benches);
