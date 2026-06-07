//! Builder pattern for constructing EventBus instances.

use crate::bus::config::EventBusConfig;
use crate::dispatcher::{ChannelDispatcher, EventDispatcher};
use crate::registry::{DashMapRegistry, EventRegistry};
use crate::subscription::SubscriptionManager;
use crate::EventEnvelope;
use crate::{EventBus, Result};
use std::sync::Arc;
use tracing::info;

/// Type alias for DLQ hook function
pub type DlqHook =
    Box<dyn Fn(Arc<EventEnvelope>) -> futures::future::BoxFuture<'static, ()> + Send + Sync>;

/// Builder pattern for constructing `EventBus` instances.
///
/// The `EventBusBuilder` provides a fluent API for configuring the event bus
/// before it is instantiated. It allows configuring dispatchers, storage backends
/// (e.g. Redb persistence), remote transports (e.g. NATS JetStream), and default configurations.
///
/// # Examples
///
/// ```rust,ignore
/// use tokio_events::bus::builder::EventBusBuilder;
///
/// #[tokio::main]
/// async fn main() {
///     let bus = EventBusBuilder::new()
///         .reliable() // Configure for reliable delivery with retries
///         .with_redb_path("events.db") // Enable persistent storage
///         .build()
///         .await
///         .unwrap();
/// }
/// ```
#[allow(missing_debug_implementations)]
pub struct EventBusBuilder {
    config: EventBusConfig,
    registry: Option<Arc<dyn EventRegistry>>,
    custom_dispatcher: Option<Box<dyn EventDispatcher>>,

    #[cfg(feature = "persistence")]
    redb: Option<Arc<redb::Database>>,

    #[cfg(feature = "persistence")]
    redb_path: Option<std::path::PathBuf>,

    dlq_handler: Option<DlqHook>,

    #[cfg(feature = "remote")]
    nats_url: Option<String>,

    #[cfg(feature = "remote")]
    nats_jetstream_name: Option<String>,

    #[cfg(feature = "remote")]
    nats_jetstream_subjects: Option<Vec<String>>,
    #[cfg(feature = "remote")]
    custom_transport: Option<std::sync::Arc<dyn crate::remote::RemoteTransport>>,
}

impl EventBusBuilder {
    /// Create a new builder with default configuration
    pub fn new() -> Self {
        Self {
            config: EventBusConfig::default(),
            registry: None,
            custom_dispatcher: None,
            #[cfg(feature = "persistence")]
            redb: None,
            #[cfg(feature = "persistence")]
            redb_path: None,
            dlq_handler: None,
            #[cfg(feature = "remote")]
            nats_url: None,
            #[cfg(feature = "remote")]
            nats_jetstream_name: None,
            #[cfg(feature = "remote")]
            nats_jetstream_subjects: None,
            #[cfg(feature = "remote")]
            custom_transport: None,
        }
    }

    /// Apply a custom `EventBusConfig` overriding all default settings.
    ///
    /// This allows you to construct a configuration object externally and apply it
    /// at once, rather than using the fluent builder methods.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let mut config = EventBusConfig::default();
    /// config.max_retries = 10;
    /// 
    /// let bus = EventBusBuilder::new()
    ///     .with_config(config)
    ///     .build()
    ///     .await?;
    /// ```
    pub fn with_config(mut self, config: EventBusConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the scheduler tick rate for persistent scheduled events.
    ///
    /// The event bus uses a background worker to poll the database for delayed or 
    /// scheduled events whose delivery times have arrived. This configuration controls
    /// exactly how often that polling occurs.
    ///
    /// - **Higher tick rate (e.g. 100ms)**: Better precision for event dispatch, but consumes more CPU and I/O.
    /// - **Lower tick rate (e.g. 5s)**: Lower overhead, but scheduled events may be dispatched up to 5 seconds late.
    ///
    /// Default is `1 second`.
    pub fn with_scheduler_tick_rate(mut self, rate: std::time::Duration) -> Self {
        self.config.scheduler_tick_rate = rate;
        self
    }

    /// Configure the event bus using a closure.
    ///
    /// This is useful for modifying specific settings on the default configuration
    /// without replacing the entire `EventBusConfig` object.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let bus = EventBusBuilder::new()
    ///     .configure(|mut cfg| {
    ///         cfg.max_retries = 10;
    ///         cfg.handler_channel_size = 500;
    ///         cfg
    ///     })
    ///     .build()
    ///     .await?;
    /// ```
    pub fn configure<F>(mut self, f: F) -> Self
    where
        F: FnOnce(EventBusConfig) -> EventBusConfig,
    {
        self.config = f(self.config);
        self
    }

    /// Use a custom `EventRegistry` implementation.
    ///
    /// By default, `tokio-events` uses an extremely fast `DashMapRegistry`. However,
    /// if you are building a distributed system and need to coordinate active subscriptions 
    /// across multiple nodes (e.g. via Redis or etcd), you can inject a custom registry here.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let custom_reg = Arc::new(MyRedisRegistry::new());
    /// let bus = EventBusBuilder::new()
    ///     .registry(custom_reg)
    ///     .build()
    ///     .await?;
    /// ```
    pub fn registry(mut self, registry: Arc<dyn EventRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Use a custom `EventDispatcher` implementation.
    ///
    /// The dispatcher is the core engine that routes events from publishers to subscribers.
    /// The default `ChannelDispatcher` uses Tokio MPSC channels. You can provide a custom 
    /// dispatcher if you need highly specialized routing logic, such as consistent hashing,
    /// priority queues, or custom backpressure handling.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let my_dispatcher = CustomPriorityDispatcher::new();
    /// let bus = EventBusBuilder::new()
    ///     .custom_dispatcher(my_dispatcher)
    ///     .build()
    ///     .await?;
    /// ```
    pub fn custom_dispatcher<D>(mut self, dispatcher: D) -> Self
    where
        D: EventDispatcher + 'static,
    {
        self.custom_dispatcher = Some(Box::new(dispatcher));
        self
    }

    /// Enable redb persistence for the event bus using an existing `Database` instance.
    ///
    /// If your application already manages a `redb::Database` for its own business logic,
    /// you can share that instance with the `EventBus`. The bus will automatically create
    /// and manage its own tables within the shared database.
    ///
    /// This enables the Transactional Outbox pattern without requiring a secondary database file.
    #[cfg(feature = "persistence")]
    pub fn with_redb(mut self, db: Arc<redb::Database>) -> Self {
        self.redb = Some(db);
        self
    }

    /// Enable redb persistence by providing a file path.
    ///
    /// The database will be created automatically when `build()` is called. Enabling
    /// persistence activates the Transactional Outbox pattern, ensuring that events
    /// are durably stored on disk before being dispatched, preventing data loss
    /// during unexpected application crashes.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let bus = EventBusBuilder::new()
    ///     .with_redb_path("./data/events.db")
    ///     .build()
    ///     .await?;
    /// ```
    #[cfg(feature = "persistence")]
    pub fn with_redb_path(mut self, path: impl AsRef<std::path::Path>) -> Self {
        self.redb_path = Some(path.as_ref().to_path_buf());
        self
    }

    /// Build with a high-throughput configuration profile.
    ///
    /// This preset prioritizes maximum event processing speed and concurrency over strict
    /// durability. It is ideal for scenarios like metrics ingestion, logging, or non-critical 
    /// telemetry where processing speed is more important than guaranteeing zero data loss.
    ///
    /// Under the hood, this configuration:
    /// - Increases the internal maximum queue size to `50,000`.
    /// - Sets worker threads to double the available CPU cores for maximum parallelism.
    /// - Allows events to be silently dropped if the queue becomes completely full (`drop_on_full = true`).
    /// - Limits retries to `1` (fail fast).
    /// - Disables waiting for disk persistence.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let bus = EventBusBuilder::new()
    ///     .high_throughput()
    ///     .build()
    ///     .await?;
    /// ```
    pub fn high_throughput(self) -> Self {
        self.with_config(EventBusConfig::high_throughput())
    }

    /// Build with a reliable processing configuration profile.
    ///
    /// This preset guarantees no event loss under heavy load, making it ideal for 
    /// financial transactions, order processing, and critical state machines.
    ///
    /// Under the hood, this configuration:
    /// - Forbids dropping events on full queues (`drop_on_full = false`).
    /// - Enables disk persistence waits (`wait_for_persistence = true`). If persistence is enabled, 
    ///   publish calls will block until the event is durably written to the physical hard drive.
    /// - Increases maximum retries to `5` with a `500ms` backoff.
    /// - Allocates a larger Dead Letter Queue (DLQ) channel size (`5000`).
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let bus = EventBusBuilder::new()
    ///     .reliable()
    ///     .with_redb_path("critical_events.db")
    ///     .build()
    ///     .await?;
    /// ```
    pub fn reliable(self) -> Self {
        self.with_config(EventBusConfig::reliable())
    }

    /// Build with a strict ordered processing configuration profile.
    ///
    /// This preset guarantees that events are processed strictly in the exact order
    /// they are published, preventing race conditions in dependent state updates.
    ///
    /// Under the hood, this configuration:
    /// - Limits the internal dispatcher to a **single worker thread** (`worker_threads = 1`).
    /// - Forbids dropping events (`drop_on_full = false`).
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let bus = EventBusBuilder::new()
    ///     .ordered()
    ///     .build()
    ///     .await?;
    /// ```
    pub fn ordered(self) -> Self {
        self.with_config(EventBusConfig::ordered())
    }

    /// Set whether publish calls should wait for disk persistence.
    ///
    /// If `true`, calling `bus.publish()` will block until the event is durably synced 
    /// to the physical disk (fsync). This provides the highest level of reliability but
    /// reduces publish throughput.
    ///
    /// If `false` (default), `bus.publish()` will return as soon as the event is written
    /// to the memory queue, while a background task syncs it to disk.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let bus = EventBusBuilder::new()
    ///     .with_redb_path("events.db")
    ///     .wait_for_persistence(true) // Maximize safety
    ///     .build()
    ///     .await?;
    /// ```
    pub fn wait_for_persistence(mut self, wait: bool) -> Self {
        self.config.wait_for_persistence = wait;
        self
    }

    /// Attach a custom handler for Dead Letter Queue (DLQ) events.
    ///
    /// This async closure will automatically be called for any event that permanently
    /// fails all processing retries. This is typically used to move the failed event 
    /// to an external storage system or alert an operations team.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let bus = EventBusBuilder::new()
    ///     .with_dlq_handler(|failed_event| async move {
    ///         println!("Event {} permanently failed!", failed_event.event_id());
    ///     })
    ///     .build()
    ///     .await?;
    /// ```
    pub fn with_dlq_handler<F, Fut>(mut self, handler: F) -> Self
    where
        F: Fn(Arc<EventEnvelope>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.dlq_handler = Some(Box::new(move |env| Box::pin(handler(env))));
        self
    }

    /// Enable Core NATS transport for distributed remote events.
    ///
    /// This attaches the event bus to a NATS cluster using standard "fire-and-forget" semantics.
    /// Any events published to the local bus will be seamlessly routed over the network to other
    /// microservices listening on the same NATS topics.
    ///
    /// *Note: Core NATS provides At-Most-Once delivery. For persistent Exactly-Once delivery, use `with_nats_jetstream`.*
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let bus = EventBusBuilder::new()
    ///     .with_nats_transport("nats://localhost:4222")
    ///     .build()
    ///     .await?;
    /// ```
    #[cfg(feature = "remote")]
    pub fn with_nats_transport(mut self, url: impl Into<String>) -> Self {
        self.nats_url = Some(url.into());
        self.nats_jetstream_name = None;
        self.nats_jetstream_subjects = None;
        self
    }

    /// Enable NATS JetStream for persistent distributed remote events.
    ///
    /// This configures the bus to use JetStream, providing enterprise-grade **Exactly-Once** 
    /// or **At-Least-Once** distributed delivery. JetStream ensures that even if a microservice
    /// is offline, the NATS server will durably hold the events until the service comes back online.
    ///
    /// # Arguments
    ///
    /// * `url` - The NATS cluster URL (e.g., `"nats://localhost:4222"`).
    /// * `stream_name` - The globally unique JetStream Stream name.
    /// * `subjects` - A list of wildcard topics (e.g., `vec!["events.*".to_string()]`) that this stream binds to.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let bus = EventBusBuilder::new()
    ///     .with_nats_jetstream(
    ///         "nats://localhost:4222", 
    ///         "MY_APP_STREAM", 
    ///         vec!["myapp.>".to_string()]
    ///     )
    ///     .build()
    ///     .await?;
    /// ```
    #[cfg(feature = "remote")]
    pub fn with_nats_jetstream(
        mut self,
        url: impl Into<String>,
        stream_name: impl Into<String>,
        subjects: Vec<String>,
    ) -> Self {
        self.nats_url = Some(url.into());
        self.nats_jetstream_name = Some(stream_name.into());
        self.nats_jetstream_subjects = Some(subjects);
        self
    }

    /// Provide a custom remote transport implementation.
    ///
    /// This allows you to inject custom network routing logic. You can use this to implement
    /// your own transports (e.g. RabbitMQ, Kafka, MQTT, or a custom TCP Mesh), or to inject 
    /// dummy transports during unit tests.
    #[cfg(feature = "remote")]
    pub fn with_custom_transport(
        mut self,
        transport: std::sync::Arc<dyn crate::remote::RemoteTransport>,
    ) -> Self {
        self.custom_transport = Some(transport);
        self
    }

    /// Construct the `EventBus` instance based on the provided configuration.
    ///
    /// This method performs all necessary asynchronous setup, such as initializing
    /// the database, establishing network connections for remote transport, and
    /// starting the background dispatcher worker tasks.
    ///
    /// # Returns
    ///
    /// Returns the fully initialized `EventBus`.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying database cannot be opened, or if the
    /// remote transport fails to connect.
    pub async fn build(self) -> Result<EventBus> {
        info!("Building EventBus");

        // Create or use provided registry
        #[cfg(not(feature = "persistence"))]
        let registry = self.registry.unwrap_or_else(|| {
            info!("Creating default DashMapRegistry");
            Arc::new(DashMapRegistry::with_capacity(100))
        });

        #[cfg(feature = "persistence")]
        let mut db_instance = self.redb;

        #[cfg(feature = "persistence")]
        if db_instance.is_none() {
            if let Some(path) = &self.redb_path {
                info!("Creating redb Database at {:?}", path);
                let db = redb::Database::create(path).map_err(|e| {
                    crate::Error::internal(format!("Failed to create redb database: {}", e))
                })?;
                db_instance = Some(Arc::new(db));
            }
        }

        #[cfg(feature = "persistence")]
        let registry = if let Some(db) = &db_instance {
            let base = Arc::new(DashMapRegistry::with_capacity(100));
            Arc::new(crate::persistence::RedbRegistry::new(db.clone(), base))
                as Arc<dyn EventRegistry>
        } else {
            self.registry.unwrap_or_else(|| {
                info!("Creating default DashMapRegistry");
                Arc::new(DashMapRegistry::with_capacity(100))
            })
        };

        // Create subscription manager
        let mut sm = SubscriptionManager::with_channel_size(
            registry.clone(),
            self.config.max_retries,
            self.config.retry_backoff,
            self.config.handler_channel_size,
        );

        let (dlq_tx, dlq_rx) = tokio::sync::mpsc::channel(self.config.dlq_channel_size);
        sm.set_dlq(dlq_tx);

        let mut dlq_rx_opt = Some(dlq_rx);

        // If a custom DLQ handler was provided, spawn a background task to consume the DLQ automatically!
        if let Some(dlq_hook) = self.dlq_handler {
            let mut rx = dlq_rx_opt.take().unwrap();
            let handler = Arc::new(dlq_hook);
            tokio::spawn(async move {
                while let Some(poison_pill) = rx.recv().await {
                    handler(poison_pill).await;
                }
            });
        }

        let subscription_manager = Arc::new(sm);

        // Create or use provided dispatcher
        #[cfg(not(feature = "persistence"))]
        let mut dispatcher = if let Some(dispatcher) = self.custom_dispatcher {
            info!("Using custom dispatcher");
            dispatcher
        } else {
            info!("Creating default ChannelDispatcher");
            Box::new(ChannelDispatcher::new(
                self.config.dispatcher.clone(),
                subscription_manager.clone(),
            )) as Box<dyn EventDispatcher>
        };

        #[cfg(feature = "persistence")]
        let mut dispatcher = if let Some(dispatcher) = self.custom_dispatcher {
            info!("Using custom dispatcher");
            dispatcher
        } else if let Some(db) = &db_instance {
            info!("Creating RedbDispatcher for persistence");
            Box::new(crate::persistence::RedbDispatcher::new(
                db.clone(),
                self.config.dispatcher.clone(),
                self.config.wait_for_persistence,
                self.config.scheduler_tick_rate,
                subscription_manager.clone(),
            )) as Box<dyn EventDispatcher>
        } else {
            info!("Creating default ChannelDispatcher");
            Box::new(ChannelDispatcher::new(
                self.config.dispatcher.clone(),
                subscription_manager.clone(),
            )) as Box<dyn EventDispatcher>
        };

        // Start the dispatcher
        dispatcher.start().await?;

        // Initialize remote transport if configured
        #[cfg(feature = "remote")]
        let remote_transport = if let Some(custom) = self.custom_transport {
            Some(custom)
        } else if let Some(url) = &self.nats_url {
            if let (Some(stream_name), Some(subjects)) =
                (&self.nats_jetstream_name, &self.nats_jetstream_subjects)
            {
                info!(
                    "Connecting to NATS JetStream at {} with stream: {}",
                    url, stream_name
                );
                let transport = crate::remote::nats::NatsTransport::connect_jetstream(
                    url,
                    stream_name,
                    subjects.clone(),
                )
                .await?;
                Some(Arc::new(transport) as Arc<dyn crate::remote::RemoteTransport>)
            } else {
                info!("Connecting to Core NATS at {}", url);
                let transport = crate::remote::nats::NatsTransport::connect(url).await?;
                Some(Arc::new(transport) as Arc<dyn crate::remote::RemoteTransport>)
            }
        } else {
            None
        };

        // Create the event bus
        let bus = EventBus {
            config: self.config,
            registry,
            subscription_manager,
            dispatcher: Arc::new(tokio::sync::RwLock::new(Some(dispatcher))),
            shutdown_hooks: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            is_shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            dlq_rx: Arc::new(tokio::sync::Mutex::new(dlq_rx_opt)),

            #[cfg(feature = "remote")]
            remote_transport,
        };

        info!("EventBus built successfully");
        Ok(bus)
    }
}

impl Default for EventBusBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_builder_default() {
        let bus = EventBusBuilder::new().build().await.unwrap();
        assert!(!bus.is_shutting_down());
    }

    #[tokio::test]
    async fn test_builder_configurations() {
        // High throughput
        let _bus = EventBusBuilder::new()
            .high_throughput()
            .build()
            .await
            .unwrap();

        // Reliable
        let _bus = EventBusBuilder::new().reliable().build().await.unwrap();

        // Ordered
        let _bus = EventBusBuilder::new().ordered().build().await.unwrap();
    }

    #[cfg(feature = "remote")]
    #[tokio::test]
    async fn test_builder_nats_config_overrides() {
        // Test with_nats_transport clears JetStream config
        let builder = EventBusBuilder::new()
            .with_nats_jetstream("nats://local", "STREAM", vec!["test".to_string()])
            .with_nats_transport("nats://core");

        assert_eq!(builder.nats_url, Some("nats://core".to_string()));
        assert!(builder.nats_jetstream_name.is_none());
        assert!(builder.nats_jetstream_subjects.is_none());

        // Test with_nats_jetstream sets JetStream config correctly
        let builder = EventBusBuilder::new()
            .with_nats_transport("nats://core")
            .with_nats_jetstream("nats://js", "JS_STREAM", vec!["subject.>".to_string()]);

        assert_eq!(builder.nats_url, Some("nats://js".to_string()));
        assert_eq!(builder.nats_jetstream_name, Some("JS_STREAM".to_string()));
        assert_eq!(
            builder.nats_jetstream_subjects,
            Some(vec!["subject.>".to_string()])
        );
    }
}
