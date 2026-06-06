//! Event registry for mapping event types to subscribers.
//!
//! The registry is responsible for maintaining the mapping between
//! event types and their subscribers in a thread-safe manner.

use crate::Result;
use std::{any::TypeId, fmt::Debug};
use uuid::Uuid;

pub mod dashmap;
pub(crate) mod topic_trie;

pub use dashmap::DashMapRegistry;

use crate::subscription::EventFilterFn;

/// A subscription entry in the registry
#[derive(Clone)]
pub struct SubscriptionEntry {
    /// Unique ID for this subscription
    pub id: Uuid,

    /// Optional name for debugging
    pub name: Option<String>,

    /// Whether this subscription is currently active
    pub active: bool,

    /// Number of events processed by this subscription
    pub events_processed: u64,

    /// Optional durable name for persistent queues
    pub durable_name: Option<String>,

    /// Optional topic/subject for wildcard routing
    pub topic: Option<String>,

    /// Optional filter
    pub filter: Option<EventFilterFn>,
}

impl Debug for SubscriptionEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubscriptionEntry")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("active", &self.active)
            .field("events_processed", &self.events_processed)
            .field("durable_name", &self.durable_name)
            .field("topic", &self.topic)
            .field("filter", &self.filter.is_some())
            .finish()
    }
}

impl SubscriptionEntry {
    /// Create a new subscription entry
    pub fn new(id: Uuid) -> Self {
        Self {
            id,
            name: None,
            active: true,
            events_processed: 0,
            durable_name: None,
            topic: None,
            filter: None,
        }
    }

    /// Create a new subscription entry with a name
    pub fn with_name(id: Uuid, name: impl Into<String>) -> Self {
        Self {
            id,
            name: Some(name.into()),
            active: true,
            events_processed: 0,
            durable_name: None,
            topic: None,
            filter: None,
        }
    }

    /// Set a durable name for persistent queues
    pub fn with_durable_name(mut self, name: impl Into<String>) -> Self {
        self.durable_name = Some(name.into());
        self
    }

    /// Set a topic pattern for routing
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// Set a filter for evaluating events before queuing
    pub fn with_filter(
        mut self,
        filter: EventFilterFn,
    ) -> Self {
        self.filter = Some(filter);
        self
    }
}

/// Trait for event registries that map event types to subscribers.
///
/// Implementations must be thread-safe as they will be accessed
/// concurrently from multiple tasks.
pub trait EventRegistry: Send + Sync + Debug {
    /// Register a new subscription for a specific event type
    fn register(
        &self,
        event_type: TypeId,
        type_name: &str,
        subscription: SubscriptionEntry,
    ) -> Result<()>;

    /// Unregister a subscription
    fn unregister(&self, subscription_id: Uuid) -> Result<()>;

    /// Get all active subscriptions for a specific event type
    fn get_subscriptions(&self, event_type: TypeId) -> Vec<SubscriptionEntry>;

    /// Get all active subscriptions matching a literal topic
    fn get_topic_subscriptions(&self, literal_topic: &str) -> Vec<SubscriptionEntry> {
        let _ = literal_topic;
        Vec::new() // Default impl for backwards compatibility
    }

    /// Get a specific subscription by ID
    fn get_subscription(&self, subscription_id: Uuid) -> Option<SubscriptionEntry>;

    /// Update subscription statistics
    fn increment_processed(&self, subscription_id: Uuid);

    /// Mark a subscription as inactive
    fn deactivate(&self, subscription_id: Uuid) -> Result<()>;

    /// Get total number of subscriptions across all event types
    fn total_subscriptions(&self) -> usize;

    /// Get number of subscriptions for a specific event type
    fn subscription_count(&self, event_type: TypeId) -> usize;

    /// Get number of subscriptions matching a literal topic
    fn topic_subscription_count(&self, literal_topic: &str) -> usize {
        self.get_topic_subscriptions(literal_topic).len()
    }

    /// Get all registered event types
    fn event_types(&self) -> Vec<TypeId>;

    /// Clear all subscriptions (useful for testing)
    fn clear(&self);

    /// Get the TypeId for a given event type name (useful for persistence layer mapping)
    fn get_type_id(&self, type_name: &str) -> Option<TypeId> {
        let _ = type_name; // To satisfy unused variable in default impl
        None
    }

    /// Acknowledge that an event was successfully processed
    fn ack_event(&self, _subscription_id: Uuid, _event_id: Uuid) {
        // Default implementation does nothing
    }
}

/// Registry statistics for monitoring
#[derive(Debug, Clone, Default)]
pub struct RegistryStats {
    /// Total number of event types
    pub event_types: usize,

    /// Total number of subscriptions
    pub total_subscriptions: usize,

    /// Number of active subscriptions
    pub active_subscriptions: usize,

    /// Number of inactive subscriptions
    pub inactive_subscriptions: usize,
}

/// Extension trait for registries with statistics
pub trait RegistryStatistics: EventRegistry {
    /// Get current registry statistics
    fn stats(&self) -> RegistryStats {
        let event_types = self.event_types();
        let mut total = 0;
        let mut active = 0;
        let mut inactive = 0;

        for event_type in &event_types {
            let subs = self.get_subscriptions(*event_type);
            total += subs.len();
            active += subs.iter().filter(|s| s.active).count();
            inactive += subs.iter().filter(|s| !s.active).count();
        }

        RegistryStats {
            event_types: event_types.len(),
            total_subscriptions: total,
            active_subscriptions: active,
            inactive_subscriptions: inactive,
        }
    }
}

// Implement statistics for all registries
impl<T: EventRegistry> RegistryStatistics for T {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test the `SubscriptionEntry::new` method.
    ///
    /// This test verifies that a new `SubscriptionEntry` is correctly
    /// initialized with the given UUID. It checks that the `id` field
    /// matches the provided UUID, the `active` field is set to true,
    /// the `events_processed` field is initialized to 0, and the `name`
    /// field is `None`.
    #[test]
    fn test_subscription_entry() {
        let id = Uuid::max();
        let entry = SubscriptionEntry::new(id);

        assert_eq!(entry.id, id);
        assert!(entry.active);
        assert_eq!(entry.events_processed, 0);
        assert!(entry.name.is_none());
    }

    #[test]
    fn test_subscription_entry_with_name() {
        let id = Uuid::max();
        let entry = SubscriptionEntry::with_name(id, "test-handler");

        assert_eq!(entry.name, Some("test-handler".to_string()));
    }
}
