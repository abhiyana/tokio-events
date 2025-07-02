//! Core event system traits and types.
//!
//! This module defines the fundamental `Event` trait and related types
//! that form the foundation of the event bus system.

use std::any::{Any, TypeId};
use std::fmt::Debug;
use std::sync::Arc;

pub mod envelope;
pub mod metadata;

pub use envelope::EventEnvelope;
pub use metadata::EventMetadata;

/// Core trait that all events must implement.
///
/// Events are the fundamental unit of communication in the event bus.
/// They must be cloneable, thread-safe, and have a static lifetime.
///
/// # Example
///
/// ```rust
/// use tokio_events::Event;
///
/// #[derive(Debug, Clone)]
/// struct UserRegistered {
///     user_id: u64,
///     email: String,
/// }
///
/// impl Event for UserRegistered {
///     fn event_type() -> &'static str {
///         "UserRegistered"
///     }
/// }
/// ```
pub trait Event: Send + Sync + Clone + Debug + 'static {
    /// Returns the type name of this event.
    ///
    /// This is used for debugging and logging purposes.
    /// It should be a stable, unique identifier for the event type.
    fn event_type() -> &'static str
    where
        Self: Sized;

    /// Get the TypeId for this event type.
    ///
    /// This is used internally for type-safe event routing.
    fn type_id() -> TypeId
    where
        Self: Sized,
    {
        TypeId::of::<Self>()
    }

    /// Convert this event into a type-erased Any trait object.
    ///
    /// This is used internally for storing events in collections.
    fn as_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

/// A marker trait for events that can be serialized.
///
/// This is useful for events that need to be persisted or
/// sent over the network.
pub trait SerializableEvent: Event + serde::Serialize + serde::de::DeserializeOwned {
    /// Serialize this event to JSON
    fn to_json(&self) -> crate::Result<String> {
        serde_json::to_string(self).map_err(|e| crate::Error::SerializationError(e.to_string()))
    }

    /// Deserialize an event from JSON
    fn from_json(json: &str) -> crate::Result<Self>
    where
        Self: Sized,
    {
        serde_json::from_str(json).map_err(|e| crate::Error::SerializationError(e.to_string()))
    }
}

/// Priority levels for event handling.
///
/// Higher priority events are processed before lower priority ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[derive(Default)]
pub enum EventPriority {
    /// Lowest priority - processed last
    Low = 0,
    /// Normal priority - default for most events  
    #[default]
    Normal = 1,
    /// High priority - processed before normal events
    High = 2,
    /// Critical priority - processed immediately
    Critical = 3,
}


/// Trait for events that have a priority.
///
/// This is a separate trait from Event to maintain object safety.
/// Implement this trait on your event types to give them priority.
pub trait HasPriority {
    /// Get the priority of this event
    fn priority(&self) -> EventPriority {
        EventPriority::default()
    }
}

/// A broadcast event that all subscribers receive.
///
/// This is useful for system-wide notifications.
#[derive(Debug, Clone)]
pub struct BroadcastEvent {
    /// The message to be broadcast to all subscribers.
    pub message: String,
}

impl Event for BroadcastEvent {
    fn event_type() -> &'static str {
        "BroadcastEvent"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event;

    #[derive(Debug, Clone)]
    struct TestEvent {
        id: u64,
        data: String,
    }

    impl Event for TestEvent {
        fn event_type() -> &'static str {
            "TestEvent"
        }
    }

    #[test]
    fn test_event_type_id() {
        let type_id1 = <event::tests::TestEvent as event::Event>::type_id();
        let type_id2 = <event::tests::TestEvent as event::Event>::type_id();
        assert_eq!(type_id1, type_id2);
    }

    #[test]
    fn test_event_as_any() {
        let event = Arc::new(TestEvent {
            id: 123,
            data: "test".to_string(),
        });

        let any = event.clone().as_any();
        let downcast = any.downcast_ref::<TestEvent>();
        assert!(downcast.is_some());
        assert_eq!(downcast.unwrap().id, 123);
    }

    #[test]
    fn test_priority_ordering() {
        assert!(EventPriority::Critical > EventPriority::High);
        assert!(EventPriority::High > EventPriority::Normal);
        assert!(EventPriority::Normal > EventPriority::Low);
    }
}
