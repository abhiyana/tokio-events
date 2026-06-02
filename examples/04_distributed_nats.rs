//! To run this example, you must enable the `remote` feature and have a NATS server running locally:
//! `cargo run --example 04_distributed_nats --features "remote"`

#[cfg(feature = "remote")]
use tokio_events::prelude::*;

#[cfg(feature = "remote")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event, Remote)]
#[remote(topic = "system.metrics.latency")]
struct LatencyMetric {
    endpoint: String,
    duration_ms: u64,
}

#[cfg(feature = "remote")]
#[tokio::main]
async fn main() -> Result<()> {
    // 1. Initialize the EventBus connected to NATS JetStream
    println!("Connecting to NATS at nats://localhost:4222...");
    
    // Note: If NATS is not running, this will fall back to local-only routing gracefully.
    let bus = EventBusBuilder::new()
        .with_nats_jetstream(
            "nats://localhost:4222", 
            "METRICS_STREAM", 
            vec!["metrics.>".to_string()]
        )
        .build()
        .await?;

    // 2. Subscribe to remote events. 
    // The "queue_group" ensures that if we ran 5 instances of this app, 
    // only ONE instance processes each event (load balancing).
    let _handle = bus
        .subscribe_remote("metrics_processor_group", |event: LatencyMetric| async move {
            println!("[Remote Consumer] Received latency metric!");
            println!("   Endpoint: {}", event.endpoint);
            println!("   Duration: {}ms", event.duration_ms);
        })
        .await?;

    println!("Publishing LatencyMetric to the network...");
    
    // 3. Publish the event over the network
    bus.publish_remote(LatencyMetric {
        endpoint: "/api/v1/users".to_string(),
        duration_ms: 142,
    })
    .await?;

    // Wait for the message to round-trip through NATS
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    bus.shutdown_gracefully().await?;

    Ok(())
}

#[cfg(not(feature = "remote"))]
fn main() {
    println!("Please run this example with the remote feature enabled:");
    println!("cargo run --example 04_distributed_nats --features \"remote\"");
}
