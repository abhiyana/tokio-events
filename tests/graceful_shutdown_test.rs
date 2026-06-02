use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio_events::{Event, EventBus};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event)]
struct SlowEvent {
    id: u64,
}

#[tokio::test]
async fn test_graceful_shutdown_processes_all_events() {
    let bus = EventBus::builder().build().await.unwrap();

    let processed_count = Arc::new(AtomicU64::new(0));
    let count_clone = processed_count.clone();

    // Subscribe a handler that takes some time to process
    let _handle = bus
        .subscribe(move |event: SlowEvent| {
            let counter = count_clone.clone();
            async move {
                // Simulate some work
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                counter.fetch_add(1, Ordering::Relaxed);
                println!("Processed event {}", event.id);
            }
        })
        .await
        .unwrap();

    // Publish 10 events
    for i in 0..10 {
        bus.publish(SlowEvent { id: i }).await.unwrap();
    }

    // Immediately shut down gracefully
    // It should block until all 10 events are processed (takes ~500ms since we have 1 worker)
    bus.shutdown_gracefully().await.unwrap();

    // Verify all 10 events were processed
    assert_eq!(processed_count.load(Ordering::Relaxed), 10);
}
