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

        // Wait for the replayed event to be processed
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        assert_eq!(
            received_count.load(Ordering::Relaxed),
            1,
            "Event should have been replayed once"
        );

        bus.shutdown().await.unwrap();
    }
}
