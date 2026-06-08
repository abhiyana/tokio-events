#![cfg(feature = "persistence")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_events::{Event, EventBus};
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event)]
struct CriticalEvent {
    id: Uuid,
    data: String,
}

#[tokio::test]
async fn test_redb_crash_recovery() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("events.redb");

    let event_id = Uuid::new_v4();

    // PHASE 1: Publish a delayed event, then CRASH the server before it fires.
    {
        let bus = EventBus::builder()
            .with_redb_path(&db_path)
            .build()
            .await
            .unwrap();

        // WE MUST SUBSCRIBE in Phase 1 so that the RedbDispatcher sees sub_count > 0
        // Otherwise, it skips persisting the event!
        let _sub1 = bus.subscribe(|_: CriticalEvent| async {}).await.unwrap();

        // Publish with 5 seconds delay so it definitely doesn't fire while we are reconnecting
        bus.publish_delayed(
            CriticalEvent {
                id: event_id,
                data: "important data".to_string(),
            },
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap();

        // Wait a tiny bit to ensure it wrote to disk
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // CRASH! Drop the bus, aborting the scheduler.
        bus.shutdown().await.unwrap();
    }

    // Give some time for DB file handles to clear
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // PHASE 2: Restart the app. The background scheduler should automatically recover and fire the event!
    {
        let config = tokio_events::bus::config::EventBusConfig {
            scheduler_tick_rate: std::time::Duration::from_secs(1),
            ..Default::default()
        };

        let bus = EventBus::builder()
            .with_config(config)
            .with_redb_path(&db_path)
            .build()
            .await
            .unwrap();

        let received_count = Arc::new(AtomicUsize::new(0));
        let received_clone = received_count.clone();

        // Register the subscriber
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

        // Wait 6 seconds for the scheduled event to fire (5s original delay + margin)
        tokio::time::sleep(tokio::time::Duration::from_secs(6)).await;

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
        .with_config(config)
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

#[tokio::test]
async fn test_redb_idempotency() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("events_idempotency.redb");

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
                counter.fetch_add(1, Ordering::Relaxed);
            }
        })
        .await
        .unwrap();

    // Publish event with IDEMPOTENCY KEY
    let metadata = tokio_events::event::EventMetadata::new().with_idempotency_key("order_123");
    bus.publish_with_metadata(
        CriticalEvent {
            id: Uuid::new_v4(),
            data: "payment".into(),
        },
        metadata.clone(),
    )
    .await
    .unwrap();

    // Publish exactly the SAME metadata
    bus.publish_with_metadata(
        CriticalEvent {
            id: Uuid::new_v4(),
            data: "payment".into(),
        },
        metadata.clone(),
    )
    .await
    .unwrap();

    bus.shutdown_gracefully().await.unwrap();

    // Only one should be processed!
    assert_eq!(processed_count.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn test_redb_dlq_replay() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("events_dlq.redb");

    let mut config = tokio_events::bus::config::EventBusConfig::default();
    config = config.max_retries(0); // Fail instantly on first error

    let bus = EventBus::builder()
        .with_config(config)
        .with_redb_path(&db_path)
        .build()
        .await
        .unwrap();

    let should_fail = Arc::new(tokio::sync::Mutex::new(true));
    let should_fail_clone = should_fail.clone();

    let processed_count = Arc::new(AtomicUsize::new(0));
    let count_clone = processed_count.clone();

    let _sub = bus
        .subscribe_fallible(move |_: CriticalEvent| {
            let counter = count_clone.clone();
            let fail = should_fail_clone.clone();
            async move {
                let is_fail = *fail.lock().await;
                if is_fail {
                    Err(tokio_events::Error::HandlerError(
                        "Simulated permanent failure".into(),
                    ))
                } else {
                    counter.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
            }
        })
        .await
        .unwrap();

    // Publish an event that will permanently fail
    bus.publish(CriticalEvent {
        id: Uuid::new_v4(),
        data: "fail".into(),
    })
    .await
    .unwrap();

    // Give it time to fail and move to DLQ
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Verify it failed
    assert_eq!(processed_count.load(Ordering::Relaxed), 0);

    // Fix the bug!
    *should_fail.lock().await = false;

    // Replay the DLQ
    let replayed = bus.replay_dlq().await.unwrap();
    assert_eq!(replayed, 1);

    bus.shutdown_gracefully().await.unwrap();

    // Verify it succeeded on the replay!
    assert_eq!(processed_count.load(Ordering::Relaxed), 1);
}
