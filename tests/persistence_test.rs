#![cfg(feature = "persistence")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_events::{Event, EventBus};
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CriticalEvent {
    id: Uuid,
    data: String,
}

impl Event for CriticalEvent {
    fn event_type() -> &'static str {
        "CriticalEvent"
    }
}

#[tokio::test]
async fn test_redb_crash_recovery() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("events.redb");

    let event_id = Uuid::new_v4();

    // PHASE 1: Create bus, register subscriber, publish event, but CRASH before it processes.
    {
        let bus = EventBus::builder()
            .with_redb_path(&db_path)
            .build()
            .await
            .unwrap();

        // Subscribe but we will force drop the bus immediately after publish
        // To make sure it doesn't process, we won't even give tokio time to yield
        // Actually, we can just publish an event, and it writes synchronously to DB (wait, it writes in a blocking task)
        // Let's create a subscriber that takes a long time, or just let it process.
        // Wait, if it processes, it deletes the event! We need to stop it from processing.
        // We can just build the dispatcher manually and insert it, OR
        // simpler: subscribe with a handler that just sleeps forever!

        let handler_started = Arc::new(tokio::sync::Notify::new());
        let handler_started_clone = handler_started.clone();

        let _sub = bus
            .subscribe(move |_: CriticalEvent| {
                let notify = handler_started_clone.clone();
                async move {
                    notify.notify_one();
                    // Sleep forever so the event is never acked!
                    tokio::time::sleep(tokio::time::Duration::from_secs(100)).await;
                }
            })
            .await
            .unwrap();

        bus.publish(CriticalEvent {
            id: event_id,
            data: "important data".to_string(),
        })
        .await
        .unwrap();

        // Wait until the handler actually receives the event (which means it's written to DB and dispatched)
        handler_started.notified().await;

        // Crash! We drop the bus. The task is aborted. The event was NEVER acked.
        bus.shutdown().await.unwrap();
    }

    // Give some time for DB file handles to clear
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // PHASE 2: Restart the app, re-attach to the same DB file.
    {
        let bus = EventBus::builder()
            .with_redb_path(&db_path)
            .build()
            .await
            .unwrap();

        let received_count = Arc::new(AtomicUsize::new(0));
        let received_clone = received_count.clone();

        // Register the subscriber again
        let _sub = bus
            .subscribe(move |event: CriticalEvent| {
                let counter = received_clone.clone();
                async move {
                    assert_eq!(event.id, event_id);
                    assert_eq!(event.data, "important data");
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            })
            .await
            .unwrap();

        // Trigger crash recovery!
        bus.replay_pending().await.unwrap();

        // Wait for the replay to finish
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        assert_eq!(received_count.load(Ordering::Relaxed), 1);

        bus.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn test_redb_graceful_shutdown() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("events_graceful.redb");

    let bus = EventBus::builder()
        .with_redb_path(&db_path)
        .build()
        .await
        .unwrap();

    let processed_count = Arc::new(AtomicUsize::new(0));
    let count_clone = processed_count.clone();

    let _sub = bus
        .subscribe(move |_: CriticalEvent| {
            let counter = count_clone.clone();
            async move {
                // Sleep to ensure queue fills and is forced to drain during shutdown
                tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
                counter.fetch_add(1, Ordering::Relaxed);
            }
        })
        .await
        .unwrap();

    for i in 0..10 {
        bus.publish(CriticalEvent {
            id: Uuid::new_v4(),
            data: format!("data {}", i),
        })
        .await
        .unwrap();
    }

    // Shut down gracefully. It must wait until all 10 events are processed.
    bus.shutdown_gracefully().await.unwrap();

    assert_eq!(processed_count.load(Ordering::Relaxed), 10);
}

#[tokio::test]
async fn test_redb_concurrent_workers() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("events_concurrent.redb");

    let mut config = tokio_events::bus::config::EventBusConfig::default();
    config = config.dispatcher_config(|d| d.worker_threads(4));

    let bus = EventBus::builder()
        .config(config)
        .with_redb_path(&db_path)
        .build()
        .await
        .unwrap();

    let processed_count = Arc::new(AtomicUsize::new(0));
    let count_clone = processed_count.clone();

    // Handler 1
    let _sub1 = bus
        .subscribe(move |_: CriticalEvent| {
            let counter = count_clone.clone();
            async move {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        })
        .await
        .unwrap();

    let count_clone2 = processed_count.clone();
    
    // Handler 2
    let _sub2 = bus
        .subscribe(move |_: CriticalEvent| {
            let counter = count_clone2.clone();
            async move {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        })
        .await
        .unwrap();

    // 100 events * 2 handlers = 200 processings
    for _ in 0..100 {
        bus.publish(CriticalEvent {
            id: Uuid::new_v4(),
            data: "concurrent".into(),
        })
        .await
        .unwrap();
    }

    bus.shutdown_gracefully().await.unwrap();

    assert_eq!(processed_count.load(Ordering::Relaxed), 200);
}
