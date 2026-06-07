//! Global singleton event bus access.
//!
//! This module provides a convenient way to access a single `EventBus` instance
//! from anywhere in your application without passing `Arc<EventBus>` around.

use crate::{Error, Event, EventBus, Result};
use std::sync::OnceLock;

static GLOBAL_BUS: OnceLock<EventBus> = OnceLock::new();

/// Sets the global singleton event bus instance.
///
/// This function should typically be called once during application startup.
/// Subsequent calls will return an error containing the `EventBus` that was passed in.
///
/// # Examples
///
/// ```rust,ignore
/// let bus = EventBusBuilder::new().build().await?;
/// tokio_events::global::set_global_bus(bus).expect("Bus already initialized");
/// ```
#[allow(clippy::result_large_err)]
pub fn set_global_bus(bus: EventBus) -> std::result::Result<(), EventBus> {
    GLOBAL_BUS.set(bus)
}

/// Gets a reference to the global event bus.
///
/// # Returns
///
/// Returns `Some(&EventBus)` if the global bus was previously initialized via 
/// `set_global_bus`, otherwise returns `None`.
pub fn get_bus() -> Option<&'static EventBus> {
    GLOBAL_BUS.get()
}

/// Publishes an event to the globally registered event bus.
///
/// This is a highly convenient wrapper around `get_bus().unwrap().publish()`.
///
/// # Examples
///
/// ```rust,ignore
/// // Anywhere in your application, without needing an Arc<EventBus>:
/// tokio_events::global::publish(UserCreated { id: 1 }).await?;
/// ```
///
/// # Errors
///
/// Returns an error if the global event bus has not been initialized via `set_global_bus`.
pub async fn publish<E: Event>(event: E) -> Result<uuid::Uuid> {
    if let Some(bus) = GLOBAL_BUS.get() {
        bus.publish(event).await
    } else {
        Err(Error::internal(
            "Global event bus not initialized. Call tokio_events::global::set_global_bus() first.",
        ))
    }
}

/// Shuts down the global event bus abruptly.
///
/// This immediately halts the dispatcher, drops any events currently sitting in the
/// memory queue, and shuts down all background workers.
///
/// # Errors
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
/// This method performs an orchestrated shutdown of the global instance:
/// 1. Rejects new `publish()` calls.
/// 2. Executes all registered shutdown hooks.
/// 3. Signals the dispatcher to finish processing currently queued events.
/// 4. Shuts down the subscription manager.
///
/// # Errors
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
        fn serialize_event(&self) -> crate::Result<Vec<u8>> {
            serde_json::to_vec(self).map_err(|e| crate::Error::SerializationError(e.to_string()))
        }
        fn deserialize_event(bytes: &[u8]) -> crate::Result<Self> {
            serde_json::from_slice(bytes)
                .map_err(|e| crate::Error::SerializationError(e.to_string()))
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
