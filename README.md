<div align="center">
  <h1>tokio-events</h1>
  <p><strong>A zero-lock, high-throughput, enterprise-grade event bus for Rust.</strong></p>
  
  [![Crates.io](https://img.shields.io/crates/v/tokio-events?style=flat-square)](https://crates.io/crates/tokio-events)
  [![Documentation](https://img.shields.io/docsrs/tokio-events?style=flat-square)](https://docs.rs/tokio-events)
  [![Build Status](https://img.shields.io/github/actions/workflow/status/abhiyana/tokio-events/rust.yml?branch=main&style=flat-square)](https://github.com/abhiyana/tokio-events/actions)
  [![Downloads](https://img.shields.io/crates/d/tokio-events?style=flat-square)](https://crates.io/crates/tokio-events)
  [![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square)](LICENSE)
</div>

---

`tokio-events` is a modern, type-safe, asynchronous event bus built natively on `tokio`. It scales seamlessly from a high-performance in-memory pub/sub channel in a monolith, all the way to a strictly-typed, distributed, persistent event architecture across microservices.

Go from zero to **2,000,000 events/sec** in 5 lines of code.

## The Elevator Pitch: Why not just use `mpsc` or `broadcast`?

`tokio::sync::broadcast` and `mpsc` channels are fantastic for simple thread-to-thread communication. But as your application grows into a complex monolith or transitions to microservices, raw channels fall short.

You need `tokio-events` if you require:
1. **Type-Safe Subscriptions**: No manual enum downcasting. Subscribe to a concrete struct and let the bus route it.
2. **Exactly-Once Delivery**: Channels drop messages if receivers lag or crash. `tokio-events` uses Redb to safely store unacknowledged events on disk and guarantees idempotency.
3. **Dead Letter Queues**: If a consumer panics 5 times, channels block or drop the message. We capture it in a DLQ.
4. **Subject-Based Routing**: Publish with metadata topics (`"orders.eu"`) to selectively trigger handlers.
5. **Zero-Lock Publishing**: Built on `arc-swap`, publishing is 100% lock-free, achieving massive throughput without CPU contention.

---

## Integration in 10 Seconds (Axum Example)

`tokio-events` is designed to be effortlessly injected into your existing web frameworks as application state.

```rust
use axum::{extract::State, routing::post, Json, Router};
use tokio_events::prelude::*;

// 1. Define your event
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event)]
struct UserSignup { email: String }

// 2. The HTTP Handler
async fn signup_handler(
    State(bus): State<EventBus>, 
    Json(payload): Json<UserSignup>
) {
    // Fire and forget: The background worker will reliably handle the email.
    bus.publish(payload).await.unwrap();
}

#[tokio::main]
async fn main() -> Result<()> {
    // 3. Initialize the bus and background worker
    let bus = EventBusBuilder::new().build().await?;
    
    bus.subscribe(|event: UserSignup| async move {
        println!("Background Task: Sending welcome email to {}", event.email);
    }).await?.detach();

    // 4. Inject into Axum
    let app = Router::new()
        .route("/signup", post(signup_handler))
        .with_state(bus);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
    Ok(())
}
```

---

## Quick Start (In-Memory JSON)

Add the dependency to your `Cargo.toml`:
```toml
[dependencies]
tokio-events = "0.4.0"
tokio = { version = "1.0", features = ["full"] }
```

Define an event, create the bus, and publish!

```rust
use tokio_events::prelude::*;

// 1. Define your Event (Defaults to JSON serialization)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event)]
struct UserCreated {
    id: u64,
    email: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // 2. Build the in-memory Event Bus
    let bus = EventBusBuilder::new().build().await?;

    // 3. Subscribe to the event
    let _handle = bus.subscribe(|event: UserCreated| async move {
        println!("New user registered! Sending email to: {}", event.email);
    }).await?;

    // 4. Publish the event (100% Lock-Free)
    bus.publish(UserCreated {
        id: 42,
        email: "alice@example.com".to_string(),
    }).await?;

    Ok(())
}
```

---

## Enterprise Reliability (Persistence, Idempotency, DLQ)

If your application crashes immediately after taking payment but before sending the `OrderConfirmed` event, you lose data. `tokio-events` solves this by natively integrating with `redb` (a pure-Rust embedded database) to implement the **Transactional Outbox Pattern**.

Enable the feature:
```toml
[dependencies]
tokio-events = { version = "0.4.0", features = ["persistence"] }
```

### 1. Exactly-Once Delivery
Use Idempotency Keys to prevent duplicate processing if upstream services accidentally double-publish:

```rust
let bus = EventBusBuilder::new().with_redb_path("events.db").build().await?;

let metadata = EventMetadata::new()
    .with_idempotency_key("order_12345_payment_captured");

// If this exact metadata is published twice, the second is instantly dropped
bus.publish_with_metadata(PaymentCaptured { id: 12345 }, metadata).await?;
```

### 2. Dead-Letter Queue (DLQ) Replay
If a handler fails repeatedly, the event is safely moved to the Dead Letter Queue (`DLQ_TABLE`) rather than being dropped. After you push a hotfix to your production code, you can replay all failed events with a single command:

```rust
// Rip all failed events out of the DLQ and process them again
let recovered_count = bus.replay_dlq().await?;
println!("Successfully recovered {} events", recovered_count);
```

---

## Distributed Network (NATS JetStream)

To route events across microservices, enable the `remote` feature, derive the `Remote` trait, and `tokio-events` will automatically route your events over NATS JetStream.

```mermaid
graph LR
    subgraph Microservice A
        Publisher[EventBus::publish_remote]
    end
    
    subgraph Network
        NATS((NATS JetStream))
    end
    
    subgraph Microservice B
        Consumer[EventBus::subscribe_remote]
    end
    
    Publisher -- "Serialize -> Bytes" --> NATS
    NATS -- "Bytes -> Deserialize" --> Consumer
```

Enable the feature:
```toml
[dependencies]
tokio-events = { version = "0.4.0", features = ["remote"] }
```

Define the routing topic and publish over the network:
```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event, Remote)]
#[remote(topic = "user.created.v1")] // NATS Topic
struct UserCreated {
    id: u64,
}

let bus = EventBusBuilder::new()
    // Connect to NATS JetStream stream "ENTERPRISE_EVENTS"
    .with_nats_jetstream("nats://localhost:4222", "ENTERPRISE_EVENTS", vec!["user.>".to_string()])
    .build()
    .await?;

// This publishes to local subscribers AND the NATS network
bus.publish_remote(UserCreated { id: 42 }).await?;
```

---

## Feature Flags

`tokio-events` uses feature flags to keep your binary size small. 

| Feature | Description | Dependencies |
|---------|-------------|--------------|
| `macros` | (Default) Enables the `#[derive(Event)]` macro. | `tokio-events-macros` |
| `persistence` | Enables embedded `redb` disk persistence for the Outbox Pattern. | `redb` |
| `remote` | Enables distributed network routing via `async-nats` JetStream. | `async-nats` |
| `protobuf` | Enables strict schema enforcement via `prost::Message`. | `prost` |
| `metrics` | Enables internal telemetry metrics. | `metrics` |

---

## Contributing

We welcome community contributions. Please feel free to submit a Pull Request or open an Issue.

**License:** MIT
