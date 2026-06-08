use criterion::{criterion_group, criterion_main, Criterion};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Notify;
use tokio_events::{Event, EventBusBuilder};

#[derive(Event, Clone, Debug, Serialize, Deserialize)]
struct RouteEvent {}

async fn bench_routing(n: usize, subscribe_topic: &str, publish_topic: &str) {
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

    bus.subscribe_topic(subscribe_topic, move |_: RouteEvent| {
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
        bus.publish_to(publish_topic, RouteEvent {}).await.unwrap();
    }

    notify.notified().await;
    bus.shutdown().await.unwrap();
}

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Topic Routing Overhead (10,000 Events)");
    let n_events = 10_000;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    group.bench_function("Exact Match (orders.eu)", |b| {
        b.to_async(&runtime).iter(|| bench_routing(n_events, "orders.eu", "orders.eu"))
    });

    group.bench_function("Single Wildcard (orders.*)", |b| {
        b.to_async(&runtime).iter(|| bench_routing(n_events, "orders.*", "orders.eu"))
    });

    group.bench_function("Multi Wildcard (orders.>)", |b| {
        b.to_async(&runtime).iter(|| bench_routing(n_events, "orders.>", "orders.eu.france.paris"))
    });

    group.finish();
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
