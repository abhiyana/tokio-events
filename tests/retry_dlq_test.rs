use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_events::{Event, EventBus};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event)]
struct FlakyEvent {
    id: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event)]
struct PoisonEvent {
    id: usize,
}

#[tokio::test]
async fn test_handler_retries_and_succeeds() {
    // Configure bus with 3 max retries and a very fast backoff for tests
    let bus = EventBus::builder()
        .configure(|c| {
            c.max_retries(3)
                .retry_backoff(std::time::Duration::from_millis(10))
        })
        .build()
        .await
        .unwrap();

    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_clone = attempts.clone();

    // Subscribe a handler that fails twice, then succeeds on the 3rd try
    let _handle = bus.subscribe_fallible(move |event: FlakyEvent| {
        let attempts = attempts_clone.clone();
        async move {
            let current_attempt = attempts.fetch_add(1, Ordering::Relaxed) + 1;
            
            if current_attempt <= 2 {
                return Err(tokio_events::Error::internal(format!(
                    "Failing intentionally on attempt {}",
                    current_attempt
                )));
            }

            // Succeed on 3rd try
            assert_eq!(event.id, 42);
            Ok(())
        }
    })
    .await
    .unwrap();

    // Publish the event
    bus.publish(FlakyEvent { id: 42 }).await.unwrap();

    // Wait for the retries to happen
    let mut i = 0;
    while attempts.load(Ordering::Relaxed) < 3 && i < 500 {
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        i += 1;
    }

    // It should have been attempted exactly 3 times
    assert_eq!(attempts.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn test_handler_fails_and_routes_to_dlq() {
    // Configure bus with 2 max retries and a very fast backoff
    let bus = EventBus::builder()
        .configure(|c| {
            c.max_retries(2)
                .retry_backoff(std::time::Duration::from_millis(5))
        })
        .build()
        .await
        .unwrap();

    let mut dlq_rx = bus.take_dlq_receiver().await.expect("Should have DLQ receiver");

    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_clone = attempts.clone();

    // Subscribe a handler that ALWAYS fails
    let _handle = bus.subscribe_fallible(move |event: PoisonEvent| {
        let attempts = attempts_clone.clone();
        async move {
            attempts.fetch_add(1, Ordering::Relaxed);
            Err(tokio_events::Error::internal(format!("Poisoned event {}", event.id)))
        }
    })
    .await
    .unwrap();

    // Publish the event
    let event_id = bus.publish(PoisonEvent { id: 99 }).await.unwrap();

    // Wait for the DLQ to receive the event (use 5 seconds to prevent starvation in CI/parallel tests)
    let dlq_envelope = tokio::time::timeout(tokio::time::Duration::from_secs(5), dlq_rx.recv())
        .await
        .expect("Timeout waiting for DLQ")
        .expect("DLQ channel closed");

    // Max retries is 2, so it should try initial + 2 retries = 3 attempts total.
    // Wait, the logic in SubscriptionManager says `attempt > max_retries`. 
    // If max_retries is 2:
    // attempt 1 fails -> attempt = 1, backoff, retry
    // attempt 2 fails -> attempt = 2, backoff, retry
    // attempt 3 fails -> attempt = 3. 3 > 2, routes to DLQ!
    // So it attempts 3 times.
    assert_eq!(attempts.load(Ordering::Relaxed), 3);

    // Verify the envelope in DLQ is exactly the one we published
    assert_eq!(dlq_envelope.event_id(), event_id);
    assert_eq!(dlq_envelope.event_type(), "PoisonEvent");
    
    // Verify we can extract the original event data
    let original_event = dlq_envelope.get_event::<PoisonEvent>().unwrap();
    assert_eq!(original_event.id, 99);
}

#[tokio::test]
async fn test_partial_handler_failure() {
    let bus = tokio_events::bus::builder::EventBusBuilder::new()
        .configure(|c| c.max_retries(2).retry_backoff(std::time::Duration::from_millis(5)))
        .build()
        .await
        .unwrap();

    let mut dlq_rx = bus.take_dlq_receiver().await.unwrap();

    let success_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fail_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let succ_clone = success_count.clone();
    let _sub1 = bus
        .subscribe(move |_: PoisonEvent| {
            let c = succ_clone.clone();
            async move {
                c.fetch_add(1, Ordering::Relaxed);
            }
        })
        .await
        .unwrap();

    let fail_clone = fail_count.clone();
    let _sub2 = bus
        .subscribe_fallible(move |_: PoisonEvent| {
            let c = fail_clone.clone();
            async move {
                c.fetch_add(1, Ordering::Relaxed);
                Err(tokio_events::Error::ShuttingDown)
            }
        })
        .await
        .unwrap();

    bus.publish(PoisonEvent {
        id: 42,
    })
    .await
    .unwrap();

    // Wait for DLQ message from the failed handler
    let _dlq_msg = dlq_rx.recv().await.expect("Expected DLQ message");
    
    // The successful handler should have only processed it ONCE
    assert_eq!(success_count.load(Ordering::Relaxed), 1);
    
    // The failed handler should have processed it exactly 1 (initial) + 2 (retries) = 3 times
    assert_eq!(fail_count.load(Ordering::Relaxed), 3);
}
