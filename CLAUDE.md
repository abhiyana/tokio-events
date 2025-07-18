# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Essential Development Commands

### Build and Check
```bash
cargo check          # Fast compilation check
cargo build          # Build the project
cargo build --release # Optimized build
```

### Testing
```bash
cargo test            # Run all tests
cargo test --lib      # Run library tests only
cargo test test_name  # Run specific test
```

### Code Quality
```bash
cargo clippy          # Lint and additional checks
cargo fmt             # Format code
cargo fmt --check     # Check formatting without applying
```

### Examples
```bash
cargo run --example test  # Run the test example
```

## Project Architecture

This is a modern, type-safe async event bus library built on Tokio with a layered architecture:

### Core Layers
1. **Event Layer** (`src/event/`): Event trait, EventEnvelope (type erasure), EventMetadata (correlation tracking)
2. **Registry Layer** (`src/registry/`): DashMap-based type-to-subscriber mapping with thread-safe concurrent access
3. **Subscription Layer** (`src/subscription/`): Handler management with EventHandler trait, typed handlers, and lifecycle control
4. **Dispatcher Layer** (`src/dispatcher/`): Event routing via Tokio channels with worker pools and backpressure
5. **EventBus Layer** (`src/bus/`): Main API facade orchestrating all layers with builder pattern configuration

### Key Design Patterns
- **Type Erasure**: EventEnvelope allows different event types in same collections while preserving type safety
- **Zero-Copy**: Events wrapped in Arc for efficient sharing across async tasks
- **Builder Pattern**: EventBusBuilder with preset configurations (high_throughput, reliable, ordered)
- **Graceful Shutdown**: Comprehensive cleanup with timeout handling and shutdown hooks

### Event Flow
Publisher → EventBus → Dispatcher (queue) → SubscriptionManager → EventRegistry → EventHandlers (concurrent execution)

## Key Types and Traits

### Events
- Must implement `Event` trait with `event_type()` method
- Require `Send + Sync + Clone + Debug + 'static`
- Wrapped in `EventEnvelope` for type erasure
- Support priority levels and correlation tracking

### Handlers
- Function closures: `Fn(Event) -> Future<Output = ()>`
- Custom handlers implementing `EventHandler` trait
- Automatic type inference for event types
- Concurrent execution via Tokio tasks

### Configuration
- `EventBusBuilder` provides preset configurations
- Builder methods: `high_throughput()`, `reliable()`, `ordered()`, `configure()`
- Configurable queue sizes, worker threads, timeouts

## Development Guidelines

### Adding Event Types
```rust
#[derive(Debug, Clone)]
struct MyEvent { /* fields */ }

impl Event for MyEvent {
    fn event_type() -> &'static str { "MyEvent" }
}
```

### Testing Patterns
- Use `EventBus::builder().configure(|c| c.enable_tracing(false))` for tests
- Use `emit_and_wait()` for synchronous-style testing
- Always call `bus.shutdown().await` in tests to prevent resource leaks

### Error Handling
- Most operations return `Result<T, Error>`
- Event handlers should not panic - use proper error handling
- `ShuttingDown` errors indicate graceful shutdown in progress

### Performance Considerations
- DashMap registry optimized for many readers, fewer writers
- Lock-free concurrent access patterns
- Configurable worker pools for parallel event processing
- Built-in metrics for monitoring performance

## Important Notes

- This is a library crate (no main binary)
- Uses extensive async/await with Tokio runtime
- Comprehensive test coverage in each module
- Documentation examples in `docs/guide-examples.md`
- Working example in `examples/test.rs`