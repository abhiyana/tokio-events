# tokio-events Usage Guide

## Table of Contents
1. [Getting Started](#getting-started)
2. [Core Concepts](#core-concepts)
3. [API Reference](#api-reference)
4. [Common Patterns](#common-patterns)
5. [Best Practices](#best-practices)
6. [Troubleshooting](#troubleshooting)

## Getting Started

### Installation

Add to your `Cargo.toml`:
```toml
[dependencies]
tokio-events = "0.1.0"
tokio = { version = "1", features = ["full"] }
```

### Basic Example

```rust
use tokio_events::{Event, EventBus};

// 1. Define your event
#[derive(Debug, Clone)]
struct UserRegistered {
    user_id: u64,
    email: String,
}

// 2. Implement the Event trait
impl Event for UserRegistered {
    fn event_type() -> &'static str {
        "UserRegistered"
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 3. Create the event bus
    let bus = EventBus::builder().build().await?;
    
    // 4. Subscribe to events
    let handle = bus.subscribe(|event: UserRegistered| async move {
        println!("New user: {} with id {}", event.email, event.user_id);
    }).await?;
    
    // 5. Publish events
    bus.publish(UserRegistered {
        user_id: 123,
        email: "user@example.com".to_string(),
    }).await?;
    
    // Wait for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    // 6. Cleanup
    bus.unsubscribe(handle).await?;
    bus.shutdown().await?;
    
    Ok(())
}
```

## Core Concepts

### Events
Events are simple data structures that implement the `Event` trait:

```rust
use tokio_events::Event;

#[derive(Debug, Clone)]
struct OrderPlaced {
    order_id: u64,
    customer_id: u64,
    total_amount: f64,
}

impl Event for OrderPlaced {
    fn event_type() -> &'static str {
        "OrderPlaced"  // Unique identifier for this event type
    }
}
```

### Event Bus
The central hub that manages all event publishing and subscriptions. You typically create **one instance** per application.

### Publishers
Any code that sends events through the bus.

### Subscribers
Functions or handlers that react to specific event types.

## API Reference

### EventBus Creation

#### `EventBus::builder() -> EventBusBuilder`
Creates a new builder for configuring the EventBus.

```rust
let bus = EventBus::builder()
    .build()
    .await?;
```

#### `EventBusBuilder::high_throughput() -> Self`
Configures the bus for high-throughput scenarios.

```rust
let bus = EventBus::builder()
    .high_throughput()  // 50k queue size, multiple workers
    .build()
    .await?;
```

**When to use**: High-volume event processing, real-time systems.

#### `EventBusBuilder::reliable() -> Self`
Configures for reliable message processing.

```rust
let bus = EventBus::builder()
    .reliable()  // Retries, no dropping, longer timeouts
    .build()
    .await?;
```

**When to use**: Critical business events, financial transactions.

#### `EventBusBuilder::ordered() -> Self`
Ensures events are processed in order.

```rust
let bus = EventBus::builder()
    .ordered()  // Single worker thread
    .build()
    .await?;
```

**When to use**: When event order matters (e.g., audit logs).

#### `EventBusBuilder::configure() -> Self`
Custom configuration with a closure.

```rust
let bus = EventBus::builder()
    .configure(|config| {
        config
            .max_retries(5)
            .enable_deduplication(true)
            .dispatcher_config(|d| {
                d.max_queue_size(100_000)
                 .worker_threads(8)
                 .processing_timeout_ms(10_000)
            })
    })
    .build()
    .await?;
```

### Publishing Events

#### `publish<T: Event>(&self, event: T) -> Result<Uuid>`
Publishes an event and returns its unique ID.

```rust
// Simple publish
let event_id = bus.publish(UserRegistered {
    user_id: 123,
    email: "user@example.com".to_string(),
}).await?;

println!("Published event: {}", event_id);
```

#### `publish_with_metadata<T: Event>(&self, event: T, metadata: EventMetadata) -> Result<Uuid>`
Publishes an event with custom metadata.

```rust
use tokio_events::{EventMetadata};
use uuid::Uuid;

// Create metadata with correlation ID
let correlation_id = Uuid::new_v4();
let metadata = EventMetadata::new()
    .set_correlation_id(correlation_id)
    .set_source("user-service")
    .set_user_id("user-123");

let event_id = bus.publish_with_metadata(
    OrderPlaced {
        order_id: 456,
        customer_id: 123,
        total_amount: 99.99,
    },
    metadata
).await?;
```

**Use cases**:
- Tracing related events across services
- Adding context like user ID or session ID
- Event sourcing with causation chains

### Subscribing to Events

#### `subscribe<T, F, Fut>(&self, handler: F) -> Result<SubscriptionHandle>`
Subscribes a function to handle events of type T.

```rust
// Basic subscription
let handle = bus.subscribe(|event: OrderPlaced| async move {
    println!("Order {} placed for ${}", event.order_id, event.total_amount);
}).await?;

// With captured variables
let db_pool = get_db_pool();
let handle = bus.subscribe(move |event: OrderPlaced| {
    let db = db_pool.clone();
    async move {
        // Save to database
        save_order_to_db(&db, event).await;
    }
}).await?;

// Multiple subscribers for same event
let email_handle = bus.subscribe(|event: OrderPlaced| async move {
    send_order_confirmation_email(event).await;
}).await?;

let inventory_handle = bus.subscribe(|event: OrderPlaced| async move {
    update_inventory(event).await;
}).await?;
```

#### `subscribe_handler<T, H>(&self, handler: H) -> Result<SubscriptionHandle>`
Subscribes a custom handler that implements `EventHandler`.

```rust
use tokio_events::{EventHandler, EventEnvelope};
use async_trait::async_trait;

// Custom handler with state
struct EmailHandler {
    smtp_client: SmtpClient,
    template_engine: TemplateEngine,
}

#[async_trait]
impl EventHandler for EmailHandler {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        if let Some(event) = envelope.downcast_ref::<OrderPlaced>() {
            self.send_order_email(event).await?;
        }
        Ok(())
    }
    
    fn name(&self) -> &str {
        "EmailHandler"
    }
}

// Subscribe with custom handler
let handler = EmailHandler {
    smtp_client: create_smtp_client(),
    template_engine: create_template_engine(),
};

let handle = bus.subscribe_handler::<OrderPlaced, _>(handler).await?;
```

### Unsubscribing

#### `unsubscribe(&self, handle: SubscriptionHandle) -> Result<()>`
Removes a subscription.

```rust
// Subscribe
let handle = bus.subscribe(|event: UserRegistered| async move {
    println!("User registered: {}", event.email);
}).await?;

// Later, unsubscribe
bus.unsubscribe(handle).await?;
```

### Event Bus Management

#### `emit_and_wait<T: Event>(&self, event: T) -> Result<()>`
Publishes an event and waits for handlers to complete (simplified version).

```rust
// Note: Current implementation just waits 100ms
// Use for testing, not production
bus.emit_and_wait(UserRegistered {
    user_id: 123,
    email: "test@example.com".to_string(),
}).await?;

// All handlers have processed the event
assert_eq!(get_user_count(), 1);
```

#### `stats(&self) -> EventBusStats`
Gets statistics about the event bus.

```rust
let stats = bus.stats();

println!("Total subscriptions: {}", stats.total_subscriptions);
println!("Event types: {}", stats.event_types);
println!("Events dispatched: {}", stats.dispatcher_stats.events_dispatched);
println!("Queue size: {}", stats.dispatcher_stats.queue_size);
println!("Avg dispatch time: {} μs", stats.dispatcher_stats.avg_dispatch_time_us);
```

#### `shutdown(self) -> Result<()>`
Gracefully shuts down the event bus.

```rust
// Shutdown when done
bus.shutdown().await?;

// Note: This consumes the bus, so it can't be used after
```

### Event Priority

```rust
use tokio_events::{Event, HasPriority, EventPriority};

#[derive(Debug, Clone)]
struct CriticalAlert {
    message: String,
    severity: u8,
}

impl Event for CriticalAlert {
    fn event_type() -> &'static str {
        "CriticalAlert"
    }
}

impl HasPriority for CriticalAlert {
    fn priority(&self) -> EventPriority {
        match self.severity {
            9..=10 => EventPriority::Critical,
            7..=8 => EventPriority::High,
            4..=6 => EventPriority::Normal,
            _ => EventPriority::Low,
        }
    }
}

// Publish with priority
bus.publish(CriticalAlert {
    message: "Database connection lost!".to_string(),
    severity: 10,
}).await?;
```

## Common Patterns

### 1. Service Integration Pattern

```rust
use std::sync::Arc;

pub struct UserService {
    bus: Arc<EventBus>,
    db: Database,
}

impl UserService {
    pub fn new(bus: Arc<EventBus>, db: Database) -> Self {
        Self { bus, db }
    }
    
    pub async fn register_user(&self, request: RegisterRequest) -> Result<User> {
        // Business logic
        let user = self.db.create_user(request).await?;
        
        // Publish event
        self.bus.publish(UserRegistered {
            user_id: user.id,
            email: user.email.clone(),
        }).await?;
        
        Ok(user)
    }
}

pub struct EmailService {
    bus: Arc<EventBus>,
    smtp: SmtpClient,
}

impl EmailService {
    pub async fn start(&self) -> Result<()> {
        // Subscribe to events
        self.bus.subscribe(|event: UserRegistered| {
            let smtp = self.smtp.clone();
            async move {
                send_welcome_email(&smtp, event).await;
            }
        }).await?;
        
        Ok(())
    }
}
```

### 2. Event Chaining Pattern

```rust
// First event triggers second event
let handle = bus.subscribe(|event: OrderPlaced| {
    let bus = bus.clone();
    async move {
        // Process order
        process_order(event).await;
        
        // Trigger next event
        bus.publish(OrderProcessed {
            order_id: event.order_id,
            processed_at: Utc::now(),
        }).await.unwrap();
    }
}).await?;
```

### 3. Error Handling Pattern

```rust
let handle = bus.subscribe(|event: PaymentRequired| async move {
    match process_payment(event).await {
        Ok(()) => {
            println!("Payment processed successfully");
        }
        Err(e) => {
            // Log error but don't crash
            eprintln!("Payment failed: {}", e);
            
            // Could publish failure event
            // bus.publish(PaymentFailed { ... }).await;
        }
    }
}).await?;
```

### 4. Correlation Pattern

```rust
// Track related events
let correlation_id = Uuid::new_v4();

// First event
let metadata = EventMetadata::new()
    .set_correlation_id(correlation_id);

bus.publish_with_metadata(OrderPlaced { ... }, metadata.clone()).await?;

// Related event with same correlation
bus.publish_with_metadata(PaymentProcessed { ... }, metadata).await?;

// In handlers, access correlation ID
let handle = bus.subscribe(|event: OrderPlaced| async move {
    // Access through EventEnvelope in custom handler
    println!("Processing order in correlation chain");
}).await?;
```

### 5. Shared State Pattern

```rust
use std::sync::Arc;
use tokio::sync::Mutex;

// Shared state between handlers
let counter = Arc::new(Mutex::new(0));

// First handler
let counter1 = counter.clone();
let handle1 = bus.subscribe(move |event: OrderPlaced| {
    let counter = counter1.clone();
    async move {
        let mut count = counter.lock().await;
        *count += 1;
        println!("Orders processed: {}", *count);
    }
}).await?;

// Second handler with same state
let counter2 = counter.clone();
let handle2 = bus.subscribe(move |event: OrderCancelled| {
    let counter = counter2.clone();
    async move {
        let mut count = counter.lock().await;
        *count -= 1;
        println!("Orders processed: {}", *count);
    }
}).await?;
```

## Best Practices

### 1. Event Design

```rust
// ✅ GOOD: Small, focused events
#[derive(Debug, Clone)]
struct OrderPlaced {
    order_id: u64,
    customer_id: u64,
    total: f64,
}

// ❌ BAD: Large, kitchen-sink events
#[derive(Debug, Clone)]
struct OrderEvent {
    order_id: u64,
    action: String,  // "placed", "cancelled", etc.
    customer: Customer,  // Entire object
    items: Vec<Item>,    // Too much data
    // ... 20 more fields
}
```

### 2. Handler Design

```rust
// ✅ GOOD: Fast, non-blocking handler
let handle = bus.subscribe(|event: OrderPlaced| async move {
    // Quick operation
    metrics::increment_counter("orders.placed");
    
    // Delegate heavy work
    spawn_background_job(event);
}).await?;

// ❌ BAD: Slow, blocking handler
let handle = bus.subscribe(|event: OrderPlaced| async move {
    // Don't do this in handler!
    let _result = expensive_calculation();
    thread::sleep(Duration::from_secs(10));
    
    // This blocks other events
}).await?;
```

### 3. Error Handling

```rust
// ✅ GOOD: Handle errors gracefully
let handle = bus.subscribe(|event: OrderPlaced| async move {
    if let Err(e) = process_order(event).await {
        // Log and continue
        error!("Failed to process order: {}", e);
        
        // Maybe publish error event
        // bus.publish(OrderFailed { ... }).await;
    }
}).await?;

// ❌ BAD: Panic in handler
let handle = bus.subscribe(|event: OrderPlaced| async move {
    process_order(event).await.unwrap();  // Don't panic!
}).await?;
```

### 4. Resource Management

```rust
// ✅ GOOD: Share resources properly
let db_pool = Arc::new(create_db_pool());
let cache = Arc::new(create_cache());

let handle = bus.subscribe(move |event: UserRegistered| {
    let db = db_pool.clone();
    let cache = cache.clone();
    async move {
        save_user(&db, &event).await;
        cache.invalidate_user(event.user_id).await;
    }
}).await?;

// ❌ BAD: Create resources in handler
let handle = bus.subscribe(|event: UserRegistered| async move {
    // Don't create connections per event!
    let db = create_db_connection().await;
    save_user(&db, &event).await;
}).await?;
```

## Troubleshooting

### Events Not Being Received

1. **Check event type matches exactly**:
```rust
// Publisher
impl Event for OrderPlaced {
    fn event_type() -> &'static str {
        "OrderPlaced"  // Must match!
    }
}

// Subscriber - must use same type
bus.subscribe(|event: OrderPlaced| async move {
    // ...
}).await?;
```

2. **Ensure bus hasn't shut down**:
```rust
if bus.is_shutting_down() {
    println!("Bus is shutting down!");
}
```

3. **Check subscription is active**:
```rust
// Don't drop the handle!
let handle = bus.subscribe(...).await?;
// Keep handle alive or events won't be received
```

### Memory Leaks

1. **Always unsubscribe when done**:
```rust
let handle = bus.subscribe(...).await?;
// ... use it ...
bus.unsubscribe(handle).await?;  // Clean up!
```

2. **Avoid circular references**:
```rust
// ❌ BAD: Circular reference
let bus_clone = bus.clone();
let handle = bus.subscribe(move |event: MyEvent| {
    let bus = bus_clone.clone();  // Circular!
    async move {
        bus.publish(AnotherEvent).await;
    }
}).await?;
```

### Performance Issues

1. **Check queue size**:
```rust
let stats = bus.stats();
if stats.dispatcher_stats.queue_size > 1000 {
    println!("Queue backing up!");
}
```

2. **Monitor dispatch times**:
```rust
let stats = bus.stats();
if stats.dispatcher_stats.avg_dispatch_time_us > 1000 {
    println!("Handlers are slow!");
}
```

3. **Use appropriate configuration**:
```rust
// For high throughput
let bus = EventBus::builder()
    .high_throughput()
    .build()
    .await?;
```

## Complete Example

```rust
use tokio_events::{Event, EventBus, EventMetadata};
use std::sync::Arc;
use uuid::Uuid;

// Define events
#[derive(Debug, Clone)]
struct OrderPlaced { order_id: u64, total: f64 }

#[derive(Debug, Clone)]
struct PaymentProcessed { order_id: u64, amount: f64 }

#[derive(Debug, Clone)]
struct OrderShipped { order_id: u64, tracking: String }

impl Event for OrderPlaced {
    fn event_type() -> &'static str { "OrderPlaced" }
}

impl Event for PaymentProcessed {
    fn event_type() -> &'static str { "PaymentProcessed" }
}

impl Event for OrderShipped {
    fn event_type() -> &'static str { "OrderShipped" }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create bus
    let bus = Arc::new(
        EventBus::builder()
            .reliable()
            .build()
            .await?
    );

    // Subscribe handlers
    let payment_handle = {
        let bus = bus.clone();
        bus.subscribe(move |event: OrderPlaced| {
            let bus = bus.clone();
            async move {
                println!("Processing payment for order {}", event.order_id);
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                
                bus.publish(PaymentProcessed {
                    order_id: event.order_id,
                    amount: event.total,
                }).await.unwrap();
            }
        }).await?
    };

    let shipping_handle = {
        let bus = bus.clone();
        bus.subscribe(move |event: PaymentProcessed| {
            let bus = bus.clone();
            async move {
                println!("Shipping order {}", event.order_id);
                
                bus.publish(OrderShipped {
                    order_id: event.order_id,
                    tracking: format!("TRACK-{}", event.order_id),
                }).await.unwrap();
            }
        }).await?
    };

    let notification_handle = bus.subscribe(|event: OrderShipped| async move {
        println!("Order {} shipped with tracking: {}", 
            event.order_id, event.tracking);
    }).await?;

    // Publish initial event
    let correlation_id = Uuid::new_v4();
    let metadata = EventMetadata::new()
        .set_correlation_id(correlation_id)
        .set_source("order-service");

    bus.publish_with_metadata(
        OrderPlaced { order_id: 1001, total: 99.99 },
        metadata
    ).await?;

    // Wait for event chain to complete
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    // Check stats
    let stats = bus.stats();
    println!("\nEvent Bus Stats:");
    println!("  Events dispatched: {}", stats.dispatcher_stats.events_dispatched);
    println!("  Active subscriptions: {}", stats.total_subscriptions);

    // Cleanup
    bus.unsubscribe(payment_handle).await?;
    bus.unsubscribe(shipping_handle).await?;
    bus.unsubscribe(notification_handle).await?;

    // Shutdown
    Arc::try_unwrap(bus)
        .map_err(|_| "Failed to get bus ownership")?
        .shutdown()
        .await?;

    println!("\nDone!");
    Ok(())
}
```

This guide covers all the main APIs and patterns for using tokio-events effectively!