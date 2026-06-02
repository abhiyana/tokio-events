//! To run this example, you must enable the `persistence` feature:
//! `cargo run --example 03_persistent_queue --features "persistence"`

use tokio_events::prelude::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event)]
struct OrderConfirmed {
    order_id: u64,
    total_cents: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    // 1. We create a Redb database on disk to act as our Outbox.
    // In a real app, this would be a persistent path (e.g. "/var/lib/events.db").
    // For this example, we use a temp file.
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("events.redb");

    // 2. We initialize the EventBus with Disk Persistence enabled.
    let bus = EventBusBuilder::new()
        .with_redb_path(&db_path)
        .build()
        .await?;

    // 3. Subscribe to the event.
    let _handle = bus
        .subscribe(|event: OrderConfirmed| async move {
            println!("[Subscriber] Processing Order #{}: {} cents", event.order_id, event.total_cents);
            
            // Simulating a slow database update...
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            
            println!("[Subscriber] Done! The event will now be deleted from disk.");
        })
        .await?;

    // 4. Publish an event.
    // The EventBus writes the serialized event to `events.redb` FIRST, before any subscribers receive it. 
    // If the server crashes right here, the event is safely stored in redb. When the server boots back up,
    // the EventBus will read it from disk and automatically replay it!
    println!("[Publisher] Emitting OrderConfirmed event to disk and subscribers...");
    bus.publish(OrderConfirmed {
        order_id: 12345,
        total_cents: 9900,
    })
    .await?;

    // Wait for the subscriber to process the event
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    // Gracefully shutdown
    bus.shutdown_gracefully().await?;
    println!("Graceful shutdown complete.");

    Ok(())
}
