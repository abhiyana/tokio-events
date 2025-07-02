//! Event metadata for tracking and correlation.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

/// Metadata associated with each event.
///
/// This includes tracking information like timestamps, correlation IDs,
/// and custom metadata that can be used for debugging and tracing.
#[derive(Debug, Clone)]
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
}

impl EventMetadata {
    /// Create new metadata with generated event ID and current timestamp
    pub fn new() -> Self {
        Self {
            event_id: Uuid::max(),
            timestamp: Utc::now(),
            correlation_id: None,
            causation_id: None,
            source: None,
            user_id: None,
            session_id: None,
            custom: HashMap::new(),
        }
    }

    /// Create metadata with a specific correlation ID
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

    /// Set the causation ID (the event that caused this one)
    pub fn set_causation_id(mut self, id: Uuid) -> Self {
        self.causation_id = Some(id);
        self
    }

    /// Set the event source
    pub fn set_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set the user ID
    pub fn set_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Set the session ID
    pub fn set_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Add custom metadata
    pub fn add_custom(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.custom.insert(key.into(), value.into());
        self
    }

    /// Get custom metadata value
    pub fn get_custom(&self, key: &str) -> Option<&String> {
        self.custom.get(key)
    }

    /// Create a chain of events by setting causation from another metadata
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
