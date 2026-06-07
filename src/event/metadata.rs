//! Event metadata for tracking and correlation.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

/// Metadata associated with each event.
///
/// This includes tracking information like timestamps, correlation IDs,
/// and custom metadata that can be used for debugging and tracing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EventMetadata {
    /// Unique identifier for this event instance
    pub event_id: Uuid,

    /// Timestamp when the event was created
    pub timestamp: DateTime<Utc>,

    /// Correlation ID for tracing related events
    pub correlation_id: Option<Uuid>,

    /// Causation ID - the event that caused this event
    pub causation_id: Option<Uuid>,

    /// Source that generated this event
    pub source: Option<String>,

    /// User ID associated with this event
    pub user_id: Option<String>,

    /// Session ID for tracking user sessions
    pub session_id: Option<String>,

    /// Custom metadata as key-value pairs
    pub custom: HashMap<String, String>,

    /// Optional scheduled delivery timestamp. If set, the event will not be delivered until this time.
    pub deliver_at: Option<DateTime<Utc>>,

    /// Optional explicit topic/subject for routing (defaults to TypeName if not set)
    pub topic: Option<String>,

    /// Optional reply-to topic for Request-Reply (RPC) correlation
    pub reply_to: Option<String>,
}

impl EventMetadata {
    /// Create new metadata with generated event ID and current timestamp
    pub fn new() -> Self {
        Self {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            correlation_id: None,
            causation_id: None,
            source: None,
            user_id: None,
            session_id: None,
            custom: HashMap::new(),
            deliver_at: None,
            topic: None,
            reply_to: None,
        }
    }

    /// Set a specific correlation ID.
    ///
    /// Correlation IDs are used in distributed tracing to link a chain of events 
    /// together across multiple microservices. If this event is published as a reaction
    /// to an incoming event, they should share the same correlation ID.
    pub fn with_correlation(correlation_id: Uuid) -> Self {
        let mut metadata = Self::new();
        metadata.correlation_id = Some(correlation_id);
        metadata
    }

    /// Set the correlation ID
    pub fn set_correlation_id(mut self, id: Uuid) -> Self {
        self.correlation_id = Some(id);
        self
    }

    /// Set the causation ID.
    ///
    /// The causation ID identifies the specific `event_id` of the parent event that 
    /// triggered the creation of this current event.
    pub fn set_causation_id(mut self, id: Uuid) -> Self {
        self.causation_id = Some(id);
        self
    }

    /// Set the event source.
    ///
    /// Identifies the service, domain, or component that produced this event.
    /// Useful for routing and debugging in large microservice architectures.
    pub fn set_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set the User ID associated with this event.
    ///
    /// This is highly useful for auditing and analytics, allowing you to trace
    /// all events triggered directly or indirectly by a specific user.
    pub fn set_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Set the Session ID associated with this event.
    ///
    /// Like `user_id`, this helps trace all events generated during a single 
    /// active user session in your application.
    pub fn set_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Add a custom metadata field.
    ///
    /// This allows you to attach any arbitrary string-based key-value pairs to the event.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let metadata = EventMetadata::new()
    ///     .with_custom("tenant_id", "acme-corp")
    ///     .with_custom("region", "eu-west-1");
    /// ```
    pub fn with_custom(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.custom.insert(key.into(), value.into());
        self
    }

    /// Set a custom topic for Subject-Based routing.
    ///
    /// By default, events are routed based on their Rust type. If you use `bus.publish_to()`
    /// or `metadata.with_topic()`, the event will only be delivered to handlers that 
    /// explicitly subscribed to this topic string via `bus.subscribe_topic()`.
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// Set a reply-to topic for Request-Reply (RPC) pattern
    pub fn with_reply_to(mut self, reply_to: impl Into<String>) -> Self {
        self.reply_to = Some(reply_to.into());
        self
    }

    /// Schedule this event to be delivered at an exact future time.
    /// Schedule the event to be delivered at an exact future time.
    ///
    /// The event will be held in the dispatcher (or persistent storage, if enabled)
    /// and will not be dispatched until the specified UTC timestamp.
    pub fn schedule_at(mut self, time: DateTime<Utc>) -> Self {
        self.deliver_at = Some(time);
        self
    }

    /// Schedule the event to be delivered after a specific delay.
    ///
    /// The event will be held in the dispatcher (or persistent storage, if enabled) 
    /// and will not be sent to subscribers until the `delay` duration has passed.
    pub fn delay(mut self, delay: std::time::Duration) -> Self {
        self.deliver_at = Some(Utc::now() + delay);
        self
    }

    /// Get custom metadata value
    pub fn get_custom(&self, key: &str) -> Option<&String> {
        self.custom.get(key)
    }

    /// Create a chain of events by linking causation to another event's metadata.
    ///
    /// This automatically sets the `causation_id` of this event to the `event_id` of the parent,
    /// and inherits the `correlation_id`, `user_id`, and `session_id` to maintain distributed trace context.
    ///
    /// # Arguments
    ///
    /// * `parent` - The `EventMetadata` of the preceding event in the workflow.
    pub fn chain_from(&mut self, parent: &EventMetadata) {
        self.causation_id = Some(parent.event_id);
        self.correlation_id = parent.correlation_id.or(Some(parent.event_id));

        // Inherit user and session context
        if self.user_id.is_none() {
            self.user_id = parent.user_id.clone();
        }
        if self.session_id.is_none() {
            self.session_id = parent.session_id.clone();
        }
    }
}

impl Default for EventMetadata {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for EventMetadata
#[allow(missing_debug_implementations)]
pub struct MetadataBuilder {
    metadata: EventMetadata,
}

impl Default for MetadataBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MetadataBuilder {
    /// Create a new metadata builder
    pub fn new() -> Self {
        Self {
            metadata: EventMetadata::new(),
        }
    }

    /// Set correlation ID
    pub fn correlation_id(mut self, id: Uuid) -> Self {
        self.metadata.correlation_id = Some(id);
        self
    }

    /// Set causation ID
    pub fn causation_id(mut self, id: Uuid) -> Self {
        self.metadata.causation_id = Some(id);
        self
    }

    /// Set source
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.metadata.source = Some(source.into());
        self
    }

    /// Set user ID
    pub fn user_id(mut self, user_id: impl Into<String>) -> Self {
        self.metadata.user_id = Some(user_id.into());
        self
    }

    /// Set session ID
    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.metadata.session_id = Some(session_id.into());
        self
    }

    /// Add custom metadata
    pub fn custom(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.custom.insert(key.into(), value.into());
        self
    }

    /// Set a custom topic for Subject-Based routing
    pub fn topic(mut self, topic: impl Into<String>) -> Self {
        self.metadata.topic = Some(topic.into());
        self
    }

    /// Set a reply-to topic for Request-Reply (RPC) pattern
    pub fn reply_to(mut self, reply_to: impl Into<String>) -> Self {
        self.metadata.reply_to = Some(reply_to.into());
        self
    }

    /// Build the metadata
    pub fn build(self) -> EventMetadata {
        self.metadata
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_creation() {
        let metadata = EventMetadata::new();
        assert_ne!(metadata.event_id, Uuid::nil());
        assert!(metadata.correlation_id.is_none());
        assert!(metadata.custom.is_empty());
    }

    #[test]
    fn test_metadata_builder() {
        let correlation_id = Uuid::max();
        let metadata = MetadataBuilder::new()
            .correlation_id(correlation_id)
            .source("test-service")
            .user_id("user123")
            .custom("environment", "test")
            .build();

        assert_eq!(metadata.correlation_id, Some(correlation_id));
        assert_eq!(metadata.source, Some("test-service".to_string()));
        assert_eq!(metadata.user_id, Some("user123".to_string()));
        assert_eq!(
            metadata.get_custom("environment"),
            Some(&"test".to_string())
        );
    }

    #[test]
    fn test_metadata_chaining() {
        let parent = EventMetadata::new()
            .set_user_id("user123")
            .set_session_id("session456");

        let mut child = EventMetadata::new();
        child.chain_from(&parent);

        assert_eq!(child.causation_id, Some(parent.event_id));
        assert_eq!(child.correlation_id, Some(parent.event_id));
        assert_eq!(child.user_id, Some("user123".to_string()));
        assert_eq!(child.session_id, Some("session456".to_string()));
    }
}
