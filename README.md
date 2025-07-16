# tokio-events

A modern, type-safe async event bus for Rust applications built on Tokio.

![image](https://github.com/abhiyana/tokio-events/blob/main/docs/eventhighlevel.png)

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
```

# Core Components

## 1. EventBus – The Main API
- Single instance shared across your application  
- Provides `publish()` and `subscribe()` methods  
- Manages component lifecycle  

## 2. Registry – Type Mapping
- Maps event types to their subscribers  
- Uses `TypeId` for type-safe routing  
- Thread-safe using `DashMap`  

## 3. Dispatcher – Event Queue
- MPSC channel for queuing events  
- Worker thread processes events  
- Configurable queue size and backpressure  

## 4. Subscription Manager – Handler Execution
- Stores active handlers  
- Spawns async tasks for parallel execution  
- Manages subscription lifecycle  



![image](https://github.com/abhiyana/tokio-events/blob/main/docs/eventflow.png)
