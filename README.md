# tokio-events

A modern, type-safe async event bus for Rust applications built on Tokio.

## 🚧 Work in Progress

This library is currently under active development. Star/watch the repo for updates!

## Features (Planned)

- 🔒 **Type-safe** event publishing and subscription
- ⚡ **High performance** with zero-copy message passing
- 🔄 **Async-first** design built on Tokio
- 📊 **Built-in metrics** and observability
- 🛡️ **Comprehensive error handling**
- 🧩 **Modular architecture** with optional features

## Quick Example (Coming Soon)

```rust
// Create an event bus
let bus = EventBus::builder()
    .with_metrics()
    .build();

// Subscribe to events
bus.subscribe(|event: UserRegistered| async {
    println!("New user: {}", event.email);
}).await?;

// Publish events
bus.publish(UserRegistered {
    id: 123,
    email: "user@example.com".into(),
}).await?;