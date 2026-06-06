use demo_shared::{GetProfileRequest, GetProfileResponse, OrderCreated};
use rand::Rng;
use tokio_events::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    tracing::info!("🌐 Starting API Gateway...");

    // 2. Setup the Event Bus and connect to NATS
    let nats_url =
        std::env::var("DEMO_NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
    tracing::info!("Connecting to NATS at {}...", nats_url);

    let bus = match EventBusBuilder::new()
        .with_nats_jetstream(&nats_url, "ECOMMERCE_V2", vec!["v2.>".to_string()])
        .build()
        .await
    {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(
                "Failed to connect to NATS. Please start NATS (docker run -p 4222:4222 nats): {}",
                e
            );
            return Ok(());
        }
    };

    tracing::info!("✅ Connected to NATS! Ready to simulate API traffic.");

    // Setup a listener for the network responses
    let bus_clone = bus.clone();
    let _response_handle = bus
        .subscribe_remote(
            "gateway_response_group",
            move |profile: GetProfileResponse| {
                let bus = bus_clone.clone();
                async move {
                    tracing::info!(
                        "   <- NETWORK SUCCESS: Fetched Profile [Name: {}, VIP: {}]",
                        profile.name,
                        profile.is_vip
                    );

                    // Now simulate the user placing an order
                    let amount = rand::thread_rng().gen_range(10.0..500.0);

                    // Route to different regions based on random chance
                    let region = if rand::thread_rng().gen_bool(0.5) {
                        "us"
                    } else {
                        "eu"
                    };
                    let topic = format!("v2.orders.{}.created", region);

                    tracing::info!("   -> Publishing Order to topic '{}'", topic);
                    // In a real app we'd parse the user ID from the response name for simplicity we just generate one
                    let _ = bus
                        .publish_remote(OrderCreated {
                            order_id: format!(
                                "ORD-{}",
                                uuid::Uuid::new_v4()
                                    .to_string()
                                    .chars()
                                    .take(8)
                                    .collect::<String>()
                            ),
                            user_id: 42,
                            amount,
                        })
                        .await;
                    println!("--------------------------------------------------");
                }
            },
        )
        .await?;

    // Simulate 5 incoming user requests, one every 2 seconds
    for i in 1..=5 {
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let user_id = rand::thread_rng().gen_range(100..999);
        tracing::info!(
            "📞 [Req {}] Incoming API Request for User ID: {}",
            i,
            user_id
        );

        // Send request over the network!
        tracing::info!("   -> Sending Network Request over NATS...");
        let _ = bus.publish_remote(GetProfileRequest { user_id }).await;
    }

    // Wait for the asynchronous network responses to arrive before shutting down
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    tracing::info!("🛑 Simulation complete. Shutting down gateway...");
    bus.shutdown_gracefully().await?;
    Ok(())
}
