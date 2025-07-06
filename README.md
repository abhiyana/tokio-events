# tokio-events

A modern, type-safe async event bus for Rust applications built on Tokio.

![image](https://github.com/user-attachments/assets/0abac6a2-0e3b-48b9-bad9-00ba9010400b)

## Quick Example

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

