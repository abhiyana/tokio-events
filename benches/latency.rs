use criterion::{criterion_group, criterion_main, Criterion};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_events::{Event, EventBusBuilder};

#[derive(Event, Clone, Debug, Serialize, Deserialize)]
struct BenchEvent {}

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Latency (Single Event Turnaround)");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    // Initialize bus OUTSIDE the benchmark loop
    let bus = runtime.block_on(async {
        Arc::new(
            EventBusBuilder::new()
                .high_throughput()
                .build()
                .await
                .unwrap(),
        )
    });

    let notify = Arc::new(Notify::new());
    let c_notify = notify.clone();

    // Subscribe once
    runtime.block_on(async {
        bus.subscribe(move |_: BenchEvent| {
            let c_notify = c_notify.clone();
            async move {
                c_notify.notify_one();
            }
        })
        .await
        .unwrap()
        .detach();
    });

    // We specifically measure only the `publish` -> `notified` time
    group.bench_function("publish_to_handler", |b| {
        b.to_async(&runtime).iter(|| async {
            bus.publish(BenchEvent {}).await.unwrap();
            notify.notified().await;
        })
    });

    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
