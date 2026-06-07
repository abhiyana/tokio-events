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
    pub(crate) dispatcher: Arc<tokio::sync::RwLock<Option<Box<dyn EventDispatcher>>>>,
    pub(crate) shutdown_hooks: Arc<Mutex<Vec<ShutdownHook>>>,
    pub(crate) is_shutting_down: Arc<AtomicBool>,
    pub(crate) dlq_rx:
        Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<Arc<EventEnvelope>>>>>,

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
    pub async fn take_dlq_receiver(
        &self,
    ) -> Option<tokio::sync::mpsc::Receiver<Arc<EventEnvelope>>> {
        self.dlq_rx.lock().await.take()
    }

    /// Publish an event to the bus.
    ///
    /// This is the primary method for dispatching an event to all interested subscribers.
    /// The event payload will automatically be wrapped in an `EventEnvelope` with a newly
    /// generated unique `Uuid`.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Event)]
    /// struct UserCreated { id: u64 }
    ///
    /// let event_id = bus.publish(UserCreated { id: 101 }).await?;
    /// println!("Published event with ID: {}", event_id);
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the event bus is in the process of shutting down, or if the
    /// underlying dispatcher fails to accept the event due to backpressure/full queues.
    pub async fn publish<T: Event>(&self, event: T) -> Result<Uuid> {
        self.publish_with_metadata(event, EventMetadata::new())
            .await
    }

    /// Publish an event with custom metadata.
    ///
    /// This allows you to attach vital contextual information to the event envelope
    /// before it enters the dispatcher. Metadata is incredibly powerful for injecting:
    /// - **Correlation IDs**: Linking multiple events in a single distributed trace.
    /// - **Causation IDs**: Tracking the parent event that triggered this child event.
    /// - **Topics**: Subject-based routing.
    /// - **Schedules**: Setting a delivery delay or exact execution time.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let metadata = EventMetadata::new()
    ///     .with_correlation(user_session_id)
    ///     .with_topic("orders.eu");
    ///
    /// bus.publish_with_metadata(OrderPlaced { ... }, metadata).await?;
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the event bus is shutting down or if dispatch fails.
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
            let guard = self.dispatcher.read().await;
            let dispatcher = guard.as_ref().ok_or(Error::ShuttingDown)?;
            dispatcher.dispatch(envelope).await?;
        }

        debug!(
            event_id = %event_id,
            event_type = T::event_type(),
            "Event published successfully"
        );

        #[cfg(feature = "metrics")]
        metrics::counter!("tokio_events_published_total", "type" => T::event_type().to_string())
            .increment(1);

        Ok(event_id)
    }

    /// Publish an event with a specific delay before it is routed to subscribers.
    ///
    /// This is a convenience wrapper around `publish_with_metadata` that sets a delivery delay.
    ///
    /// # Arguments
    ///
    /// * `event` - The event payload to publish.
    /// * `delay` - The `Duration` to wait before delivering the event to subscribers.
    ///
    /// # Returns
    ///
    /// Returns the `Uuid` of the published event.
    ///
    /// # Errors
    ///
    /// Returns an error if the event bus is shutting down or if dispatch fails.
    pub async fn publish_delayed<T: Event>(
        &self,
        event: T,
        delay: std::time::Duration,
    ) -> Result<Uuid> {
        let metadata = EventMetadata::new().delay(delay);
        self.publish_with_metadata(event, metadata).await
    }

    /// Publish an event to a specific topic (for Subject-Based Routing).
    ///
    /// Subscriptions matching this exact topic string will receive the event.
    ///
    /// # Arguments
    ///
    /// * `topic` - The routing key or topic string.
    /// * `event` - The event payload to publish.
    ///
    /// # Returns
    ///
    /// Returns the `Uuid` of the published event.
    pub async fn publish_to<T: Event>(&self, topic: impl Into<String>, event: T) -> Result<Uuid> {
        let metadata = EventMetadata::new().with_topic(topic);
        self.publish_with_metadata(event, metadata).await
    }

    /// Publish an event scheduled for an exact future time.
    ///
    /// # Arguments
    ///
    /// * `event` - The event payload to publish.
    /// * `deliver_at` - The exact UTC `DateTime` when the event should be dispatched.
    ///
    /// # Returns
    ///
    /// Returns the `Uuid` of the published event.
    pub async fn publish_scheduled<T: Event>(
        &self,
        event: T,
        deliver_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Uuid> {
        let metadata = EventMetadata::new().schedule_at(deliver_at);
        self.publish_with_metadata(event, metadata).await
    }

    /// Subscribe a handler to events of a specific type.
    ///
    /// The provided async closure will be invoked asynchronously in a dedicated worker task
    /// for every published event that matches type `T`.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let handle = bus.subscribe(|event: UserCreated| async move {
    ///     println!("User {} was created!", event.id);
    /// }).await?;
    ///
    /// // The subscription remains active as long as the handle is not dropped
    /// // (unless you detach it).
    /// ```
    ///
    /// # CRITICAL: Handle Lifecycle
    ///
    /// The `subscribe` method returns a `SubscriptionHandle`. If you do **not** assign this 
    /// handle to a variable (e.g., `let handle = ...`), Rust will instantly drop the handle 
    /// at the end of the statement, immediately cancelling your subscription before it can process 
    /// any events.
    ///
    /// If you want a subscription to run permanently in the background without needing to store 
    /// the handle, you **MUST** chain `.detach()`:
    ///
    /// ```rust,ignore
    /// bus.subscribe(|e: MyEvent| async move { ... }).await?.detach();
    /// ```
    ///
    /// # Returns
    ///
    /// Returns a `SubscriptionHandle` that manages the lifecycle of the subscription.
    ///
    /// # Errors
    ///
    /// Returns an error if the bus is shutting down or if registration fails.
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

    /// Subscribe a handler to events on a specific wildcard or literal topic.
    ///
    /// This enables **Subject-Based Routing**. The handler will only execute if the
    /// event is both of type `T` AND published with a matching topic in its metadata.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // Subscribes to exact topic
    /// bus.subscribe_topic("orders.eu", |evt: OrderEvent| async move { ... }).await?;
    ///
    /// // Subscribes to all order regions (using wildcard syntax if supported by transport)
    /// bus.subscribe_topic("orders.*", |evt: OrderEvent| async move { ... }).await?;
    /// ```
    ///
    /// # CRITICAL: Handle Lifecycle
    ///
    /// The `subscribe_topic` method returns a `SubscriptionHandle`. If you do **not** assign this 
    /// handle to a variable (e.g., `let handle = ...`), Rust will instantly drop the handle 
    /// at the end of the statement, immediately cancelling your subscription.
    ///
    /// If you want a subscription to run permanently in the background, you **MUST** chain `.detach()`:
    ///
    /// ```rust,ignore
    /// bus.subscribe_topic("topic", |e: MyEvent| async move { ... }).await?.detach();
    /// ```
    ///
    /// # Returns
    ///
    /// Returns a `SubscriptionHandle` governing this topic binding.
    pub async fn subscribe_topic<T, F, Fut>(
        &self,
        topic: &str,
        handler: F,
    ) -> Result<SubscriptionHandle>
    where
        T: Event,
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if self.is_shutting_down.load(Ordering::SeqCst) {
            return Err(Error::ShuttingDown);
        }

        self.subscription_manager
            .subscribe_topic_fn(topic, handler)
            .await
    }

    /// Send a request and wait for a response (Request-Reply Pattern).
    ///
    /// This method automatically creates a temporary inbox subscription, attaches the inbox
    /// topic to the `EventMetadata::reply_to` field of the request, publishes the request,
    /// and asynchronously waits for a responder to send a response back.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let request = FetchUser { id: 1 };
    /// let response: UserFetched = bus.request(request).await?;
    /// println!("Got user: {}", response.name);
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the responder drops the channel without replying, or if the request
    /// times out after the default 30-second waiting period.
    pub async fn request<Req, Res>(&self, request: Req) -> Result<Res>
    where
        Req: Event,
        Res: Event,
    {
        if self.is_shutting_down.load(Ordering::SeqCst) {
            return Err(Error::ShuttingDown);
        }

        let inbox_topic = format!("_INBOX.{}", Uuid::new_v4());
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Wrap the sender in an Option in a Mutex so the handler can take it once
        let tx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(tx)));

        // Subscribe to the inbox topic
        let handle = self
            .subscribe_topic(&inbox_topic, move |response: Res| {
                let tx = tx.clone();
                async move {
                    let mut tx_guard = tx.lock().await;
                    if let Some(sender) = tx_guard.take() {
                        let _ = sender.send(response);
                    }
                }
            })
            .await?;

        // Publish the request with reply_to
        let metadata = EventMetadata::new().with_reply_to(&inbox_topic);
        self.publish_with_metadata(request, metadata).await?;

        // Wait for the response (with a timeout of 30 seconds to prevent hanging)
        let response = match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
            Ok(Ok(res)) => Ok(res),
            Ok(Err(_)) => Err(Error::internal(
                "Responder dropped the channel without replying",
            )),
            Err(_) => Err(Error::internal("Request timed out after 30 seconds")),
        };

        // Cleanup the ephemeral subscription
        let _ = self.unsubscribe(handle).await;

        response
    }

    /// Register a responder for the Request-Reply (RPC) pattern.
    ///
    /// The provided handler receives requests and returns a response. The event bus automatically
    /// packages the response and publishes it to the specific temporary inbox topic provided
    /// in the request's `reply_to` metadata.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// bus.respond(|req: FetchUser| async move {
    ///     let user_name = db.get_user(req.id).await;
    ///     UserFetched { name: user_name }
    /// }).await?;
    /// ```
    ///
    /// # CRITICAL: Handle Lifecycle
    ///
    /// The `respond` method returns a `SubscriptionHandle`. If you do **not** assign this 
    /// handle to a variable (e.g., `let handle = ...`), Rust will instantly drop the handle 
    /// at the end of the statement, immediately cancelling your responder.
    ///
    /// If you want the responder to run permanently in the background, you **MUST** chain `.detach()`:
    ///
    /// ```rust,ignore
    /// bus.respond(|req: FetchUser| async move { ... }).await?.detach();
    /// ```
    pub async fn respond<Req, Res, F, Fut>(&self, handler: F) -> Result<SubscriptionHandle>
    where
        Req: Event,
        Res: Event,
        F: Fn(Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Res> + Send + 'static,
    {
        if self.is_shutting_down.load(Ordering::SeqCst) {
            return Err(Error::ShuttingDown);
        }

        let responder = ResponderHandler {
            bus: self.clone(),
            handler,
            _phantom: std::marker::PhantomData,
        };

        let name = format!("RPCResponder<{}>", Req::event_type());
        self.subscription_manager
            .subscribe_typed::<Req, _>(responder, name, None)
            .await
    }

    /// Subscribe to events with a handler that can return errors.
    ///
    /// If the async closure returns an `Err(_)`, the event bus will hold onto the event
    /// and use its internal retry mechanism (respecting `max_retries` and `retry_backoff`
    /// configured in the builder). If it fails repeatedly, it is routed to the Dead Letter Queue.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// bus.subscribe_fallible(|event: ProcessPayment| async move {
    ///     if network_is_down() {
    ///         return Err(tokio_events::Error::internal("Network down"));
    ///     }
    ///     Ok(())
    /// }).await?;
    /// ```
    ///
    /// # CRITICAL: Handle Lifecycle
    ///
    /// The `subscribe_fallible` method returns a `SubscriptionHandle`. If you do **not** assign this 
    /// handle to a variable (e.g., `let handle = ...`), Rust will instantly drop the handle 
    /// at the end of the statement, immediately cancelling your subscription.
    ///
    /// If you want a subscription to run permanently in the background, you **MUST** chain `.detach()`:
    ///
    /// ```rust,ignore
    /// bus.subscribe_fallible(|e: MyEvent| async move { ... }).await?.detach();
    /// ```
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
        self.subscription_manager
            .subscribe::<T, _>(function_handler)
            .await
    }

    /// Subscribe using a custom struct that implements `EventHandler`.
    ///
    /// Unlike `subscribe()` which takes a closure, this takes a full struct implementation.
    /// This is useful when your handler needs to maintain complex internal state,
    /// implement the `filter()` method to drop events instantly, or implement `on_shutdown()`
    /// to gracefully close database connections when the bus stops.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// struct MyHandler { db_pool: Pool }
    /// impl EventHandler for MyHandler { ... }
    ///
    /// bus.subscribe_handler::<MyEvent, _>(MyHandler { db_pool }).await?;
    /// ```
    ///
    /// # CRITICAL: Handle Lifecycle
    ///
    /// The `subscribe_handler` method returns a `SubscriptionHandle`. If you do **not** assign this 
    /// handle to a variable (e.g., `let handle = ...`), Rust will instantly drop the handle 
    /// at the end of the statement, immediately cancelling your subscription.
    ///
    /// If you want a subscription to run permanently in the background, you **MUST** chain `.detach()`:
    ///
    /// ```rust,ignore
    /// bus.subscribe_handler::<MyEvent, _>(handler).await?.detach();
    /// ```
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
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // Multiple instances of "inventory_service" will load-balance the events
    /// bus.subscribe_remote("inventory_service", |event: OrderPlaced| async move {
    ///     println!("Order received from network: {}", event.id);
    /// }).await?;
    /// ```
    ///
    /// # CRITICAL: Handle Lifecycle
    ///
    /// The `subscribe_remote` method returns a `SubscriptionHandle`. If you do **not** assign this 
    /// handle to a variable (e.g., `let handle = ...`), Rust will instantly drop the handle 
    /// at the end of the statement, immediately cancelling your subscription.
    ///
    /// If you want a subscription to run permanently in the background, you **MUST** chain `.detach()`:
    ///
    /// ```rust,ignore
    /// bus.subscribe_remote("group", |e: MyEvent| async move { ... }).await?.detach();
    /// ```
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
                tracing::info!(
                    "Started Remote Consumer Loop for topic: {} (group: {})",
                    topic,
                    queue_group_owned
                );

                while let Some((bytes, ack_tx)) = stream.next().await {
                    match T::deserialize_event(&bytes) {
                        Ok(event) => {
                            // Successfully deserialized! Inject it into the local memory bus.
                            if let Err(e) = local_bus.publish(event).await {
                                tracing::error!("Failed to route remote event locally: {}", e);
                            }
                        }
                        Err(e) => {
                            // MITIGATION: Network Poison Pill DLQ
                            tracing::error!(
                                "Failed to deserialize remote event on topic {}: {}",
                                topic,
                                e
                            );

                            if let Some(dlq_tx) = local_bus.subscription_manager.dlq_tx() {
                                let mut envelope =
                                    EventEnvelope::new(crate::event::BroadcastEvent {
                                        message: "Poison Pill".to_string(),
                                    });
                                envelope.payload_bytes = Some(bytes);

                                let _ = dlq_tx.send(Arc::new(envelope)).await;
                            }
                        }
                    }

                    // Trigger NATS acknowledgment after successful local routing or DLQ drop.
                    // This guarantees At-Least-Once delivery!
                    let _ = ack_tx.send(());
                }

                tracing::info!("Remote Consumer Loop stopped for topic: {}", topic);
            });
        } else {
            tracing::warn!("subscribe_remote called without a remote_transport configured! Only listening locally.");
        }

        Ok(handle)
    }

    /// Unsubscribe an active subscription.
    ///
    /// # Arguments
    ///
    /// * `handle` - The `SubscriptionHandle` corresponding to the subscription to remove.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if successfully unsubscribed.
    ///
    /// # Errors
    ///
    /// Returns an error if the subscription was not found or if the event bus is shutting down.
    pub async fn unsubscribe(&self, handle: SubscriptionHandle) -> Result<()> {
        self.subscription_manager.unsubscribe(handle).await
    }

    /// Replay unacknowledged events from persistent storage.
    ///
    /// This should be called manually *after* setting up all your `.subscribe(...)`
    /// routes. The dispatcher will scan the persistent database for orphaned events
    /// that were saved but never dispatched before the last crash, and inject them
    /// into the memory queues of the currently active subscribers.
    ///
    /// If you do not use the `persistence` feature, this method does nothing.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// bus.subscribe(|evt: OrderPlaced| async move { ... }).await?;
    ///
    /// // Now that subscribers are ready, process any missed events:
    /// bus.replay_pending().await?;
    /// ```
    pub async fn replay_pending(&self) -> Result<()> {
        let mut dispatcher_guard = self.dispatcher.write().await;
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
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let event_id = bus.publish_remote(OrderPlaced { id: 1 }).await?;
    /// ```
    #[cfg(feature = "remote")]
    #[cfg_attr(docsrs, doc(cfg(feature = "remote")))]
    pub async fn publish_remote<T: crate::event::Remote>(&self, event: T) -> Result<Uuid> {
        let topic = T::remote_topic();
        let payload = event.serialize_event()?;

        let event_id = Uuid::new_v4();

        // 2. We route it over the network
        if let Some(transport) = &self.remote_transport {
            let msg_id = event_id.to_string();
            // We pass the envelope ID as the `msg_id` for NATS exactly-once deduplication!
            transport
                .publish(topic, &payload, Some(msg_id.as_str()))
                .await?;
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
                if let Err(e) = transport_clone
                    .publish(&topic_owned, &payload, Some(&msg_id))
                    .await
                {
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
                        if let Err(e) = transport_clone
                            .publish(&topic_owned, &payload, Some(msg_id.as_str()))
                            .await
                        {
                            tracing::error!("Failed to route scheduled remote event: {}", e);
                        }
                    });
                }
            } else {
                // Time has already passed, publish immediately
                transport
                    .publish(topic, &payload, Some(msg_id.as_str()))
                    .await?;
            }
        }

        Ok(event_id)
    }

    /// Retrieve statistics about the event bus.
    ///
    /// # Returns
    ///
    /// Returns an `EventBusStats` struct containing snapshots of processed events,
    /// errors, and subscription counts.
    pub fn stats(&self) -> EventBusStats {
        // Try to get dispatcher stats; if shutdown already took it, use defaults
        let dispatcher_stats = self
            .dispatcher
            .try_read()
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

    /// Register a custom shutdown hook.
    ///
    /// Shutdown hooks are asynchronous closures executed exactly once when `bus.shutdown()`
    /// or `bus.shutdown_gracefully()` is called. You can use this to close database
    /// connection pools, flush metrics, or cleanly close network sockets.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// bus.register_shutdown_hook(|| async move {
    ///     println!("Flushing redis cache before shutdown...");
    ///     Ok(())
    /// }).await?;
    /// ```
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

    /// Check if the event bus is currently in the process of shutting down.
    ///
    /// # Returns
    ///
    /// Returns `true` if shutting down, `false` otherwise.
    pub fn is_shutting_down(&self) -> bool {
        self.is_shutting_down.load(Ordering::SeqCst)
    }

    /// Shutdown the event bus abruptly.
    ///
    /// This immediately halts the dispatcher, drops any events currently sitting in the
    /// memory queue, and shuts down all background workers. If you want to process
    /// pending events before exiting, use `shutdown_gracefully()` instead.
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
            let mut guard = self.dispatcher.write().await;
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

    /// Shut down the event bus gracefully.
    ///
    /// This method performs an orchestrated shutdown:
    /// 1. Sets the shutting down flag to reject new `publish()` calls.
    /// 2. Executes all registered shutdown hooks concurrently.
    /// 3. Signals the dispatcher to finish processing currently queued events (waiting up to `shutdown_timeout`).
    /// 4. Shuts down the subscription manager and waits for handlers to finish their active futures.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` once all internal components have successfully terminated.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// tokio::signal::ctrl_c().await.unwrap();
    /// println!("Ctrl-C received, draining queues...");
    /// bus.shutdown_gracefully().await?;
    /// ```
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
            let mut guard = self.dispatcher.write().await;
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

#[allow(missing_debug_implementations)]
struct ResponderHandler<Req, Res, F, Fut>
where
    Req: Event,
    Res: Event,
    F: Fn(Req) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Res> + Send + 'static,
{
    bus: EventBus,
    handler: F,
    _phantom: std::marker::PhantomData<fn(Req) -> Res>,
}

#[async_trait::async_trait]
impl<Req, Res, F, Fut> crate::subscription::handler::EventHandler
    for ResponderHandler<Req, Res, F, Fut>
where
    Req: Event,
    Res: Event,
    F: Fn(Req) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Res> + Send + 'static,
{
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        let req = envelope.get_event::<Req>()?;
        let response = (self.handler)(req).await;

        if let Some(reply_to) = &envelope.metadata.reply_to {
            let mut metadata = EventMetadata::new().with_topic(reply_to);
            metadata.correlation_id = envelope
                .metadata
                .correlation_id
                .or(Some(envelope.event_id()));
            metadata.causation_id = Some(envelope.event_id());

            self.bus.publish_with_metadata(response, metadata).await?;
        } else {
            tracing::warn!("Received RPC request without a reply_to topic!");
        }

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
            serde_json::from_slice(bytes)
                .map_err(|e| crate::Error::SerializationError(e.to_string()))
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
