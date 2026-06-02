use std::time::Duration;
use tokio_events::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event)]
struct TestDelayedEvent {
    id: usize,
}

#[tokio::test]
async fn test_in_memory_delayed_events() -> Result<()> {
    let bus = EventBusBuilder::new().build().await?;
    let counter = Arc::new(AtomicUsize::new(0));

    let counter_clone = counter.clone();
    let _handle = bus.subscribe(move |event: TestDelayedEvent| {
        let c = counter_clone.clone();
        async move {
            c.fetch_add(event.id, Ordering::SeqCst);
        }
    }).await?;

    // Schedule for 100ms in the future
    bus.publish_delayed(TestDelayedEvent { id: 5 }, Duration::from_millis(100)).await?;
    
    // Check instantly - should be 0
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    
    // Wait 150ms
    tokio::time::sleep(Duration::from_millis(150)).await;
    
    // Should be 5
    assert_eq!(counter.load(Ordering::SeqCst), 5);

    bus.shutdown_gracefully().await?;
    Ok(())
}
