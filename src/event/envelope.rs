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
    /// The type-erased event payload (None if loaded from disk)
    payload: Option<Arc<dyn Any + Send + Sync>>,

    /// The serialized event payload (Some if loaded from disk)
    pub(crate) payload_bytes: Option<Vec<u8>>,

    /// Function to serialize the in-memory payload
    serializer: fn(&Arc<dyn Any + Send + Sync>) -> crate::Result<Vec<u8>>,

    /// Type ID of the original event
    type_id: TypeId,

    /// Human-readable type name for debugging
    type_name: String,

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
            payload: Some(Arc::new(event)),
            payload_bytes: None,
            serializer: |any| {
                let event = any.downcast_ref::<T>().ok_or_else(|| {
                    crate::Error::internal("Failed to downcast for serialization")
                })?;
                event.serialize_event()
            },
            type_id: T::type_id(),
            type_name: T::event_type().to_string(),
            metadata,
            priority: EventPriority::default(),
        }
    }

    /// Get the raw network bytes if available (e.g., for DLQ inspection)
    pub fn payload_bytes(&self) -> Option<&[u8]> {
        self.payload_bytes.as_deref()
    }

    /// Create a new envelope with custom metadata and a calculated priority.
    ///
    /// This constructor is automatically used if your event type implements the
    /// `HasPriority` trait.
    pub fn with_priority<T: Event + HasPriority>(event: T, metadata: EventMetadata) -> Self {
        let priority = event.priority();

        Self {
            payload: Some(Arc::new(event)),
            payload_bytes: None,
            serializer: |any| {
                let event = any.downcast_ref::<T>().ok_or_else(|| {
                    crate::Error::internal("Failed to downcast for serialization")
                })?;
                event.serialize_event()
            },
            type_id: T::type_id(),
            type_name: T::event_type().to_string(),
            metadata,
            priority,
        }
    }

    /// Create a new envelope from serialized raw bytes.
    ///
    /// This is an advanced method used exclusively by the persistence engine (`redb`)
    /// and the distributed transport layer (`NATS`) to reconstruct an envelope from disk 
    /// or network without needing to know its concrete generic type `T` at runtime.
    pub fn from_serialized(
        type_id: TypeId,
        type_name: String,
        metadata: EventMetadata,
        priority: EventPriority,
        payload_bytes: Vec<u8>,
    ) -> Self {
        Self {
            payload: None,
            payload_bytes: Some(payload_bytes),
            serializer: |_| {
                Err(crate::Error::internal(
                    "Cannot serialize an already serialized event",
                ))
            },
            type_id,
            type_name,
            metadata,
            priority,
        }
    }

    /// Get the event type name
    pub fn event_type(&self) -> &str {
        &self.type_name
    }

    /// Get the type ID of the contained event
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// Try to downcast to a specific event type (only works if in-memory)
    pub fn downcast_ref<T: Event>(&self) -> Option<&T> {
        if self.type_id == TypeId::of::<T>() {
            self.payload.as_ref().and_then(|p| p.downcast_ref::<T>())
        } else {
            None
        }
    }

    /// Extract the concrete event payload from the envelope.
    ///
    /// The envelope stores the event as a type-erased `Any` object (or as raw bytes 
    /// if loaded from a persistent disk). This method safely attempts to downcast or 
    /// deserialize the payload back into the requested concrete type `T`.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let event: MyEvent = envelope.get_event::<MyEvent>()?;
    /// println!("Extracted event: {:?}", event);
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if `T` does not match the actual type inside the envelope,
    /// or if deserialization fails (when loading from disk).
    pub fn get_event<T: Event>(&self) -> crate::Result<T> {
        if self.type_id != TypeId::of::<T>() {
            return Err(crate::Error::EventNotRegistered {
                type_name: self.type_name.clone(),
            });
        }

        if let Some(payload) = &self.payload {
            // It's in memory, clone it
            if let Some(event) = payload.downcast_ref::<T>() {
                return Ok(event.clone());
            }
        }

        if let Some(bytes) = &self.payload_bytes {
            // It was loaded from disk, deserialize it
            return T::deserialize_event(bytes);
        }

        Err(crate::Error::internal("Event envelope is empty"))
    }

    /// Get the serialized payload bytes (serializes on demand if needed)
    pub fn into_bytes(&self) -> crate::Result<Vec<u8>> {
        if let Some(bytes) = &self.payload_bytes {
            Ok(bytes.clone())
        } else if let Some(payload) = &self.payload {
            (self.serializer)(payload)
        } else {
            Err(crate::Error::internal("Event envelope is empty"))
        }
    }

    #[allow(clippy::result_large_err)]
    /// Try to extract the event as a specific type
    pub fn try_into_inner<T: Event>(self) -> Result<Arc<T>, Self> {
        if self.type_id == TypeId::of::<T>() {
            if let Some(payload) = self.payload.clone() {
                // Try to downcast the Arc
                match Arc::downcast::<T>(payload) {
                    Ok(event) => Ok(event),
                    Err(_) => Err(self),
                }
            } else {
                Err(self)
            }
        } else {
            Err(self)
        }
    }

    /// Check if this envelope contains a specific event type
    pub fn is<T: Event>(&self) -> bool {
        self.type_id == TypeId::of::<T>()
    }

    /// Get the correlation ID from metadata.
    ///
    /// The correlation ID is used in distributed tracing to link a chain of events 
    /// together across multiple microservices. If this event was published as a 
    /// reaction to another event, they should share the same correlation ID.
    ///
    /// # Returns
    ///
    /// Returns `Some(Uuid)` if a correlation ID was attached, or `None`.
    pub fn correlation_id(&self) -> Option<uuid::Uuid> {
        self.metadata.correlation_id
    }

    /// Get the unique ID of this specific event occurrence.
    ///
    /// This is guaranteed to be a unique `Uuid` (v4). It is primarily used for
    /// deduplication, exactly-once delivery semantics, and tracking specific events
    /// in logs or tracing platforms.
    pub fn event_id(&self) -> uuid::Uuid {
        self.metadata.event_id
    }

    /// Clone the inner event payload
    pub fn clone_payload(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        self.payload.clone()
    }

    /// Create a new envelope that chains from this one (Causality).
    ///
    /// When you publish a new event as a direct result of processing this event,
    /// you should `chain()` them. This automatically carries over the `correlation_id`
    /// (to keep them in the same distributed trace) and sets the `causation_id` 
    /// of the new event to the `event_id` of this envelope.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // Inside an event handler for `OrderPlaced`:
    /// let next_event = PaymentProcessed { ... };
    /// let chained_envelope = incoming_envelope.chain(next_event);
    /// bus.publish_envelope(chained_envelope).await?;
    /// ```
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
    /// Create a new envelope builder.
    ///
    /// # Arguments
    ///
    /// * `event` - The event payload.
    ///
    /// # Returns
    ///
    /// Returns a new `EnvelopeBuilder`.
    pub fn new(event: T) -> Self {
        Self {
            event,
            metadata: EventMetadata::new(),
            priority: None,
        }
    }

    /// Set custom metadata for the event.
    ///
    /// # Arguments
    ///
    /// * `metadata` - The `EventMetadata` struct containing routing and tracing data.
    pub fn metadata(mut self, metadata: EventMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Set the correlation ID for distributed tracing.
    ///
    /// # Arguments
    ///
    /// * `id` - The correlation `Uuid`.
    pub fn correlation_id(mut self, id: uuid::Uuid) -> Self {
        self.metadata.correlation_id = Some(id);
        self
    }

    /// Set the causation ID.
    ///
    /// The causation ID is the `event_id` of the parent event that triggered the creation
    /// of this event. This is useful for auditing and debugging causality chains.
    pub fn causation_id(mut self, id: uuid::Uuid) -> Self {
        self.metadata.causation_id = Some(id);
        self
    }

    /// Set the event source.
    ///
    /// Identifies the service, domain, or component that produced this event.
    /// Useful for routing and debugging in large microservice architectures.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.metadata.source = Some(source.into());
        self
    }

    /// Set the event priority.
    ///
    /// In highly congested systems, High priority events may bypass standard queues
    /// or be selected first by custom dispatchers.
    pub fn priority(mut self, priority: EventPriority) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Build the final `EventEnvelope`.
    ///
    /// # Returns
    ///
    /// Returns the fully constructed `EventEnvelope`.
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

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct TestEvent {
        id: u64,
        _message: String,
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

    // Note: String cannot easily implement Event if it requires Serialize without a newtype.
    // So we'll use a newtype for StringEvent.
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct StringEvent(String);

    impl Event for StringEvent {
        fn event_type() -> &'static str {
            "StringEvent"
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
    fn test_envelope_creation() {
        let event = TestEvent {
            id: 123,
            _message: "test".to_string(),
        };

        let envelope = EventEnvelope::new(event.clone());
        assert_eq!(envelope.event_type(), "TestEvent");
        assert_eq!(envelope.type_id(), TypeId::of::<TestEvent>());
        assert!(envelope.is::<TestEvent>());
        assert!(!envelope.is::<StringEvent>());
    }

    #[test]
    fn test_envelope_downcast() {
        let event = TestEvent {
            id: 456,
            _message: "downcast test".to_string(),
        };

        let envelope = EventEnvelope::new(event);

        let downcast = envelope.get_event::<TestEvent>();
        assert!(downcast.is_ok());
        assert_eq!(downcast.unwrap().id, 456);
        let wrong_downcast = envelope.get_event::<StringEvent>();
        assert!(wrong_downcast.is_err());
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

    #[test]
    fn test_envelope_poison_pill() {
        // Create an envelope with a fallback type for the poison pill
        let mut envelope = EventEnvelope::new(crate::event::BroadcastEvent {
            message: "Poison Pill".to_string(),
        });

        // Override the payload bytes with the raw poison pill
        let broken_bytes = b"{\"broken\": \"json\"";
        envelope.payload_bytes = Some(broken_bytes.to_vec());

        // We can access the payload bytes for the DLQ to inspect
        assert_eq!(
            envelope.payload_bytes.as_deref(),
            Some(broken_bytes.as_slice())
        );
    }
}
