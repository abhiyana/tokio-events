use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::time::{sleep, Duration};
use tokio_events::prelude::*;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Event, Serialize, Deserialize)]
struct OrderEvent {
    id: u64,
}

#[tokio::test]
async fn test_topic_based_routing() -> Result<()> {
    let bus = EventBusBuilder::new().build().await?;

    let us_orders_count = Arc::new(AtomicUsize::new(0));
    let us_orders_count_clone = us_orders_count.clone();

    let eu_orders_count = Arc::new(AtomicUsize::new(0));
    let eu_orders_count_clone = eu_orders_count.clone();

    let all_orders_count = Arc::new(AtomicUsize::new(0));
    let all_orders_count_clone = all_orders_count.clone();

    // Subscribe to specific region with exact match
    let _h1 = bus
        .subscribe_topic("orders.us.created", move |_evt: OrderEvent| {
            let count = us_orders_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await?;

    // Subscribe to any region with single-token wildcard
    let _h2 = bus
        .subscribe_topic("orders.*.created", move |_evt: OrderEvent| {
            let count = eu_orders_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await?;

    // Subscribe to everything under orders using trailing wildcard
    let _h3 = bus
        .subscribe_topic("orders.>", move |_evt: OrderEvent| {
            let count = all_orders_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await?;

    // 1. Exact match US
    bus.publish_to("orders.us.created", OrderEvent { id: 1 })
        .await?;

    // 2. Exact match EU (should hit * and > but not US)
    bus.publish_to("orders.eu.created", OrderEvent { id: 2 })
        .await?;

    // 3. Deeper path (should only hit >)
    bus.publish_to("orders.us.electronics.created", OrderEvent { id: 3 })
        .await?;

    // Wait for dispatch
    sleep(Duration::from_millis(200)).await;

    assert_eq!(us_orders_count.load(Ordering::SeqCst), 1); // Only event 1
    assert_eq!(eu_orders_count.load(Ordering::SeqCst), 2); // Event 1 and 2
    assert_eq!(all_orders_count.load(Ordering::SeqCst), 3); // All 3 events

    Ok(())
}
