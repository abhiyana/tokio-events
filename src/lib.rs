#![doc = include_str!("../README.md")]
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
pub mod global;

/// Persistent storage for events
#[cfg(feature = "persistence")]
#[cfg_attr(docsrs, doc(cfg(feature = "persistence")))]
pub mod persistence;

/// Remote transport engines (e.g., NATS) for distributed event buses.
#[cfg(feature = "remote")]
#[cfg_attr(docsrs, doc(cfg(feature = "remote")))]
pub mod remote;

// Re-export commonly used types
pub use bus::{EventBus, EventBusBuilder};
pub use error::{Error, Result};
pub use event::{Event, EventEnvelope, EventMetadata, EventPriority, HasPriority};

#[cfg(feature = "remote")]
#[cfg_attr(docsrs, doc(cfg(feature = "remote")))]
pub use event::Remote;
pub use subscription::{EventHandler, SubscriptionHandle};

#[cfg(feature = "macros")]
#[cfg_attr(docsrs, doc(cfg(feature = "macros")))]
pub use tokio_events_macros::{Event, Remote};

/// Prelude module for convenient imports
///
/// # Example
/// ```rust
/// use tokio_events::prelude::*;
/// ```
pub mod prelude {
    pub use crate::bus::{EventBus, EventBusBuilder};
    pub use crate::error::{Error, Result};
    pub use crate::event::{Event, EventPriority, HasPriority};

    #[cfg(feature = "remote")]
    pub use crate::event::Remote;

    pub use crate::subscription::SubscriptionHandle;

    #[cfg(feature = "macros")]
    pub use tokio_events_macros::{Event, Remote};
}
