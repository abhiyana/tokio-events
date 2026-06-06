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
/// use uuid::Uuid;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Clone, Debug, Event, Serialize, Deserialize)]
/// struct UserRegistered {
///     user_id: Uuid,
///     email: String,
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

    /// Serialize the event to raw bytes (e.g., JSON or Protobuf)
    fn serialize_event(&self) -> crate::Result<Vec<u8>>;

    /// Deserialize the event from raw bytes
    fn deserialize_event(bytes: &[u8]) -> crate::Result<Self>
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

/// A marker trait for events that can be serialized to JSON easily.
pub trait JsonSerializableEvent: Event + serde::Serialize + serde::de::DeserializeOwned {
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

// Blanket impl
impl<T: Event + serde::Serialize + serde::de::DeserializeOwned> JsonSerializableEvent for T {}

/// Priority levels for event handling.
///
/// Higher priority events are processed before lower priority ones.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BroadcastEvent {
    /// The message to be broadcast to all subscribers.
    pub message: String,
}

impl Event for BroadcastEvent {
    fn event_type() -> &'static str {
        "BroadcastEvent"
    }

    fn serialize_event(&self) -> crate::Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| crate::Error::SerializationError(e.to_string()))
    }

    fn deserialize_event(bytes: &[u8]) -> crate::Result<Self> {
        serde_json::from_slice(bytes).map_err(|e| crate::Error::SerializationError(e.to_string()))
    }
}

/// A trait for events that can be sent over a distributed network (e.g., NATS).
///
/// This trait extends the base `Event` trait and requires serialization capabilities.
#[cfg(feature = "remote")]
#[cfg_attr(docsrs, doc(cfg(feature = "remote")))]
pub trait Remote: Event {
    /// The unique routing topic for this event over the network.
    ///
    /// Must contain at least 3 segments separated by dots (e.g., `domain.service.entity.action`).
    fn remote_topic() -> &'static str;
}

#[cfg(test)]
mod tests {
    extern crate self as tokio_events;
    use super::*;
    use crate::event;

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct TestEvent {
        id: u64,
        data: String,
    }

    impl Event for TestEvent {
        fn event_type() -> &'static str {
            "TestEvent"
        }

        fn serialize_event(&self) -> crate::Result<Vec<u8>> {
            serde_json::to_vec(self).map_err(|e| crate::Error::SerializationError(e.to_string()))
        }

        fn deserialize_event(bytes: &[u8]) -> crate::Result<Self> {
            serde_json::from_slice(bytes)
                .map_err(|e| crate::Error::SerializationError(e.to_string()))
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
        assert_eq!(downcast.unwrap().data, "test");
    }

    #[test]
    fn test_priority_ordering() {
        assert!(EventPriority::Critical > EventPriority::High);
        assert!(EventPriority::High > EventPriority::Normal);
        assert!(EventPriority::Normal > EventPriority::Low);
    }

    #[cfg(feature = "protobuf")]
    #[test]
    fn test_protobuf_core_serialization() {
        #[derive(Clone, PartialEq, prost::Message, tokio_events_macros::Event)]
        #[event(format = "protobuf")]
        struct ProtoUnit {
            #[prost(uint64, tag = "1")]
            pub id: u64,
        }

        let event = ProtoUnit { id: 999 };

        // 1. Verify it serializes correctly using prost
        let bytes = event
            .serialize_event()
            .expect("Failed to serialize via protobuf");
        assert!(!bytes.is_empty());

        // 2. Verify it deserializes correctly
        let reconstructed =
            ProtoUnit::deserialize_event(&bytes).expect("Failed to deserialize via protobuf");
        assert_eq!(reconstructed.id, 999);
        assert_eq!(
            ProtoUnit::event_type(),
            concat!(module_path!(), "::ProtoUnit")
        );
    }
}
