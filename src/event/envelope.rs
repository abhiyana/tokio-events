//! Event envelope for type-erased event storage and transmission.

use crate::event::{Event, EventMetadata, EventPriority, HasPriority};
use std::any::{Any, TypeId};
use std::fmt;
use std::sync::Arc;

/// A type-erased wrapper for events.
///
/// The EventEnvelope allows us to store different event types in the same
/// collection while preserving type safety for handlers.
#[derive(Clone)]
pub struct EventEnvelope {
    /// The type-erased event payload
    payload: Arc<dyn Any + Send + Sync>,

    /// Type ID of the original event
    type_id: TypeId,

    /// Human-readable type name for debugging
    type_name: &'static str,

    /// Event metadata
    pub metadata: EventMetadata,

    /// Event priority
    pub priority: EventPriority,
}

impl EventEnvelope {
    /// Create a new envelope from an event
    pub fn new<T: Event>(event: T) -> Self {
        Self::with_metadata(event, EventMetadata::new())
    }

    /// Create a new envelope with custom metadata
    pub fn with_metadata<T: Event>(event: T, metadata: EventMetadata) -> Self {
        // For events that don't implement HasPriority, use default priority
        Self {
            payload: Arc::new(event),
            type_id: T::type_id(),
            type_name: T::event_type(),
            metadata,
            priority: EventPriority::default(),
        }
    }

    /// Create a new envelope with custom metadata and priority
    pub fn with_priority<T: Event + HasPriority>(event: T, metadata: EventMetadata) -> Self {
        let priority = event.priority();

        Self {
            payload: Arc::new(event),
            type_id: T::type_id(),
            type_name: T::event_type(),
            metadata,
            priority,
        }
    }

    /// Get the event type name
    pub fn event_type(&self) -> &'static str {
        self.type_name
    }

    /// Get the type ID of the contained event
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// Try to downcast to a specific event type
    pub fn downcast_ref<T: Event>(&self) -> Option<&T> {
        if self.type_id == TypeId::of::<T>() {
            self.payload.downcast_ref::<T>()
        } else {
            None
        }
    }

    #[allow(clippy::result_large_err)]
    /// Try to extract the event as a specific type
    pub fn try_into_inner<T: Event>(self) -> Result<Arc<T>, Self> {
        if self.type_id == TypeId::of::<T>() {
            // Try to downcast the Arc
            match Arc::downcast::<T>(self.payload.clone()) {
                Ok(event) => Ok(event),
                Err(_) => Err(self),
            }
        } else {
            Err(self)
        }
    }

    /// Check if this envelope contains a specific event type
    pub fn is<T: Event>(&self) -> bool {
        self.type_id == TypeId::of::<T>()
    }

    /// Get the correlation ID from metadata
    pub fn correlation_id(&self) -> Option<uuid::Uuid> {
        self.metadata.correlation_id
    }

    /// Get the event ID
    pub fn event_id(&self) -> uuid::Uuid {
        self.metadata.event_id
    }

    /// Clone the inner event payload
    pub fn clone_payload(&self) -> Arc<dyn Any + Send + Sync> {
        self.payload.clone()
    }

    /// Create a new envelope that chains from this one
    pub fn chain<T: Event>(&self, event: T) -> Self {
        let mut metadata = EventMetadata::new();
        metadata.chain_from(&self.metadata);
        Self::with_metadata(event, metadata)
    }
}

impl fmt::Debug for EventEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventEnvelope")
            .field("type_name", &self.type_name)
            .field("event_id", &self.metadata.event_id)
            .field("priority", &self.priority)
            .field("correlation_id", &self.metadata.correlation_id)
            .finish()
    }
}

/// Builder for creating event envelopes with custom configuration
#[derive(Debug)]
pub struct EnvelopeBuilder<T: Event> {
    event: T,
    metadata: EventMetadata,
    priority: Option<EventPriority>,
}

impl<T: Event> EnvelopeBuilder<T> {
    /// Create a new envelope builder
    pub fn new(event: T) -> Self {
        Self {
            event,
            metadata: EventMetadata::new(),
            priority: None,
        }
    }

    /// Set custom metadata
    pub fn metadata(mut self, metadata: EventMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set correlation ID
    pub fn correlation_id(mut self, id: uuid::Uuid) -> Self {
        self.metadata.correlation_id = Some(id);
        self
    }

    /// Set causation ID
    pub fn causation_id(mut self, id: uuid::Uuid) -> Self {
        self.metadata.causation_id = Some(id);
        self
    }

    /// Set event source
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.metadata.source = Some(source.into());
        self
    }

    /// Set priority
    pub fn priority(mut self, priority: EventPriority) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Build the envelope
    pub fn build(self) -> EventEnvelope {
        let mut envelope = EventEnvelope::with_metadata(self.event, self.metadata);
        if let Some(priority) = self.priority {
            envelope.priority = priority;
        }
        envelope
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[derive(Debug, Clone)]
    struct TestEvent {
        id: u64,
        _message: String,
    }

    impl Event for TestEvent {
        fn event_type() -> &'static str {
            "TestEvent"
        }
    }

    impl Event for String {
        fn event_type() -> &'static str {
            "StringEvent"
        }
    }

    #[test]
    fn test_envelope_creation() {
        let event = TestEvent {
            id: 123,
            _message: "test".to_string(),
        };

        let envelope = EventEnvelope::new(event.clone());
        assert_eq!(envelope.event_type(), "TestEvent");
        assert_eq!(envelope.type_id(), TypeId::of::<TestEvent>());
        assert!(envelope.is::<TestEvent>());
        assert!(!envelope.is::<String>());
    }

    #[test]
    fn test_envelope_downcast() {
        let event = TestEvent {
            id: 456,
            _message: "downcast test".to_string(),
        };

        let envelope = EventEnvelope::new(event);

        let downcast = envelope.downcast_ref::<TestEvent>();
        assert!(downcast.is_some());
        assert_eq!(downcast.unwrap().id, 456);
        let wrong_downcast = envelope.downcast_ref::<String>();
        assert!(wrong_downcast.is_none());
    }

    #[test]
    fn test_envelope_builder() {
        let correlation_id = Uuid::max();
        let event = TestEvent {
            id: 789,
            _message: "builder test".to_string(),
        };

        let envelope = EnvelopeBuilder::new(event)
            .correlation_id(correlation_id)
            .source("test-source")
            .priority(EventPriority::High)
            .build();

        assert_eq!(envelope.correlation_id(), Some(correlation_id));
        assert_eq!(envelope.metadata.source, Some("test-source".to_string()));
        assert_eq!(envelope.priority, EventPriority::High);
    }

    #[test]
    fn test_envelope_chaining() {
        let parent_event = TestEvent {
            id: 1,
            _message: "parent".to_string(),
        };
        let parent_envelope = EventEnvelope::new(parent_event);

        let child_event = TestEvent {
            id: 2,
            _message: "child".to_string(),
        };
        let child_envelope = parent_envelope.chain(child_event);

        assert_eq!(
            child_envelope.metadata.causation_id,
            Some(parent_envelope.event_id())
        );
        assert_eq!(
            child_envelope.metadata.correlation_id,
            Some(parent_envelope.event_id())
        );
    }
}
