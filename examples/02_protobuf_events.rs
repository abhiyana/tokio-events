//! To run this example, you must enable the `protobuf` feature:
//! `cargo run --example 02_protobuf_events --features "protobuf"`

#[cfg(feature = "protobuf")]
use tokio_events::prelude::*;

#[cfg(feature = "protobuf")]
#[derive(Clone, PartialEq, prost::Message, Event)]
#[event(format = "protobuf")] // This tells the macro to use strict Protobuf serialization!
struct UserUpdated {
    #[prost(uint64, tag = "1")]
    pub id: u64,
    #[prost(string, tag = "2")]
    pub email: String,
}

#[cfg(feature = "protobuf")]
#[tokio::main]
async fn main() -> Result<()> {
    let bus = EventBusBuilder::new().build().await?;

    let _handle = bus
        .subscribe(|event: UserUpdated| async move {
            println!("[Subscriber] Received Protobuf Event: User updated!");
            println!("   ID: {}", event.id);
            println!("   Email: {}", event.email);
        })
        .await?;

    println!("Publishing UserUpdated protobuf event...");

    // The EventBus natively handles the Protobuf serialization behind the scenes
    bus.publish(UserUpdated {
        id: 42,
        email: "alice_v2@example.com".to_string(),
    })
    .await?;

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    bus.shutdown_gracefully().await?;

    Ok(())
}

#[cfg(not(feature = "protobuf"))]
fn main() {
    println!("Please run this example with the protobuf feature enabled:");
    println!("cargo run --example 02_protobuf_events --features \"protobuf\"");
}
