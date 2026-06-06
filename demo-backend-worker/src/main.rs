use demo_shared::{GetProfileRequest, GetProfileResponse, OrderCreated, SendSurveyEmail};
use rand::Rng;
use tokio_events::bus::builder::EventBusBuilder;
use tokio_events::event::EventMetadata;
use tokio_events::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    tracing::info!("⚙️ Starting Backend Worker Service...");

    // 2. Setup the Event Bus and connect to NATS
    let nats_url =
        std::env::var("DEMO_NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
    tracing::info!("Connecting to NATS at {}...", nats_url);

    let bus = match EventBusBuilder::new()
        .with_redb_path("demo_events.redb")
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

    tracing::info!("✅ Connected to NATS! Listening for requests and events.");

    // Replay any events that were saved to disk but crashed before processing!
    if let Err(e) = bus.replay_pending().await {
        tracing::error!("Failed to replay pending events from disk: {}", e);
    } else {
        tracing::info!("🔄 Replayed pending events from disk successfully.");
    }

    // -----------------------------------------------------------------------------
    // FEATURE: Remote Pub/Sub Round-Trip (Replaces Local RPC)
    // -----------------------------------------------------------------------------
    let bus_clone = bus.clone();
    let _profile_handle = bus
        .subscribe_remote("profile_fetcher_group", move |req: GetProfileRequest| {
            let bus = bus_clone.clone();
            async move {
                tracing::info!(
                    "📥 [Network Request Received] Fetching profile for User ID: {}",
                    req.user_id
                );

                // Simulate DB lookup
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

                let res = GetProfileResponse {
                    name: format!("Alice_{}", req.user_id),
                    is_vip: req.user_id.is_multiple_of(2),
                    error_msg: None,
                };

                // Send the response back across the network
                match bus.publish_remote(res).await {
                    Ok(id) => tracing::info!("   -> [Network Response Sent] Event ID: {}", id),
                    Err(e) => tracing::error!("   ❌ [Network Response Failed]: {}", e),
                }
            }
        })
        .await?;

    // -----------------------------------------------------------------------------
    // FEATURE: Wildcard Routing & Scheduled Events
    // -----------------------------------------------------------------------------

    // FEATURE: Filtered Handlers & Handler Structs
    let alert_handler = tokio_events::subscription::handler::FunctionHandler::with_name(
        |event: OrderCreated| async move {
            tracing::warn!(
                "🚨 [VIP ALERT] High-value order detected: ${} by User {}",
                event.amount,
                event.user_id
            );
        },
        "VIP_Alert_Handler",
    );
    let filtered_alert = tokio_events::subscription::handler::FilteredHandler::new(
        alert_handler,
        |envelope| {
            envelope
                .get_event::<OrderCreated>()
                .map(|e| e.amount > 250.0)
                .unwrap_or(false)
        },
        "AmountFilter",
    );
    let _vip_alert = bus
        .subscribe_handler::<OrderCreated, _>(filtered_alert)
        .await
        .unwrap();

    // Listen to ALL regions using the `*` wildcard!
    let bus_clone = bus.clone();
    let _order_handle = bus
        .subscribe_remote("order_processor_group", move |event: OrderCreated| {
            let bus = bus_clone.clone();
            async move {
                tracing::info!(
                    "📦 [Order Received] Processing Order {} for User {} (${})",
                    event.order_id,
                    event.user_id,
                    event.amount
                );

                // Simulate processing
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                tracing::info!("✅ [Order Processed] {}", event.order_id);

                // FEATURE: Scheduled Event
                // Let's schedule a "send email" event for 5 seconds in the future
                tracing::info!("   -> Scheduling survey email for 5 seconds from now...");
                let email_event = SendSurveyEmail {
                    order_id: event.order_id.clone(),
                    user_id: event.user_id,
                };

                let mut meta = EventMetadata::new();
                meta.deliver_at = Some(chrono::Utc::now() + chrono::Duration::seconds(5));
                let _ = bus.publish_with_metadata(email_event, meta).await;
            }
        })
        .await?;

    // -----------------------------------------------------------------------------
    // FEATURE: Fallible Handler & Retry/DLQ
    // -----------------------------------------------------------------------------
    // Subscribe to the scheduled email event we just published above
    let _email_handle = bus
        .subscribe_fallible(|event: SendSurveyEmail| async move {
            tracing::info!(
                "📧 [Email Service] Attempting to send survey email for Order {}...",
                event.order_id
            );

            // Randomly fail to simulate network issues
            let is_flaky = {
                let mut rng = rand::thread_rng();
                rng.gen_bool(0.70) // 70% chance to fail!
            };

            if is_flaky {
                tracing::warn!(
                    "   ❌ [Email Service] Network timeout! Returning Err to trigger retry..."
                );
                return Err(tokio_events::Error::internal(
                    "Simulated SMTP server timeout",
                ));
            }

            tracing::info!(
                "   ✅ [Email Service] Survey email successfully sent to User {}!",
                event.user_id
            );
            Ok(())
        })
        .await?;

    tracing::info!("👂 Backend is running and waiting for events from the Gateway...");
    tracing::info!("   Press Ctrl+C to gracefully shutdown.");

    // Keep the service alive
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!("Failed to listen for ctrl_c: {}", e);
    }

    tracing::info!("🛑 Initiating Graceful Shutdown...");
    bus.shutdown_gracefully().await?;
    Ok(())
}
