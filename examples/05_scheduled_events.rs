use chrono::Utc;
use std::time::Duration;
use tokio_events::prelude::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event)]
struct ReminderEmail {
    user_id: u64,
    template: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // We are using the in-memory bus for this example, which uses Tokio's internal Timing Wheel
    // for millisecond precision scheduling!
    // (If you enable the persistence feature, this will automatically use the Poller architecture for 100% crash resilience).
    let bus = EventBusBuilder::new().build().await?;

    let _handle = bus
        .subscribe(|event: ReminderEmail| async move {
            println!(
                "🕒 [Subscriber] Event Fired! Sending '{}' email to User {}",
                event.template, event.user_id
            );
        })
        .await?;

    println!("1. Publishing a delayed event (Fires in 3 seconds)...");
    bus.publish_delayed(
        ReminderEmail {
            user_id: 42,
            template: "abandoned_cart".to_string(),
        },
        Duration::from_secs(3),
    )
    .await?;

    println!("2. Publishing a scheduled event (Fires at an exact timestamp in 1 second)...");
    let future_time = Utc::now() + Duration::from_secs(1);
    bus.publish_scheduled(
        ReminderEmail {
            user_id: 99,
            template: "welcome_series_2".to_string(),
        },
        future_time,
    )
    .await?;

    println!("Events published! Waiting for them to fire...\n");

    // Keep the main thread alive to see the events fire
    tokio::time::sleep(Duration::from_secs(4)).await;

    println!("\nAll scheduled events have fired!");
    bus.shutdown_gracefully().await?;

    Ok(())
}
