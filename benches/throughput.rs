use criterion::{criterion_group, criterion_main, Criterion};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{broadcast, mpsc, Notify};
use tokio_events::{Event, EventBusBuilder};

#[derive(Event, Clone, Debug, Serialize, Deserialize)]
struct BenchEvent {}

/// Benchmarks raw `tokio::sync::mpsc` performance (theoretical max throughput for a single consumer)
async fn bench_raw_mpsc(n: usize) {
    let (tx, mut rx) = mpsc::channel(n * 2);

    let notify = Arc::new(Notify::new());
    let c_notify = notify.clone();

    tokio::spawn(async move {
        let mut count = 0;
        while rx.recv().await.is_some() {
            count += 1;
            if count == n {
                c_notify.notify_one();
                break;
            }
        }
    });

    for _ in 0..n {
        tx.send(()).await.unwrap();
    }

    notify.notified().await;
}

/// Benchmarks raw `tokio::sync::broadcast` performance (theoretical max throughput for Pub/Sub routing)
async fn bench_raw_broadcast(n: usize) {
    let (tx, mut rx) = broadcast::channel(n * 2);

    let notify = Arc::new(Notify::new());
    let c_notify = notify.clone();

    tokio::spawn(async move {
        let mut count = 0;
        while rx.recv().await.is_ok() {
            count += 1;
            if count == n {
                c_notify.notify_one();
                break;
            }
        }
    });

    for _ in 0..n {
        let _ = tx.send(()); // Broadcast doesn't need to await
    }

    notify.notified().await;
}

/// Benchmarks `tokio-events` performance (measures routing, topic Trie, and abstraction overhead)
async fn bench_tokio_events(n: usize) {
    let bus = Arc::new(
        EventBusBuilder::new()
            .high_throughput() // Disable disk persistence, maximize concurrency
            .build()
            .await
            .unwrap(),
    );

    let count = Arc::new(AtomicUsize::new(0));
    let notify = Arc::new(Notify::new());

    let c_count = count.clone();
    let c_notify = notify.clone();

    bus.subscribe(move |_: BenchEvent| {
        let c_count = c_count.clone();
        let c_notify = c_notify.clone();
        async move {
            if c_count.fetch_add(1, Ordering::Relaxed) + 1 == n {
                c_notify.notify_one();
            }
        }
    })
    .await
    .unwrap()
    .detach();

    for _ in 0..n {
        bus.publish(BenchEvent {}).await.unwrap();
    }

    notify.notified().await;

    // Fast teardown
    bus.shutdown().await.unwrap();
}

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Throughput (10,000 Events)");
    let n_events = 10_000;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    group.bench_function("tokio::sync::mpsc", |b| {
        b.to_async(&runtime).iter(|| bench_raw_mpsc(n_events))
    });

    group.bench_function("tokio::sync::broadcast", |b| {
        b.to_async(&runtime)
            .iter(|| bench_raw_broadcast(n_events))
    });

    group.bench_function("tokio-events (High Throughput)", |b| {
        b.to_async(&runtime)
            .iter(|| bench_tokio_events(n_events))
    });

    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
