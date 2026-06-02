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
/// # Sharing
///
/// `EventBus` is designed to be shared across tasks. Wrap it in `Arc<EventBus>`
/// and clone the `Arc` to pass it around. Shutdown works via `&self`.
///
/// # Example
///
/// ```rust,ignore
/// use tokio_events::{EventBus, Event};
/// use std::sync::Arc;
///
/// #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
/// struct MyEvent { data: String }
///
/// impl Event for MyEvent {
///     fn event_type() -> &'static str { "MyEvent" }
/// }
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let bus = Arc::new(EventBus::builder().build().await?);
///     
///     // Subscribe to events
///     let handle = bus.subscribe(|event: MyEvent| async move {
///         println!("Received: {}", event.data);
///     }).await?;
///     
///     // Publish an event
///     bus.publish(MyEvent { data: "Hello".into() }).await?;
///     
///     // Shutdown works via &self — no need to unwrap the Arc
///     bus.shutdown_gracefully().await?;
///     
///     Ok(())
/// }
/// ```
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct EventBus {
    pub(crate) config: EventBusConfig,
    pub(crate) registry: Arc<dyn EventRegistry>,
    pub(crate) subscription_manager: Arc<SubscriptionManager>,
    pub(crate) dispatcher: Arc<tokio::sync::Mutex<Option<Box<dyn EventDispatcher>>>>,
    pub(crate) shutdown_hooks: Arc<Mutex<Vec<ShutdownHook>>>,
    pub(crate) is_shutting_down: Arc<AtomicBool>,
    pub(crate) dlq_rx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<Arc<EventEnvelope>>>>>,
    
    #[cfg(feature = "remote")]
    pub(crate) remote_transport: Option<Arc<dyn crate::remote::RemoteTransport>>,
}

impl EventBus {
    /// Create a new EventBus builder
    pub fn builder() -> EventBusBuilder {
        EventBusBuilder::new()
    }

    /// Take the Dead Letter Queue (DLQ) receiver.
    /// 
    /// This can only be called once. Returns `None` if it has already been taken.
    pub async fn take_dlq_receiver(&self) -> Option<tokio::sync::mpsc::Receiver<Arc<EventEnvelope>>> {
        self.dlq_rx.lock().await.take()
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
        if self.is_shutting_down.load(Ordering::SeqCst) {
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
        {
            let guard = self.dispatcher.lock().await;
            let dispatcher = guard.as_ref().ok_or(Error::ShuttingDown)?;
            dispatcher.dispatch(envelope).await?;
        }

        debug!(
            event_id = %event_id,
            event_type = T::event_type(),
            "Event published successfully"
        );
        
        #[cfg(feature = "metrics")]
        metrics::counter!("tokio_events_published_total", "type" => T::event_type().to_string()).increment(1);

        Ok(event_id)
    }

    /// Publish an event with a specific delay before it is routed to subscribers.
    pub async fn publish_delayed<T: Event>(
        &self,
        event: T,
        delay: std::time::Duration,
    ) -> Result<Uuid> {
        let metadata = EventMetadata::new().delay(delay);
        self.publish_with_metadata(event, metadata).await
    }

    /// Publish an event scheduled for an exact future time.
    pub async fn publish_scheduled<T: Event>(
        &self,
        event: T,
        deliver_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Uuid> {
        let metadata = EventMetadata::new().schedule_at(deliver_at);
        self.publish_with_metadata(event, metadata).await
    }

    /// Subscribe to events of a specific type
    pub async fn subscribe<T, F, Fut>(&self, handler: F) -> Result<SubscriptionHandle>
    where
        T: Event,
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if self.is_shutting_down.load(Ordering::SeqCst) {
            return Err(Error::ShuttingDown);
        }

        self.subscription_manager.subscribe_fn(handler).await
    }

    /// Subscribe to events of a specific type with a handler that can fail.
    ///
    /// If the handler returns an `Err`, the event bus will use its retry mechanism
    /// (with exponential backoff) before routing the event to the Dead Letter Queue.
    pub async fn subscribe_fallible<T, F, Fut>(&self, handler: F) -> Result<SubscriptionHandle>
    where
        T: Event,
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        if self.is_shutting_down.load(Ordering::SeqCst) {
            return Err(Error::ShuttingDown);
        }

        let function_handler = crate::subscription::handler::FallibleFunctionHandler::new(handler);
        self.subscription_manager.subscribe::<T, _>(function_handler).await
    }

    /// Subscribe with a custom handler implementation
    pub async fn subscribe_handler<T, H>(&self, handler: H) -> Result<SubscriptionHandle>
    where
        T: Event,
        H: EventHandler,
    {
        if self.is_shutting_down.load(Ordering::SeqCst) {
            return Err(Error::ShuttingDown);
        }

        self.subscription_manager.subscribe::<T, H>(handler).await
    }

    /// Subscribe to a distributed event over the remote network (e.g. NATS).
    ///
    /// This routes both LOCAL and REMOTE events to the provided handler.
    /// The `queue_group` ensures that if multiple instances of this microservice are running,
    /// only one instance processes each remote event (load balancing).
    #[cfg(feature = "remote")]
    #[cfg_attr(docsrs, doc(cfg(feature = "remote")))]
    pub async fn subscribe_remote<T, F, Fut>(
        &self,
        queue_group: &str,
        handler: F,
    ) -> Result<SubscriptionHandle>
    where
        T: crate::event::Remote,
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        // 1. Subscribe locally first
        let handle = self.subscribe(handler).await?;
        
        // 2. Connect the inbound network stream to the local dispatcher
        if let Some(transport) = &self.remote_transport {
            use futures::StreamExt;
            
            let topic = T::remote_topic();
            let mut stream = transport.subscribe(topic, queue_group).await?;
            
            // We need a clone of the event bus to publish inbound events locally
            let local_bus = self.clone();
            let queue_group_owned = queue_group.to_string();
            
            tokio::spawn(async move {
                tracing::info!("Started Remote Consumer Loop for topic: {} (group: {})", topic, queue_group_owned);
                
                while let Some(bytes) = stream.next().await {
                    match T::deserialize_event(&bytes) {
                        Ok(event) => {
                            // Successfully deserialized! Inject it into the local memory bus.
                            if let Err(e) = local_bus.publish(event).await {
                                tracing::error!("Failed to route remote event locally: {}", e);
                            }
                        }
                        Err(e) => {
                            // MITIGATION: Network Poison Pill DLQ
                            tracing::error!("Failed to deserialize remote event on topic {}: {}", topic, e);
                            
                            if let Some(dlq_tx) = local_bus.subscription_manager.dlq_tx() {
                                let mut envelope = EventEnvelope::new(
                                    crate::event::BroadcastEvent { message: "Poison Pill".to_string() }
                                );
                                envelope.payload_bytes = Some(bytes);
                                
                                let _ = dlq_tx.send(Arc::new(envelope)).await;
                            }
                        }
                    }
                }
                
                tracing::info!("Remote Consumer Loop stopped for topic: {}", topic);
            });
        } else {
            tracing::warn!("subscribe_remote called without a remote_transport configured! Only listening locally.");
        }
        
        Ok(handle)
    }

    /// Unsubscribe a handler
    pub async fn unsubscribe(&self, handle: SubscriptionHandle) -> Result<()> {
        self.subscription_manager.unsubscribe(handle).await
    }

    /// Replay unacknowledged events from persistent storage
    ///
    /// This should be called manually *after* setting up all your `.subscribe(...)`
    /// routes. The dispatcher will scan the persistent database for orphaned events
    /// and inject them into the memory queues of the currently active subscribers.
    /// If you do not use the `persistence` feature, this method does nothing.
    pub async fn replay_pending(&self) -> Result<()> {
        let mut dispatcher_guard = self.dispatcher.lock().await;
        if let Some(dispatcher) = dispatcher_guard.as_mut() {
            dispatcher.replay_pending().await
        } else {
            Err(Error::internal("Dispatcher has been shut down"))
        }
    }

    /// Publish a distributed event over the remote network (e.g. NATS).
    ///
    /// This utilizes the Outbox pattern. The event is first published locally
    /// (to ensure disk persistence if configured) and then dispatched over the network.
    #[cfg(feature = "remote")]
    #[cfg_attr(docsrs, doc(cfg(feature = "remote")))]
    pub async fn publish_remote<T: crate::event::Remote>(&self, event: T) -> Result<Uuid> {
        let topic = T::remote_topic();
        let payload = event.serialize_event()?;
            
        // 1. We ALWAYS route it locally first!
        // This is the Outbox Pattern: it ensures the event is saved to redb (if configured)
        // and routed to any local subscribers before we hit the network.
        let event_id = self.publish(event).await?;
        
        // 2. We route it over the network
        if let Some(transport) = &self.remote_transport {
            let msg_id = event_id.to_string();
            // We pass the envelope ID as the `msg_id` for NATS exactly-once deduplication!
            transport.publish(topic, &payload, Some(&msg_id)).await?;
        } else {
            tracing::warn!("publish_remote called on EventBus without a remote_transport configured! Event {} was only routed locally.", event_id);
        }

        Ok(event_id)
    }

    /// Publish a distributed event over the remote network (e.g. NATS) with a specific delay.
    ///
    /// **Caveat**: The delay is held locally in memory before crossing the network. If this server
    /// crashes during the delay, the event will safely fire locally on reboot, but the NATS
    /// network publish will be lost.
    #[cfg(feature = "remote")]
    #[cfg_attr(docsrs, doc(cfg(feature = "remote")))]
    pub async fn publish_remote_delayed<T: crate::event::Remote>(
        &self,
        event: T,
        delay: std::time::Duration,
    ) -> Result<Uuid> {
        let topic = T::remote_topic();
        let payload = event.serialize_event()?;
            
        // 1. Publish locally WITH the delay (This guarantees local crash resilience via Redb)
        let event_id = self.publish_delayed(event, delay).await?;
        
        // 2. Schedule the network publish
        if let Some(transport) = &self.remote_transport {
            let msg_id = event_id.to_string();
            let transport_clone = transport.clone();
            let topic_owned = topic.to_string();
            
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                if let Err(e) = transport_clone.publish(&topic_owned, &payload, Some(&msg_id)).await {
                    tracing::error!("Failed to route delayed remote event: {}", e);
                }
            });
        }

        Ok(event_id)
    }

    /// Publish a distributed event over the remote network (e.g. NATS) scheduled for an exact time.
    ///
    /// **Caveat**: The delay is held locally in memory before crossing the network. If this server
    /// crashes during the delay, the event will safely fire locally on reboot, but the NATS
    /// network publish will be lost.
    #[cfg(feature = "remote")]
    #[cfg_attr(docsrs, doc(cfg(feature = "remote")))]
    pub async fn publish_remote_scheduled<T: crate::event::Remote>(
        &self,
        event: T,
        deliver_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Uuid> {
        let topic = T::remote_topic();
        let payload = event.serialize_event()?;
            
        // 1. Publish locally WITH the schedule
        let event_id = self.publish_scheduled(event, deliver_at).await?;
        
        // 2. Schedule the network publish
        if let Some(transport) = &self.remote_transport {
            let now = chrono::Utc::now();
            let msg_id = event_id.to_string();
            let transport_clone = transport.clone();
            let topic_owned = topic.to_string();

            if deliver_at > now {
                if let Ok(delay) = (deliver_at - now).to_std() {
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        if let Err(e) = transport_clone.publish(&topic_owned, &payload, Some(&msg_id)).await {
                            tracing::error!("Failed to route scheduled remote event: {}", e);
                        }
                    });
                }
            } else {
                // Time has already passed, publish immediately
                transport.publish(&topic, &payload, Some(&msg_id)).await?;
            }
        }

        Ok(event_id)
    }

    /// Get statistics about the event bus
    pub fn stats(&self) -> EventBusStats {
        // Try to get dispatcher stats; if shutdown already took it, use defaults
        let dispatcher_stats = self.dispatcher.try_lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|d| d.stats()))
            .unwrap_or_default();

        EventBusStats {
            total_subscriptions: self.registry.total_subscriptions(),
            event_types: self.registry.event_types().len(),
            dispatcher_stats,
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
        self.is_shutting_down.load(Ordering::SeqCst)
    }

    /// Shutdown the event bus abruptly
    ///
    /// This method works via `&self` so you can call it on an `Arc<EventBus>`.
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down EventBus (abruptly)");

        // Mark as shutting down
        self.is_shutting_down.store(true, Ordering::SeqCst);

        // Run shutdown hooks
        let hooks = self.shutdown_hooks.lock().await;
        for hook in hooks.iter() {
            if let Err(e) = hook().await {
                error!("Shutdown hook failed: {}", e);
            }
        }
        drop(hooks);

        // Take and stop the dispatcher with timeout
        let dispatcher_shutdown = tokio::time::timeout(self.config.shutdown_timeout, async {
            let mut guard = self.dispatcher.lock().await;
            if let Some(mut dispatcher) = guard.take() {
                dispatcher.stop().await
            } else {
                Ok(())
            }
        });

        if dispatcher_shutdown.await.is_err() {
            warn!("Dispatcher shutdown timed out");
        }

        // Shutdown subscription manager
        self.subscription_manager.shutdown().await?;

        info!("EventBus abrupt shutdown complete");
        Ok(())
    }

    /// Shutdown the event bus gracefully
    /// 
    /// This will prevent new events from being published, wait for the dispatcher to route
    /// all buffered events, and wait for all handler tasks to finish processing their events.
    ///
    /// This method works via `&self` so you can call it on an `Arc<EventBus>`.
    pub async fn shutdown_gracefully(&self) -> Result<()> {
        info!("Shutting down EventBus gracefully");

        // Mark as shutting down, preventing new publishes
        self.is_shutting_down.store(true, Ordering::SeqCst);

        // Run shutdown hooks
        let hooks = self.shutdown_hooks.lock().await;
        for hook in hooks.iter() {
            if let Err(e) = hook().await {
                error!("Shutdown hook failed: {}", e);
            }
        }
        drop(hooks);

        // Take and gracefully drain the dispatcher
        {
            let mut guard = self.dispatcher.lock().await;
            if let Some(mut dispatcher) = guard.take() {
                if let Err(e) = dispatcher.shutdown_gracefully().await {
                    error!("Dispatcher graceful shutdown failed: {}", e);
                }
            }
        }

        // Wait for subscription manager to finish processing handler tasks
        self.subscription_manager.shutdown_gracefully().await;

        info!("EventBus graceful shutdown complete");
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

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct TestEvent {
        value: String,
    }

    impl Event for TestEvent {
        fn event_type() -> &'static str {
            "TestEvent"
        }
        fn serialize_event(&self) -> crate::Result<Vec<u8>> {
            serde_json::to_vec(self).map_err(|e| crate::Error::SerializationError(e.to_string()))
        }
        fn deserialize_event(bytes: &[u8]) -> crate::Result<Self> {
            serde_json::from_slice(bytes).map_err(|e| crate::Error::SerializationError(e.to_string()))
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
