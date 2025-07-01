//! Event handler traits and implementations.

use crate::{Error, Event, EventEnvelope, Result};
use async_trait::async_trait;
use std::fmt::{self, Debug};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Type alias for a boxed future that handlers return
pub type HandlerFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

/// Trait for event handlers that can process events asynchronously.
#[async_trait]
pub trait EventHandler: Send + Sync  + 'static {
    /// Process an event envelope
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()>;

    /// Get the handler name for debugging
    fn name(&self) -> &str {
        "unnamed"
    }

    /// Called when the handler is being shut down
    async fn on_shutdown(&self) -> Result<()> {
        Ok(())
    }
}

/// A typed event handler that processes specific event types.
pub trait TypedHandler<T: Event>: Send + Sync  + 'static {
    /// Handle a specific event type
    fn handle_typed(&self, event: &T) -> HandlerFuture;

    /// Get the handler name
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }
}

/// Adapter that converts a TypedHandler into an EventHandler
pub struct TypedHandlerAdapter<T: Event, H: TypedHandler<T>> {
    handler: Arc<H>,
    _phantom: std::marker::PhantomData<T>,
}

impl<T: Event, H: TypedHandler<T>> TypedHandlerAdapter<T, H> {
    /// Create a new typed handler adapter
    pub fn new(handler: H) -> Self {
        Self {
            handler: Arc::new(handler),
            _phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<T: Event, H: TypedHandler<T>> EventHandler for TypedHandlerAdapter<T, H> {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        if let Some(event) = envelope.downcast_ref::<T>() {
            self.handler.handle_typed(event).await
        } else {
            Err(Error::EventNotRegistered {
                type_name: envelope.event_type(),
            })
        }
    }

    fn name(&self) -> &str {
        self.handler.name()
    }
}

/// A function-based event handler using closures.
pub struct FunctionHandler<T, F, Fut>
where
    T: Event,
    F: Fn(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    function: F,
    name: String,
    _phantom: std::marker::PhantomData<T>,
}

impl<T, F, Fut> FunctionHandler<T, F, Fut>
where
    T: Event,
    F: Fn(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    /// Create a new function handler
    pub fn new(function: F) -> Self {
        Self {
            function,
            name: format!("FunctionHandler<{}>", T::event_type()),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Create a new function handler with a custom name
    pub fn with_name(function: F, name: impl Into<String>) -> Self {
        Self {
            function,
            name: name.into(),
            _phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<T, F, Fut> EventHandler for FunctionHandler<T, F, Fut>
where
    T: Event,
    F: Fn(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        if let Some(event) = envelope.downcast_ref::<T>() {
            (self.function)(event.clone()).await;
            Ok(())
        } else {
            Err(Error::EventNotRegistered {
                type_name: envelope.event_type(),
            })
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// A handler that can filter events before processing
pub struct FilteredHandler<H: EventHandler> {
    inner: H,
    filter: Box<dyn Fn(&EventEnvelope) -> bool + Send + Sync>,
    filter_name: String,
}

impl<H: EventHandler> FilteredHandler<H> {
    /// Create a new filtered handler
    pub fn new<F>(inner: H, filter: F, filter_name: impl Into<String>) -> Self
    where
        F: Fn(&EventEnvelope) -> bool + Send + Sync + 'static,
    {
        Self {
            inner,
            filter: Box::new(filter),
            filter_name: filter_name.into(),
        }
    }
}

#[async_trait]
impl<H: EventHandler> EventHandler for FilteredHandler<H> {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        if (self.filter)(envelope) {
            self.inner.handle(envelope).await
        } else {
            Ok(())
        }
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}

/// A handler that wraps errors and continues processing
pub struct ErrorWrappingHandler<H: EventHandler> {
    inner: H,
    error_handler: Box<dyn Fn(Error) + Send + Sync>,
}

impl<H: EventHandler> ErrorWrappingHandler<H> {
    /// Create a new error wrapping handler
    pub fn new<E>(inner: H, error_handler: E) -> Self
    where
        E: Fn(Error) + Send + Sync + 'static,
    {
        Self {
            inner,
            error_handler: Box::new(error_handler),
        }
    }
}

#[async_trait]
impl<H: EventHandler> EventHandler for ErrorWrappingHandler<H> {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        match self.inner.handle(envelope).await {
            Ok(()) => Ok(()),
            Err(e) => {
                (self.error_handler)(e);
                Ok(())
            }
        }
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}

/// Handler statistics for monitoring
#[derive( Clone, Default)]
pub struct HandlerStats {
    pub events_processed: u64,
    pub events_failed: u64,
    pub total_processing_time_ms: u64,
    pub last_error: Option<String>,
}

impl fmt::Display for HandlerStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Processed: {}, Failed: {}, Avg time: {}ms",
            self.events_processed,
            self.events_failed,
            if self.events_processed > 0 {
                self.total_processing_time_ms / self.events_processed
            } else {
                0
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct TestEvent {
        value: i32,
    }

    impl Event for TestEvent {
        fn event_type() -> &'static str {
            "TestEvent"
        }
    }

    #[tokio::test]
    async fn test_function_handler() {
        let handler = FunctionHandler::new(|event: TestEvent| async move {
            assert_eq!(event.value, 42);
        });

        let event = TestEvent { value: 42 };
        let envelope = EventEnvelope::new(event);

        handler.handle(&envelope).await.unwrap();
    }

    #[tokio::test]
    async fn test_filtered_handler() {
        let base_handler = FunctionHandler::new(|_: TestEvent| async move {
            // This should only be called for events with value > 10
        });

        let filtered = FilteredHandler::new(
            base_handler,
            |envelope: &EventEnvelope| {
                envelope
                    .downcast_ref::<TestEvent>()
                    .map(|e| e.value > 10)
                    .unwrap_or(false)
            },
            "value > 10",
        );

        // This should be processed
        let event1 = TestEvent { value: 20 };
        let envelope1 = EventEnvelope::new(event1);
        assert!(filtered.handle(&envelope1).await.is_ok());

        // This should be filtered out
        let event2 = TestEvent { value: 5 };
        let envelope2 = EventEnvelope::new(event2);
        assert!(filtered.handle(&envelope2).await.is_ok());
    }
}
