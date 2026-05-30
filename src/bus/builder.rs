//! Builder pattern for constructing EventBus instances.

use crate::bus::config::EventBusConfig;
use crate::dispatcher::{ChannelDispatcher, EventDispatcher};
use crate::registry::{DashMapRegistry, EventRegistry};
use crate::subscription::SubscriptionManager;
use crate::{EventBus, Result};
use std::sync::Arc;
use tracing::info;

/// Builder for creating EventBus instances
#[allow(missing_debug_implementations)]
pub struct EventBusBuilder {
    config: EventBusConfig,
    registry: Option<Arc<dyn EventRegistry>>,
    custom_dispatcher: Option<Box<dyn EventDispatcher>>,

    #[cfg(feature = "persistence")]
    redb: Option<Arc<redb::Database>>,

    #[cfg(feature = "persistence")]
    redb_path: Option<std::path::PathBuf>,
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
        }
    }

    /// Use a custom configuration
    pub fn config(mut self, config: EventBusConfig) -> Self {
        self.config = config;
        self
    }

    /// Configure the event bus
    pub fn configure<F>(mut self, f: F) -> Self
    where
        F: FnOnce(EventBusConfig) -> EventBusConfig,
    {
        self.config = f(self.config);
        self
    }

    /// Use a custom registry implementation
    pub fn registry(mut self, registry: Arc<dyn EventRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Use a custom dispatcher implementation
    pub fn custom_dispatcher<D>(mut self, dispatcher: D) -> Self
    where
        D: EventDispatcher + 'static,
    {
        self.custom_dispatcher = Some(Box::new(dispatcher));
        self
    }

    /// Enable redb persistence for the event bus using an existing Database instance
    #[cfg(feature = "persistence")]
    pub fn with_redb(mut self, db: Arc<redb::Database>) -> Self {
        self.redb = Some(db);
        self
    }

    /// Enable redb persistence by providing a file path
    /// The database will be created automatically when `build()` is called.
    #[cfg(feature = "persistence")]
    pub fn with_redb_path(mut self, path: impl AsRef<std::path::Path>) -> Self {
        self.redb_path = Some(path.as_ref().to_path_buf());
        self
    }

    /// Build with high-throughput configuration
    pub fn high_throughput(self) -> Self {
        self.config(EventBusConfig::high_throughput())
    }

    /// Build with reliable processing configuration
    pub fn reliable(self) -> Self {
        self.config(EventBusConfig::reliable())
    }

    /// Build with ordered processing configuration
    pub fn ordered(self) -> Self {
        self.config(EventBusConfig::ordered())
    }

    /// Build the EventBus
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
        let subscription_manager = Arc::new(SubscriptionManager::new(registry.clone()));

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
            ))
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

        // Create the event bus
        let bus = EventBus {
            config: self.config,
            registry,
            subscription_manager,
            dispatcher,
            shutdown_hooks: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            is_shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
}
