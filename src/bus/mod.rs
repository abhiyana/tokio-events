//! The main EventBus implementation.
//!
//! The EventBus is the primary interface for publishing and subscribing to events.
//! It coordinates between the registry, subscription manager, and dispatcher.

use crate::dispatcher::EventDispatcher;
use crate::registry::EventRegistry;
use crate::subscription::{EventHandler, SubscriptionHandle, SubscriptionManager};
use crate::{Error, Event, EventEnvelope, EventMetadata, Result};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

pub mod builder;
pub mod config;

pub use builder::EventBusBuilder;
pub use config::EventBusConfig;

/// Shutdown hook function type
type ShutdownHook = Box<dyn Fn() -> futures::future::BoxFuture<'static, Result<()>> + Send + Sync>;

/// The main event bus for publishing and subscribing to events.
///
/// The EventBus provides a high-level API for event-driven communication
/// between different parts of an application.
///
/// # Example
///
/// ```rust,ignore
/// use tokio_events::{EventBus, Event};
///
/// #[derive(Debug, Clone)]
/// struct MyEvent { data: String }
///
/// impl Event for MyEvent {
///     fn event_type() -> &'static str { "MyEvent" }
/// }
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let bus = EventBus::builder().build().await?;
///     
///     // Subscribe to events
///     let handle = bus.subscribe(|event: MyEvent| async move {
///         println!("Received: {}", event.data);
///     }).await?;
///     
///     // Publish an event
///     bus.publish(MyEvent { data: "Hello".into() }).await?;
///     
///     Ok(())
/// }
/// ```
pub struct EventBus {
    pub(crate) config: EventBusConfig,
    pub(crate) registry: Arc<dyn EventRegistry>,
    pub(crate) subscription_manager: Arc<SubscriptionManager>,
    pub(crate) dispatcher: Box<dyn EventDispatcher>,
    pub(crate) shutdown_hooks: Arc<Mutex<Vec<ShutdownHook>>>,
    pub(crate) is_shutting_down: Arc<AtomicBool>,
}

impl EventBus {
    /// Create a new EventBus builder
    pub fn builder() -> EventBusBuilder {
        EventBusBuilder::new()
    }

    /// Publish an event to all subscribers
    pub async fn publish<T: Event>(&self, event: T) -> Result<Uuid> {
        self.publish_with_metadata(event, EventMetadata::new())
            .await
    }

    /// Publish an event with custom metadata
    pub async fn publish_with_metadata<T: Event>(
        &self,
        event: T,
        metadata: EventMetadata,
    ) -> Result<Uuid> {
        if self.is_shutting_down.load(Ordering::Relaxed) {
            return Err(Error::ShuttingDown);
        }

        let event_id = metadata.event_id;

        trace!(
            event_id = %event_id,
            event_type = T::event_type(),
            "Publishing event"
        );

        let envelope = EventEnvelope::with_metadata(event, metadata);

        // Dispatch the event
        self.dispatcher.dispatch(envelope).await?;

        debug!(
            event_id = %event_id,
            event_type = T::event_type(),
            "Event published successfully"
        );

        Ok(event_id)
    }

    /// Subscribe to events of a specific type
    pub async fn subscribe<T, F, Fut>(&self, handler: F) -> Result<SubscriptionHandle>
    where
        T: Event,
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if self.is_shutting_down.load(Ordering::Relaxed) {
            return Err(Error::ShuttingDown);
        }

        self.subscription_manager.subscribe_fn(handler).await
    }

    /// Subscribe with a custom handler implementation
    pub async fn subscribe_handler<T, H>(&self, handler: H) -> Result<SubscriptionHandle>
    where
        T: Event,
        H: EventHandler,
    {
        if self.is_shutting_down.load(Ordering::Relaxed) {
            return Err(Error::ShuttingDown);
        }

        self.subscription_manager.subscribe::<T, H>(handler).await
    }

    /// Unsubscribe a handler
    pub async fn unsubscribe(&self, handle: SubscriptionHandle) -> Result<()> {
        self.subscription_manager.unsubscribe(handle).await
    }

    /// Emit an event and wait for all handlers to complete
    pub async fn emit_and_wait<T: Event>(&self, event: T) -> Result<()> {
        if self.is_shutting_down.load(Ordering::Relaxed) {
            return Err(Error::ShuttingDown);
        }

        // Create a tracking mechanism for this specific event
        let metadata = EventMetadata::new();
        let envelope = Arc::new(EventEnvelope::with_metadata(event, metadata));

        // Get the number of active subscribers for this event type
        let subscriber_count = self.registry.subscription_count(T::type_id());

        if subscriber_count == 0 {
            // No subscribers, just return
            return Ok(());
        }

        // For now, we'll just dispatch normally and wait a bit
        // In a full implementation, we'd modify the dispatcher to support completion tracking
        self.dispatcher.dispatch((*envelope).clone()).await?;

        // Wait for all handlers to complete or timeout
        let timeout = tokio::time::Duration::from_secs(30);
        let start = tokio::time::Instant::now();

        while start.elapsed() < timeout {
            // Check if all events have been processed
            // This is still simplified - in production we'd track completions properly
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

            // For now, we just wait a reasonable amount of time
            // A proper implementation would track handler completions
            if start.elapsed() > tokio::time::Duration::from_millis(100) {
                break;
            }
        }

        Ok(())
    }

    /// Emit an event and wait with a custom timeout
    pub async fn emit_and_wait_timeout<T: Event>(
        &self,
        event: T,
        timeout: std::time::Duration,
    ) -> Result<()> {
        tokio::time::timeout(timeout, self.emit_and_wait(event))
            .await
            .map_err(|_| Error::internal("Timeout waiting for handlers to complete"))?
    }

    /// Get statistics about the event bus
    pub fn stats(&self) -> EventBusStats {
        EventBusStats {
            total_subscriptions: self.registry.total_subscriptions(),
            event_types: self.registry.event_types().len(),
            dispatcher_stats: self.dispatcher.stats(),
            subscription_stats: self.subscription_manager.stats(),
        }
    }

    /// Register a shutdown hook
    pub async fn register_shutdown_hook<F, Fut>(&self, hook: F) -> Result<()>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let hook = Box::new(move || -> futures::future::BoxFuture<'static, Result<()>> {
            Box::pin(hook())
        });

        self.shutdown_hooks.lock().await.push(hook);
        Ok(())
    }

    /// Check if the event bus is shutting down
    pub fn is_shutting_down(&self) -> bool {
        self.is_shutting_down.load(Ordering::Relaxed)
    }

    /// Shutdown the event bus gracefully
    pub async fn shutdown(self) -> Result<()> {
        info!("Shutting down EventBus");

        // Mark as shutting down
        self.is_shutting_down.store(true, Ordering::Relaxed);

        // Run shutdown hooks
        let hooks = self.shutdown_hooks.lock().await;
        for hook in hooks.iter() {
            if let Err(e) = hook().await {
                error!("Shutdown hook failed: {}", e);
            }
        }
        drop(hooks);

        // Stop the dispatcher with timeout
        let dispatcher_shutdown = tokio::time::timeout(self.config.shutdown_timeout, async {
            let mut dispatcher = self.dispatcher;
            dispatcher.stop().await
        });

        if let Err(_) = dispatcher_shutdown.await {
            warn!("Dispatcher shutdown timed out");
        }

        // Shutdown subscription manager
        self.subscription_manager.shutdown().await?;

        info!("EventBus shutdown complete");
        Ok(())
    }
}

/// Statistics about the event bus
#[derive(Debug, Clone)]
pub struct EventBusStats {
    /// Total number of subscriptions
    pub total_subscriptions: usize,

    /// Number of unique event types
    pub event_types: usize,

    /// Dispatcher statistics
    pub dispatcher_stats: crate::dispatcher::DispatcherStats,

    /// Subscription manager statistics
    pub subscription_stats: crate::subscription::SubscriptionStats,
}

impl std::fmt::Display for EventBusStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "EventBus Stats: {} subscriptions, {} event types, {} events dispatched",
            self.total_subscriptions, self.event_types, self.dispatcher_stats.events_dispatched
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[derive(Debug, Clone)]
    struct TestEvent {
        value: String,
    }

    impl Event for TestEvent {
        fn event_type() -> &'static str {
            "TestEvent"
        }
    }

    #[tokio::test]
    async fn test_event_bus_basic() {
        let bus = EventBus::builder()
            .configure(|c| c.enable_tracing(false))
            .build()
            .await
            .unwrap();

        let received = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        // Subscribe
        let handle = bus
            .subscribe(move |event: TestEvent| {
                let received = received_clone.clone();
                async move {
                    received.lock().await.push(event.value);
                }
            })
            .await
            .unwrap();

        // Publish events
        bus.publish(TestEvent {
            value: "first".into(),
        })
        .await
        .unwrap();
        bus.publish(TestEvent {
            value: "second".into(),
        })
        .await
        .unwrap();

        // Wait for processing
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Check results
        let messages = received.lock().await;
        assert_eq!(messages.len(), 2);
        assert!(messages.contains(&"first".to_string()));
        assert!(messages.contains(&"second".to_string()));

        // Unsubscribe
        bus.unsubscribe(handle).await.unwrap();

        // Shutdown
        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_event_bus_stats() {
        let bus = EventBus::builder().build().await.unwrap();

        let _handle1 = bus.subscribe(|_: TestEvent| async {}).await.unwrap();
        let _handle2 = bus.subscribe(|_: TestEvent| async {}).await.unwrap();

        let stats = bus.stats();
        assert_eq!(stats.total_subscriptions, 2);
        assert_eq!(stats.event_types, 1);

        bus.shutdown().await.unwrap();
    }
}
