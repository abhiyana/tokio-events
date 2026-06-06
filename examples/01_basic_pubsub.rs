use tokio_events::prelude::*;

// Define an event with JSON serialization (default)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event)]
struct UserCreated {
    id: u64,
    email: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Build the in-memory Event Bus
    let bus = EventBusBuilder::new().build().await?;

    // 2. Subscribe to the event
    let _handle = bus
        .subscribe(|event: UserCreated| async move {
            println!("[Subscriber] Received Event: New user registered!");
            println!("   ID: {}", event.id);
            println!("   Email: {}", event.email);
        })
        .await?;

    println!("Publishing UserCreated event...");

    // 3. Publish the event
    bus.publish(UserCreated {
        id: 42,
        email: "alice@example.com".to_string(),
    })
    .await?;

    // Give the async handler a tiny bit of time to print before exiting
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Gracefully shut down the bus
    bus.shutdown_gracefully().await?;
    println!("Shutdown complete.");

    Ok(())
}
