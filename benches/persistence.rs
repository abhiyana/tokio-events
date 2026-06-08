use criterion::{criterion_group, criterion_main, Criterion};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Notify;
use tokio_events::{Event, EventBusBuilder};

#[derive(Event, Clone, Debug, Serialize, Deserialize)]
struct PersistEvent {}

async fn bench_memory(n: usize) {
    let bus = Arc::new(
        EventBusBuilder::new()
            .high_throughput()
            .build()
            .await
            .unwrap(),
    );

    let count = Arc::new(AtomicUsize::new(0));
    let notify = Arc::new(Notify::new());

    let c_count = count.clone();
    let c_notify = notify.clone();

    bus.subscribe(move |_: PersistEvent| {
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

    let mut tasks = Vec::new();
    for _ in 0..n {
        let bus_clone = bus.clone();
        tasks.push(tokio::spawn(async move {
            bus_clone.publish(PersistEvent {}).await.unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    notify.notified().await;
    bus.shutdown().await.unwrap();
}

async fn bench_redb(n: usize) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("bench_queue.redb");

    let bus = Arc::new(
        EventBusBuilder::new()
            .reliable()
            .with_redb_path(db_path.to_str().unwrap())
            .build()
            .await
            .unwrap(),
    );

    let count = Arc::new(AtomicUsize::new(0));
    let notify = Arc::new(Notify::new());

    let c_count = count.clone();
    let c_notify = notify.clone();

    bus.subscribe(move |_: PersistEvent| {
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

    let mut tasks = Vec::new();
    for _ in 0..n {
        let bus_clone = bus.clone();
        tasks.push(tokio::spawn(async move {
            bus_clone.publish(PersistEvent {}).await.unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    notify.notified().await;
    bus.shutdown().await.unwrap();
}

async fn bench_redb_async(n: usize) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("bench_queue_async.redb");

    let bus = Arc::new(
        EventBusBuilder::new()
            .with_redb_path(db_path.to_str().unwrap())
            .wait_for_persistence(false) // Use the OS Page Cache
            .build()
            .await
            .unwrap(),
    );

    let count = Arc::new(AtomicUsize::new(0));
    let notify = Arc::new(Notify::new());

    let c_count = count.clone();
    let c_notify = notify.clone();

    bus.subscribe(move |_: PersistEvent| {
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

    let mut tasks = Vec::new();
    for _ in 0..n {
        let bus_clone = bus.clone();
        tasks.push(tokio::spawn(async move {
            bus_clone.publish(PersistEvent {}).await.unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    notify.notified().await;
    bus.shutdown().await.unwrap();
}

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Persistence vs Memory (1,000 Events)");
    let n_events = 1000; // Reduced to 1k because disk fsyncs are slow

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    group.bench_function("Memory Routing", |b| {
        b.to_async(&runtime).iter(|| bench_memory(n_events))
    });

    // Reduce sample size to 10 because fsync is very slow on physical disks
    group.sample_size(10);
    group.bench_function("Redb Persistence Routing", |b| {
        b.to_async(&runtime).iter(|| bench_redb(n_events))
    });

    group.bench_function("Redb Page Cache Routing (wait_for_persistence=false)", |b| {
        b.to_async(&runtime).iter(|| bench_redb_async(n_events))
    });

    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
