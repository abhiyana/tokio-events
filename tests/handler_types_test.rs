use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_events::{Event, EventBus};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BasicEvent {
    value: usize,
}

impl Event for BasicEvent {
    fn event_type() -> &'static str {
        "BasicEvent"
    }
}

struct CustomStructHandler {
    counter: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl tokio_events::EventHandler for CustomStructHandler {
    async fn handle(&self, envelope: &tokio_events::EventEnvelope) -> tokio_events::Result<()> {
        println!("CustomStructHandler handle called for event_id: {}", envelope.event_id());
        match envelope.get_event::<BasicEvent>() {
            Ok(event) => {
                self.counter.fetch_add(event.value, Ordering::Relaxed);
            }
            Err(e) => {
                println!("Error getting event: {:?}", e);
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn test_custom_struct_handler() {
    let bus = EventBus::builder().build().await.unwrap();
    let counter = Arc::new(AtomicUsize::new(0));

    let handler = CustomStructHandler {
        counter: counter.clone(),
    };

    let _handle = bus.subscribe_handler::<BasicEvent, _>(handler)
        .await
        .unwrap();

    bus.publish(BasicEvent { value: 5 }).await.unwrap();
    bus.publish(BasicEvent { value: 10 }).await.unwrap();

    // Give it time to dispatch into the handler's internal queue
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    bus.shutdown_gracefully().await.unwrap();

    assert_eq!(counter.load(Ordering::Relaxed), 15);
}

#[tokio::test]
async fn test_unsubscribe_behavior() {
    let bus = EventBus::builder().build().await.unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let handle = bus
        .subscribe(move |event: BasicEvent| {
            let c = counter_clone.clone();
            async move {
                c.fetch_add(event.value, Ordering::Relaxed);
            }
        })
        .await
        .unwrap();

    bus.publish(BasicEvent { value: 10 }).await.unwrap();
    
    // Give it time to process
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    assert_eq!(counter.load(Ordering::Relaxed), 10);

    // Unsubscribe!
    let _ = handle.unsubscribe().await;

    // Publish again. This should NOT be processed.
    bus.publish(BasicEvent { value: 100 }).await.unwrap();

    bus.shutdown_gracefully().await.unwrap();

    // Still 10!
    assert_eq!(counter.load(Ordering::Relaxed), 10);
}
