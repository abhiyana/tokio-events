use tokio_events::{Event, EventBus};

#[derive(Debug, Clone)]
struct MyEvent {
    message: String,
}

impl Event for MyEvent {
    fn event_type() -> &'static str {
        "MyEvent"
    }
}

#[tokio::main]
async fn main() {
    println!("Testing tokio-events...\n");

    // Create event bus
    let bus = EventBus::builder().build().await.unwrap();

    // Subscribe to events
    let handle = bus
        .subscribe(|event: MyEvent| async move {
            println!("📨 Received: {}", event.message);
        })
        .await
        .unwrap();

    let handle2 = bus
        .subscribe(|event: MyEvent| async move {
            println!("📨 Received in second handler: {}", event.message);
        })
        .await
        .unwrap();

    // Publish some events
    println!("Publishing events...");
    bus.emit_and_wait(MyEvent {
        message: "Hello!".into(),
    })
    .await
    .unwrap();
    bus.emit_and_wait(MyEvent {
        message: "World!".into(),
    })
    .await
    .unwrap();
    bus.emit_and_wait(MyEvent {
        message: "Event Bus is working!".into(),
    })
    .await
    .unwrap();

    // Wait a bit for events to process
    // tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Clean up
    bus.unsubscribe(handle).await.unwrap();
    bus.unsubscribe(handle2).await.unwrap();
    bus.shutdown().await.unwrap();

    println!("\n✅ Test completed successfully!");
}
