//! Global singleton event bus access.
//!
//! This module provides a convenient way to access a single `EventBus` instance
//! from anywhere in your application without passing `Arc<EventBus>` around.

use crate::{Error, Event, EventBus, Result};
use std::sync::OnceLock;

static GLOBAL_BUS: OnceLock<EventBus> = OnceLock::new();

/// Sets the global event bus instance.
/// 
/// This function can only be called once. Subsequent calls will return an error
/// containing the `EventBus` that was passed in.
#[allow(clippy::result_large_err)]
pub fn set_global_bus(bus: EventBus) -> std::result::Result<(), EventBus> {
    GLOBAL_BUS.set(bus)
}

/// Gets a reference to the global event bus, if it has been initialized.
pub fn get_bus() -> Option<&'static EventBus> {
    GLOBAL_BUS.get()
}

/// Publishes an event to the global event bus.
/// 
/// Returns an error if the global event bus has not been initialized via `set_global_bus`.
pub async fn publish<E: Event>(event: E) -> Result<uuid::Uuid> {
    if let Some(bus) = GLOBAL_BUS.get() {
        bus.publish(event).await
    } else {
        Err(Error::internal("Global event bus not initialized. Call tokio_events::global::set_global_bus() first."))
    }
}

/// Shuts down the global event bus abruptly.
///
/// Returns an error if the global event bus has not been initialized.
pub async fn shutdown() -> Result<()> {
    if let Some(bus) = GLOBAL_BUS.get() {
        bus.shutdown().await
    } else {
        Err(Error::internal("Global event bus not initialized."))
    }
}

/// Shuts down the global event bus gracefully.
///
/// Returns an error if the global event bus has not been initialized.
pub async fn shutdown_gracefully() -> Result<()> {
    if let Some(bus) = GLOBAL_BUS.get() {
        bus.shutdown_gracefully().await
    } else {
        Err(Error::internal("Global event bus not initialized."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventBusBuilder;

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct GlobalTestEvent {
        id: u32,
    }

    impl Event for GlobalTestEvent {
        fn event_type() -> &'static str {
            "GlobalTestEvent"
        }
    }

    #[tokio::test]
    async fn test_global_bus() {
        // We can't safely test the OnceLock globally multiple times in standard cargo test
        // without test isolation issues, but we can verify it initializes and publishes.
        
        // Setup bus
        let bus = EventBusBuilder::new().build().await.unwrap();
        
        // This might fail if another test initialized it first, but we just want to ensure
        // the global bus is populated.
        let _ = set_global_bus(bus);
        
        assert!(get_bus().is_some());
        
        // Publish should succeed
        let res = publish(GlobalTestEvent { id: 42 }).await;
        assert!(res.is_ok());
    }
}
