//! # tokio-events
//!
//! A modern, type-safe async event bus for Rust applications built on Tokio.
//!
//! ## Features
//!
//! - **Type-safe** event publishing and subscription
//! - **High performance** with zero-copy message passing
//! - **Async-first** design built on Tokio
//! - **Flexible** subscription management
//! - **Thread-safe** by default
//!
//! ## Quick Example
//!
//! ```rust,no_run
//! use tokio_events::{Event, EventBus};
//!
//! #[derive(Debug, Clone)]
//! struct UserRegistered {
//!     user_id: u64,
//!     email: String,
//! }
//!
//! impl Event for UserRegistered {
//!     fn event_type() -> &'static str {
//!         "UserRegistered"
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create event bus
//!     let bus = EventBus::builder().build().await?;
//!
//!     // Subscribe to events
//!     let handle = bus.subscribe(|event: UserRegistered| async move {
//!         println!("New user registered: {}", event.email);
//!     }).await?;
//!
//!     // Publish events
//!     bus.publish(UserRegistered {
//!         user_id: 123,
//!         email: "user@example.com".to_string(),
//!     }).await?;
//!
//!     // Unsubscribe when done
//!     bus.unsubscribe(handle).await?;
//!
//!     Ok(())
//! }
//! ```

#![warn(
    missing_docs,
    rust_2018_idioms,
    missing_debug_implementations,
    unreachable_pub
)]
#![cfg_attr(docsrs, feature(doc_cfg))]

/// Core event system traits and types
pub mod event;

/// Error types and result aliases
pub mod error;

/// Event registry for type-to-subscriber mapping
pub mod registry;

/// Subscription management for event handlers
pub mod subscription;

/// Event dispatcher for routing events
pub mod dispatcher;

/// The main event bus implementation
pub mod bus;

// These modules will be uncommented as we implement them
// pub mod bus;
// pub mod registry;
// pub mod subscription;
// pub mod dispatcher;

// #[cfg(feature = "metrics")]
// #[cfg_attr(docsrs, doc(cfg(feature = "metrics")))]
// pub mod metrics;

// #[cfg(feature = "middleware")]
// #[cfg_attr(docsrs, doc(cfg(feature = "middleware")))]
// pub mod middleware;

// Re-export commonly used types
pub use error::{Error, Result};
pub use event::{Event, EventEnvelope, EventMetadata, EventPriority, HasPriority};
pub use subscription::{SubscriptionHandle, EventHandler};
pub use bus::{EventBus, EventBusBuilder};

// Will be added when implemented:
// pub use metrics::Metrics;

// Will be added when implemented:
// pub use bus::{EventBus, EventBusBuilder};
// pub use subscription::SubscriptionHandle;

/// Prelude module for convenient imports
///
/// # Example
/// ```rust
/// use tokio_events::prelude::*;
/// ```
pub mod prelude {
    pub use crate::event::{Event, EventPriority, HasPriority};
    pub use crate::error::{Error, Result};
    pub use crate::bus::{EventBus, EventBusBuilder};
    pub use crate::subscription::SubscriptionHandle;
}