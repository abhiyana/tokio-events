//! Subscription management for event handlers.
//!
//! This module provides the infrastructure for managing event subscriptions,
//! including handler registration, lifecycle management, and execution.

use crate::registry::{EventRegistry, SubscriptionEntry};
use crate::{Error, Event, EventEnvelope, Result};
use dashmap::DashMap;
use std::any::TypeId;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{debug, error, trace, warn};
use uuid::Uuid;

pub mod handle;
pub mod handler;

pub use handle::{SubscriptionBuilder, SubscriptionHandle};
pub use handler::{EventHandler, FilteredHandler, FunctionHandler, TypedHandler};

/// Internal subscription data
struct SubscriptionData {
    handler: Arc<dyn EventHandler>,
    handle: JoinHandle<()>,
}

/// Manages all active subscriptions in the event bus.
pub struct SubscriptionManager {
    /// Registry for type-to-subscription mapping
    registry: Arc<dyn EventRegistry>,

    /// Active subscription data
    subscriptions: Arc<DashMap<Uuid, SubscriptionData>>,

    /// Channel for receiving events to dispatch
    event_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<EventEnvelope>>,
}

impl SubscriptionManager {
    /// Create a new subscription manager
    pub fn new(registry: Arc<dyn EventRegistry>) -> Self {
        Self {
            registry,
            subscriptions: Arc::new(DashMap::new()),
            event_receiver: None,
        }
    }

    /// Set the event receiver channel
    pub fn set_event_receiver(
        &mut self,
        receiver: tokio::sync::mpsc::UnboundedReceiver<EventEnvelope>,
    ) {
        self.event_receiver = Some(receiver);
    }

    /// Subscribe a handler to events of type T
    pub async fn subscribe<T, H>(&self, handler: H) -> Result<SubscriptionHandle>
    where
        T: Event,
        H: EventHandler,
    {
        self.subscribe_typed::<T, H>(handler, format!("Handler<{}>", T::event_type()))
            .await
    }

    /// Subscribe a typed handler with a custom name
    pub async fn subscribe_typed<T, H>(
        &self,
        handler: H,
        name: impl Into<String>,
    ) -> Result<SubscriptionHandle>
    where
        T: Event,
        H: EventHandler,
    {
        let name = name.into();
        let (handle, _shutdown_rx) = SubscriptionHandle::with_name(Uuid::new_v4(), &name);

        debug!(
            subscription_id = %handle.id(),
            event_type = T::event_type(),
            handler_name = %name,
            "Subscribing handler"
        );

        // Register in the registry
        let entry = SubscriptionEntry::with_name(handle.id(), &name);
        self.registry.register(T::type_id(), entry)?;

        // Create subscription data
        let subscription_data = SubscriptionData {
            handler: Arc::new(handler),
            handle: tokio::spawn(async move {
                // Placeholder task
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
                }
            }),
        };

        // Store subscription
        self.subscriptions.insert(handle.id(), subscription_data);

        debug!(
            subscription_id = %handle.id(),
            "Handler subscribed successfully"
        );

        Ok(handle)
    }

    /// Subscribe an untyped handler (can handle any event type)
    pub async fn subscribe_untyped(
        &self,
        handler: impl EventHandler,
        event_type_id: TypeId,
        event_type_name: &'static str,
    ) -> Result<SubscriptionHandle> {
        let name = format!("Handler<{}>", event_type_name);
        let (handle, _shutdown_rx) = SubscriptionHandle::with_name(Uuid::new_v4(), &name);

        debug!(
            subscription_id = %handle.id(),
            event_type = event_type_name,
            handler_name = %name,
            "Subscribing untyped handler"
        );

        // Register in the registry
        let entry = SubscriptionEntry::with_name(handle.id(), &name);
        self.registry.register(event_type_id, entry)?;

        // Create subscription data
        let subscription_data = SubscriptionData {
            handler: Arc::new(handler),
            handle: tokio::spawn(async move {
                // Placeholder task
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
                }
            }),
        };

        // Store subscription
        self.subscriptions.insert(handle.id(), subscription_data);

        Ok(handle)
    }

    /// Subscribe a function as an event handler
    pub async fn subscribe_fn<T, F, Fut>(&self, f: F) -> Result<SubscriptionHandle>
    where
        T: Event,
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let handler = FunctionHandler::new(f);
        self.subscribe::<T, _>(handler).await
    }

    /// Unsubscribe a handler
    pub async fn unsubscribe(&self, handle: SubscriptionHandle) -> Result<()> {
        debug!(subscription_id = %handle.id(), "Unsubscribing handler");

        // Remove from registry
        self.registry.unregister(handle.id())?;

        // Remove subscription data
        if let Some((_, data)) = self.subscriptions.remove(&handle.id()) {
            // Cancel the task
            data.handle.abort();

            debug!(subscription_id = %handle.id(), "Handler unsubscribed");
            Ok(())
        } else {
            Err(Error::SubscriptionNotFound { id: handle.id() })
        }
    }

    /// Dispatch an event to all registered handlers
    pub async fn dispatch(&self, envelope: Arc<EventEnvelope>) -> Result<()> {
        trace!(
            event_id = %envelope.event_id(),
            event_type = %envelope.event_type(),
            "Dispatching event"
        );

        // Get all subscriptions for this event type
        let event_type = envelope.type_id();
        let subscriptions = self.registry.get_subscriptions(event_type);

        if subscriptions.is_empty() {
            trace!("No subscriptions for event type");
            return Ok(());
        }

        debug!(
            event_id = %envelope.event_id(),
            subscription_count = subscriptions.len(),
            "Found subscriptions for event"
        );

        // Collect handlers before spawning tasks
        let handlers: Vec<(Uuid, Arc<dyn EventHandler>)> = subscriptions
            .into_iter()
            .filter_map(|sub_entry| {
                self.subscriptions
                    .get(&sub_entry.id)
                    .map(|sub_data| (sub_entry.id, sub_data.handler.clone()))
            })
            .collect();

        // Dispatch to all handlers concurrently
        let mut tasks = Vec::new();

        for (sub_id, handler) in handlers {
            let envelope_clone = envelope.clone();
            let registry = self.registry.clone();

            // Spawn task for each handler
            let task = tokio::spawn(async move {
                trace!(subscription_id = %sub_id, "Executing handler");

                match handler.handle(&envelope_clone).await {
                    Ok(()) => {
                        registry.increment_processed(sub_id);
                        trace!(subscription_id = %sub_id, "Handler executed successfully");
                    }
                    Err(e) => {
                        error!(
                            subscription_id = %sub_id,
                            error = %e,
                            "Handler execution failed"
                        );
                    }
                }
            });

            tasks.push(task);
        }

        // Wait for all handlers to complete
        for task in tasks {
            if let Err(e) = task.await {
                warn!("Handler task failed: {}", e);
            }
        }

        Ok(())
    }

    /// Get statistics about subscriptions
    pub fn stats(&self) -> SubscriptionStats {
        SubscriptionStats {
            active_subscriptions: self.subscriptions.len(),
            total_event_types: self.registry.event_types().len(),
        }
    }

    /// Shutdown all subscriptions
    pub async fn shutdown(&self) -> Result<()> {
        debug!("Shutting down subscription manager");

        // Clear registry
        self.registry.clear();

        // Cancel all subscription tasks
        for entry in self.subscriptions.iter() {
            entry.value().handle.abort();
        }

        // Clear subscriptions
        self.subscriptions.clear();

        debug!("Subscription manager shut down");
        Ok(())
    }
}

/// Statistics about the subscription manager
#[derive(Debug, Clone)]
pub struct SubscriptionStats {
    pub active_subscriptions: usize,
    pub total_event_types: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::DashMapRegistry;

    #[derive(Debug, Clone)]
    struct TestEvent {
        message: String,
    }

    impl Event for TestEvent {
        fn event_type() -> &'static str {
            "TestEvent"
        }
    }

    #[tokio::test]
    async fn test_subscription_manager() {
        let registry = Arc::new(DashMapRegistry::new());
        let manager = SubscriptionManager::new(registry.clone());

        // Subscribe a function handler
        let counter = Arc::new(tokio::sync::Mutex::new(0));
        let counter_clone = counter.clone();

        let handle = manager
            .subscribe_fn::<TestEvent, _, _>(move |event| {
                let counter = counter_clone.clone();
                async move {
                    let mut count = counter.lock().await;
                    *count += 1;
                    println!("Received: {}", event.message);
                }
            })
            .await
            .unwrap();

        // Dispatch an event
        let event = TestEvent {
            message: "Hello".to_string(),
        };
        let envelope = Arc::new(EventEnvelope::new(event));

        manager.dispatch(envelope).await.unwrap();

        // Check that handler was called
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        assert_eq!(*counter.lock().await, 1);

        // Unsubscribe
        manager.unsubscribe(handle).await.unwrap();

        // Verify stats
        let stats = manager.stats();
        assert_eq!(stats.active_subscriptions, 0);
    }
}
