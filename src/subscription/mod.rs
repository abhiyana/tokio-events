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
    sender: tokio::sync::mpsc::Sender<Arc<EventEnvelope>>,
    handle: JoinHandle<()>,
}

/// Manages all active subscriptions in the event bus.
#[allow(missing_debug_implementations)]
pub struct SubscriptionManager {
    /// Registry for type-to-subscription mapping
    registry: Arc<dyn EventRegistry>,

    /// Active subscription data
    subscriptions: Arc<DashMap<Uuid, SubscriptionData>>,

    /// Channel for receiving events to dispatch
    event_receiver: Option<tokio::sync::mpsc::UnboundedReceiver<EventEnvelope>>,

    /// Sender for auto-unsubscribe requests
    unsub_tx: tokio::sync::mpsc::UnboundedSender<Uuid>,

    /// Maximum number of retries for failing handlers
    max_retries: u32,

    /// Base backoff duration for retries
    retry_backoff: std::time::Duration,

    /// Optional Dead Letter Queue (DLQ) sender for permanently failed events
    dlq_tx: Option<tokio::sync::mpsc::Sender<Arc<EventEnvelope>>>,

    /// Per-handler channel buffer size
    handler_channel_size: usize,
}

impl SubscriptionManager {
    /// Create a new subscription manager
    pub fn new(
        registry: Arc<dyn EventRegistry>,
        max_retries: u32,
        retry_backoff: std::time::Duration,
    ) -> Self {
        Self::with_channel_size(registry, max_retries, retry_backoff, 256)
    }

    /// Create a new subscription manager with custom handler channel size
    pub fn with_channel_size(
        registry: Arc<dyn EventRegistry>,
        max_retries: u32,
        retry_backoff: std::time::Duration,
        handler_channel_size: usize,
    ) -> Self {
        let subscriptions = Arc::new(DashMap::<Uuid, SubscriptionData>::new());
        let (unsub_tx, mut unsub_rx) = tokio::sync::mpsc::unbounded_channel();

        let subscriptions_clone = subscriptions.clone();
        let registry_clone = registry.clone();

        tokio::spawn(async move {
            while let Some(id) = unsub_rx.recv().await {
                if let Some((_, data)) = subscriptions_clone.remove(&id) {
                    let _ = registry_clone.unregister(id);
                    data.handle.abort();
                    debug!(subscription_id = %id, "Handler auto-unsubscribed");
                }
            }
        });

        Self {
            registry,
            subscriptions,
            event_receiver: None,
            unsub_tx,
            max_retries,
            retry_backoff,
            dlq_tx: None,
            handler_channel_size,
        }
    }

    /// Set the Dead Letter Queue (DLQ) sender
    pub fn set_dlq(&mut self, dlq_tx: tokio::sync::mpsc::Sender<Arc<EventEnvelope>>) {
        self.dlq_tx = Some(dlq_tx);
    }

    /// Set the event receiver channel
    pub fn set_event_receiver(
        &mut self,
        receiver: tokio::sync::mpsc::UnboundedReceiver<EventEnvelope>,
    ) {
        self.event_receiver = Some(receiver);
    }

    /// Shut down the subscription manager gracefully, waiting for all handlers to process buffered events
    pub async fn shutdown_gracefully(&self) {
        let mut handles = Vec::new();

        // Extract all subscriptions from the map
        let ids: Vec<_> = self.subscriptions.iter().map(|s| *s.key()).collect();
        for id in ids {
            if let Some((_, data)) = self.subscriptions.remove(&id) {
                // Drop the sender to close the channel
                drop(data.sender);
                handles.push(data.handle);
                let _ = self.registry.unregister(id);
            }
        }

        // Wait for all handlers to finish
        for handle in handles {
            let _ = handle.await;
        }
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
        let (handle, mut shutdown_rx) = SubscriptionHandle::with_name(Uuid::new_v4(), &name);

        debug!(
            subscription_id = %handle.id(),
            event_type = T::event_type(),
            handler_name = %name,
            "Subscribing handler"
        );

        let (tx, mut rx) = tokio::sync::mpsc::channel(self.handler_channel_size);
        let handler = Arc::new(handler);
        let sub_id = handle.id();
        let registry_clone = self.registry.clone();
        let unsub_tx = self.unsub_tx.clone();
        let max_retries = self.max_retries;
        let retry_backoff = self.retry_backoff;
        let dlq_tx = self.dlq_tx.clone();

        // Create subscription data
        let subscription_data = SubscriptionData {
            sender: tx,
            handle: tokio::spawn(async move {
                loop {
                    tokio::select! {
                        msg = rx.recv() => {
                            if let Some(envelope_clone) = msg {
                                let mut attempt = 0;
                                loop {
                                    trace!(
                                        subscription_id = %sub_id,
                                        attempt = attempt + 1,
                                        "Executing handler"
                                    );
                                    #[cfg(feature = "metrics")]
                                    let start_time = std::time::Instant::now();
                                    
                                    let result = handler.handle(&envelope_clone).await;
                                    
                                    #[cfg(feature = "metrics")]
                                    {
                                        let elapsed = start_time.elapsed().as_secs_f64();
                                        metrics::histogram!("tokio_events_dispatch_duration_seconds", "type" => envelope_clone.event_type().to_string()).record(elapsed);
                                    }

                                    match result {
                                        Ok(()) => {
                                            registry_clone.increment_processed(sub_id);
                                            registry_clone.ack_event(sub_id, envelope_clone.event_id());
                                            trace!(
                                                subscription_id = %sub_id,
                                                "Handler executed successfully"
                                            );
                                            
                                            #[cfg(feature = "metrics")]
                                            metrics::counter!("tokio_events_dispatched_total", "type" => envelope_clone.event_type().to_string()).increment(1);
                                            
                                            break;
                                        }
                                        Err(e) => {
                                            attempt += 1;
                                            
                                            #[cfg(feature = "metrics")]
                                            metrics::counter!("tokio_events_handler_errors_total", "type" => envelope_clone.event_type().to_string()).increment(1);
                                            
                                            if attempt > max_retries {
                                                error!(
                                                    subscription_id = %sub_id,
                                                    error = %e,
                                                    "Handler permanently failed after {} attempts",
                                                    attempt
                                                );
                                                
                                                // Send to DLQ if configured
                                                if let Some(dlq) = &dlq_tx {
                                                    let _ = dlq.send(envelope_clone.clone()).await;
                                                    
                                                    #[cfg(feature = "metrics")]
                                                    metrics::counter!("tokio_events_dlq_total", "type" => envelope_clone.event_type().to_string()).increment(1);
                                                }
                                                
                                                // We must still ack the event so it gets removed from the dispatcher/persistence
                                                // since we've now routed it to DLQ (or dropped it).
                                                registry_clone.ack_event(sub_id, envelope_clone.event_id());
                                                
                                                break;
                                            }

                                            // Exponential backoff
                                            let backoff = retry_backoff * (2u32.pow(attempt - 1));
                                            tracing::warn!(
                                                subscription_id = %sub_id,
                                                error = %e,
                                                "Handler failed, retrying in {:?} (attempt {}/{})",
                                                backoff,
                                                attempt,
                                                max_retries
                                            );
                                            tokio::time::sleep(backoff).await;
                                        }
                                    }
                                }
                            } else {
                                break;
                            }
                        }
                        _ = &mut shutdown_rx => {
                            break;
                        }
                    }
                }
                let _ = unsub_tx.send(sub_id);
            }),
        };

        // Store subscription
        self.subscriptions.insert(handle.id(), subscription_data);

        // Then register in the registry so publishers can discover it
        let entry = SubscriptionEntry::with_name(handle.id(), &name);
        self.registry
            .register(T::type_id(), T::event_type(), entry)?;

        debug!(
            subscription_id = %handle.id(),
            "Handler subscribed successfully"
        );

        Ok(handle)
    }

    /// Get a reference to the registry
    pub fn registry(&self) -> Arc<dyn EventRegistry> {
        self.registry.clone()
    }

    /// Subscribe an untyped handler (can handle any event type)
    pub async fn subscribe_untyped(
        &self,
        handler: impl EventHandler,
        event_type_id: TypeId,
        event_type_name: &'static str,
    ) -> Result<SubscriptionHandle> {
        let name = format!("Handler<{}>", event_type_name);
        let (handle, mut shutdown_rx) = SubscriptionHandle::with_name(Uuid::new_v4(), &name);

        debug!(
            subscription_id = %handle.id(),
            event_type = event_type_name,
            handler_name = %name,
            "Subscribing untyped handler"
        );

        // Defer registering in the registry until the end

        let (tx, mut rx) = tokio::sync::mpsc::channel(self.handler_channel_size);
        let handler = Arc::new(handler);
        let sub_id = handle.id();
        let registry_clone = self.registry.clone();
        let unsub_tx = self.unsub_tx.clone();
        let max_retries = self.max_retries;
        let retry_backoff = self.retry_backoff;
        let dlq_tx = self.dlq_tx.clone();

        // Create subscription data
        let subscription_data = SubscriptionData {
            sender: tx,
            handle: tokio::spawn(async move {
                loop {
                    tokio::select! {
                        msg = rx.recv() => {
                            if let Some(envelope_clone) = msg {
                                let mut attempt = 0;
                                loop {
                                    trace!(
                                        subscription_id = %sub_id,
                                        attempt = attempt + 1,
                                        "Executing untyped handler"
                                    );
                                    match handler.handle(&envelope_clone).await {
                                        Ok(()) => {
                                            registry_clone.increment_processed(sub_id);
                                            registry_clone.ack_event(sub_id, envelope_clone.event_id());
                                            trace!(
                                                subscription_id = %sub_id,
                                                "Handler executed successfully"
                                            );
                                            break;
                                        }
                                        Err(e) => {
                                            attempt += 1;
                                            if attempt > max_retries {
                                                error!(
                                                    subscription_id = %sub_id,
                                                    error = %e,
                                                    "Handler permanently failed after {} attempts",
                                                    attempt
                                                );
                                                
                                                if let Some(dlq) = &dlq_tx {
                                                    let _ = dlq.send(envelope_clone.clone()).await;
                                                }
                                                
                                                // We must still ack the event so it gets removed from the dispatcher/persistence
                                                // since we've now routed it to DLQ (or dropped it).
                                                registry_clone.ack_event(sub_id, envelope_clone.event_id());
                                                break;
                                            }

                                            let backoff = retry_backoff * (2u32.pow(attempt - 1));
                                            tracing::warn!(
                                                subscription_id = %sub_id,
                                                error = %e,
                                                "Handler failed, retrying in {:?} (attempt {}/{})",
                                                backoff,
                                                attempt,
                                                max_retries
                                            );
                                            tokio::time::sleep(backoff).await;
                                        }
                                    }
                                }
                            } else {
                                break;
                            }
                        }
                        _ = &mut shutdown_rx => {
                            break;
                        }
                    }
                }
                let _ = unsub_tx.send(sub_id);
            }),
        };

        // Store subscription
        self.subscriptions.insert(handle.id(), subscription_data);

        // Register in the registry
        let entry = SubscriptionEntry::with_name(handle.id(), &name);
        self.registry
            .register(event_type_id, event_type_name, entry)?;

        debug!(
            subscription_id = %handle.id(),
            "Untyped handler subscribed successfully"
        );

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
            println!("SM: No subscriptions found for event type: {}", envelope.event_type());
            trace!("No subscriptions for event type");
            return Ok(());
        }

        println!("SM: Found {} subscriptions for event {}", subscriptions.len(), envelope.event_type());

        debug!(
            event_id = %envelope.event_id(),
            subscription_count = subscriptions.len(),
            "Found subscriptions for event"
        );

        // Collect handlers before spawning tasks
        let senders: Vec<(Uuid, tokio::sync::mpsc::Sender<Arc<EventEnvelope>>)> = subscriptions
            .into_iter()
            .filter_map(|sub_entry| {
                self.subscriptions
                    .get(&sub_entry.id)
                    .map(|sub_data| (sub_entry.id, sub_data.sender.clone()))
            })
            .collect();

        // Dispatch to all handlers concurrently using channel backpressure
        let mut sends = Vec::new();

        for (sub_id, sender) in senders {
            println!("SM: dispatching to subscription {}", sub_id);
            let envelope_clone = envelope.clone();
            sends.push(async move {
                if let Err(e) = sender.send(envelope_clone).await {
                    warn!("Failed to dispatch event to subscription {}: {}", sub_id, e);
                }
            });
        }

        // Wait for all sends to complete (handles backpressure if full)
        futures::future::join_all(sends).await;

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
    /// The number of currently active subscriptions.
    pub active_subscriptions: usize,
    /// The total number of event types registered.
    pub total_event_types: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::DashMapRegistry;

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
        let manager = SubscriptionManager::new(
            registry.clone(),
            0, // No retries for test
            std::time::Duration::from_millis(10),
        );

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
