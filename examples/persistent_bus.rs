use tokio_events::{Event, EventBus};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ImportantEvent {
    id: u64,
    data: String,
}

impl Event for ImportantEvent {
    fn event_type() -> &'static str {
        "ImportantEvent"
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Enable tracing for visibility
    // tracing_subscriber::fmt::init(); // Uncomment if you add tracing_subscriber to dependencies

    // 1. We start by creating a redb Database on disk.
    // In a real app, this might be saved to a specific persistent path.
    // Here we create it in a temp directory for the example.
    let temp_dir = tempfile::tempdir()?;
    let db_path = temp_dir.path().join("events.redb");

    // 2. We use the `with_redb_path` helper which is available when the "persistence" feature is enabled.
    // It automatically creates the redb database and sets up the RedbDispatcher!
    let bus = EventBus::builder().with_redb_path(&db_path).build().await?;

    // 3. Subscribe to our ImportantEvent
    let handle = bus
        .subscribe(|event: ImportantEvent| async move {
            println!("Subscriber processed event: {:?}", event);
            // Once this function completes, the RedbRegistry will receive an ack_event
            // and safely remove the event from the redb store.
        })
        .await?;

    // 4. Publish an event.
    // Because we use RedbDispatcher, it writes the serialized event to `events.redb`
    // FIRST, before any subscribers receive it. If the app crashes right here,
    // the event is safely stored in redb.
    println!("Publishing ImportantEvent...");
    bus.publish(ImportantEvent {
        id: 42,
        data: "Highly critical data!".to_string(),
    })
    .await?;

    // Wait for the subscriber to process the event
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Cleanup
    bus.unsubscribe(handle).await?;
    bus.shutdown().await?;

    Ok(())
}
