//! DashMap-based implementation of EventRegistry for concurrent access.

use super::{EventRegistry, SubscriptionEntry};
use crate::{Error, Result};
use dashmap::DashMap;
use std::any::TypeId;
use std::sync::Arc;
use tracing::{debug, trace};
use uuid::Uuid;

/// A thread-safe event registry implementation using DashMap.
///
/// where we have many readers (getting subscriptions) and fewer writers
/// (registering/unregistering).
#[derive(Debug, Clone)]
pub struct DashMapRegistry {
    /// Map from event TypeId to list of subscriptions
    subscriptions: Arc<DashMap<TypeId, Vec<SubscriptionEntry>>>,
    
    /// Map from subscription ID to event TypeId for faster lookups
    subscription_to_type: Arc<DashMap<Uuid, TypeId>>,
}

impl DashMapRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            subscriptions: Arc::new(DashMap::new()),
            subscription_to_type: Arc::new(DashMap::new()),
        }
    }
    
    /// Create a registry with pre-allocated capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            subscriptions: Arc::new(DashMap::with_capacity(capacity)),
            subscription_to_type: Arc::new(DashMap::with_capacity(capacity * 10)), // Assume ~10 subs per type
        }
    }
}

impl Default for DashMapRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl EventRegistry for DashMapRegistry {
    fn register(&self, event_type: TypeId, subscription: SubscriptionEntry) -> Result<()> {
        trace!(
            subscription_id = %subscription.id,
            ?event_type,
            "Registering subscription"
        );
        
        // Store the type mapping
        self.subscription_to_type.insert(subscription.id, event_type);
        
        // Add to subscriptions list
        self.subscriptions
            .entry(event_type)
            .or_default()
            .push(subscription.clone());
        
        debug!(
            subscription_id = %subscription.id,
            "Subscription registered successfully"
        );
    
        Ok(())
    }
    
    fn unregister(&self, subscription_id: Uuid) -> Result<()> {
        trace!(subscription_id = %subscription_id, "Unregistering subscription");
        
        // Find the event type for this subscription
        let event_type = self.subscription_to_type
            .remove(&subscription_id)
            .map(|(_, type_id)| type_id)
            .ok_or(Error::SubscriptionNotFound { id: subscription_id })?;
        
        // Remove from subscriptions list
        if let Some(mut subs) = self.subscriptions.get_mut(&event_type) {
            subs.retain(|s| s.id != subscription_id);
            
            // If no more subscriptions for this type, remove the entry
            if subs.is_empty() {
                drop(subs);
                self.subscriptions.remove(&event_type);
            }
        }
        
        debug!(subscription_id = %subscription_id, "Subscription unregistered");
        Ok(())
    }
    
    fn get_subscriptions(&self, event_type: TypeId) -> Vec<SubscriptionEntry> {
        self.subscriptions
            .get(&event_type)
            .map(|subs| {
                subs.iter()
                    .filter(|s| s.active)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
    
    fn get_subscription(&self, subscription_id: Uuid) -> Option<SubscriptionEntry> {
        // First find the event type
        let event_type = self.subscription_to_type.get(&subscription_id)?;
        
        // Then find the subscription
        self.subscriptions
            .get(&event_type)
            .and_then(|subs| {
                subs.iter()
                    .find(|s| s.id == subscription_id)
                    .cloned()
            })
    }
    
    fn increment_processed(&self, subscription_id: Uuid) {
        if let Some(event_type) = self.subscription_to_type.get(&subscription_id) {
            if let Some(mut subs) = self.subscriptions.get_mut(&*event_type) {
                if let Some(sub) = subs.iter_mut().find(|s| s.id == subscription_id) {
                    sub.events_processed += 1;
                }
            }
        }
    }
    
    fn deactivate(&self, subscription_id: Uuid) -> Result<()> {
        trace!(subscription_id = %subscription_id, "Deactivating subscription");
        
        let event_type = self.subscription_to_type
            .get(&subscription_id)
            .ok_or(Error::SubscriptionNotFound { id: subscription_id })?;
        
        if let Some(mut subs) = self.subscriptions.get_mut(&*event_type) {
            if let Some(sub) = subs.iter_mut().find(|s| s.id == subscription_id) {
                sub.active = false;
                debug!(subscription_id = %subscription_id, "Subscription deactivated");
                Ok(())
            } else {
                Err(Error::SubscriptionNotFound { id: subscription_id })
            }
        } else {
            Err(Error::SubscriptionNotFound { id: subscription_id })
        }
    }
    
    fn total_subscriptions(&self) -> usize {
        self.subscription_to_type.len()
    }
    
    fn subscription_count(&self, event_type: TypeId) -> usize {
        self.subscriptions
            .get(&event_type)
            .map(|subs| subs.len())
            .unwrap_or(0)
    }
    
    fn event_types(&self) -> Vec<TypeId> {
        self.subscriptions
            .iter()
            .map(|entry| *entry.key())
            .collect()
    }
    
    fn clear(&self) {
        self.subscriptions.clear();
        self.subscription_to_type.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Event;
    
    #[derive(Debug, Clone)]
    struct TestEvent;
    
    impl Event for TestEvent {
        fn event_type() -> &'static str {
            "TestEvent"
        }
    }
    
    #[derive(Debug, Clone)]
    struct AnotherEvent;
    
    impl Event for AnotherEvent {
        fn event_type() -> &'static str {
            "AnotherEvent"
        }
    }
    
    #[test]
    fn test_register_and_get() {
        let registry = DashMapRegistry::new();
        let sub_id = Uuid::max();
        let subscription = SubscriptionEntry::new(sub_id);
        
        // Register subscription
        registry.register(TestEvent::type_id(), subscription).unwrap();
        
        // Get subscriptions for the event type
        let subs = registry.get_subscriptions(TestEvent::type_id());
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].id, sub_id);
    }
    
    #[test]
    fn test_unregister() {
        let registry = DashMapRegistry::new();
        let sub_id = Uuid::max();
        let subscription = SubscriptionEntry::new(sub_id);
        
        // Register and then unregister
        registry.register(TestEvent::type_id(), subscription).unwrap();
        assert_eq!(registry.total_subscriptions(), 1);
        
        registry.unregister(sub_id).unwrap();
        assert_eq!(registry.total_subscriptions(), 0);
        
        // Should have no subscriptions for this type
        let subs = registry.get_subscriptions(TestEvent::type_id());
        assert!(subs.is_empty());
    }
    
    #[test]
    fn test_multiple_subscriptions() {
        let registry = DashMapRegistry::new();
        
        // Register multiple subscriptions for same event
        for i in 0..3 {
            let sub = SubscriptionEntry::with_name(Uuid::new_v4(), format!("handler-{}", i));
            registry.register(TestEvent::type_id(), sub).unwrap();
        }
        
        // Register subscription for different event
        let other_sub = SubscriptionEntry::new(Uuid::new_v4());
        registry.register(AnotherEvent::type_id(), other_sub).unwrap();
        
        assert_eq!(registry.subscription_count(TestEvent::type_id()), 3);
        assert_eq!(registry.subscription_count(AnotherEvent::type_id()), 1);
        assert_eq!(registry.total_subscriptions(), 4);
        assert_eq!(registry.event_types().len(), 2);
    }
    
    #[test]
    fn test_deactivate() {
        let registry = DashMapRegistry::new();
        let sub_id = Uuid::max();
        let subscription = SubscriptionEntry::new(sub_id);
        
        registry.register(TestEvent::type_id(), subscription).unwrap();
        
        // Deactivate the subscription
        registry.deactivate(sub_id).unwrap();
        
        // Should not appear in active subscriptions
        let active_subs = registry.get_subscriptions(TestEvent::type_id());
        assert!(active_subs.is_empty());
        
        // But should still exist in registry
        let sub = registry.get_subscription(sub_id).unwrap();
        assert!(!sub.active);
    }
    
    #[test]
    fn test_increment_processed() {
        let registry = DashMapRegistry::new();
        let sub_id = Uuid::max();
        let subscription = SubscriptionEntry::new(sub_id);
        
        registry.register(TestEvent::type_id(), subscription).unwrap();
        
        // Increment counter
        registry.increment_processed(sub_id);
        registry.increment_processed(sub_id);
        
        let sub = registry.get_subscription(sub_id).unwrap();
        assert_eq!(sub.events_processed, 2);
    }
}