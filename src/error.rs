//! Error types for the tokio-events library.

use std::fmt;
use thiserror::Error;
use uuid::Uuid;

/// Type alias for Results in this crate
pub type Result<T> = std::result::Result<T, Error>;

/// Main error type for tokio-events
#[derive(Error, Debug)]
pub enum Error {
    /// Event publishing failed
    #[error("Failed to publish event: {0}")]
    PublishError(String),

    /// Subscription failed
    #[error("Failed to subscribe: {0}")]
    SubscriptionError(String),

    /// Event not found in registry
    #[error("Event type not registered: {type_name}")]
    EventNotRegistered { type_name: &'static str },

    /// Subscription not found
    #[error("Subscription not found: {id}")]
    SubscriptionNotFound { id: Uuid },

    /// Event handler error
    #[error("Handler error: {0}")]
    HandlerError(String),

    /// Event bus is shutting down
    #[error("Event bus is shutting down")]
    ShuttingDown,

    /// Channel send error
    #[error("Failed to send through channel")]
    ChannelSendError,

    /// Channel receive error
    #[error("Failed to receive from channel")]
    ChannelReceiveError,

    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Generic internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Create a new internal error with a custom message
    pub fn internal(msg: impl Into<String>) -> Self {
        Error::Internal(msg.into())
    }

    /// Create a new handler error
    pub fn handler(msg: impl Into<String>) -> Self {
        Error::HandlerError(msg.into())
    }

    /// Check if this error indicates the system is shutting down
    pub fn is_shutdown(&self) -> bool {
        matches!(self, Error::ShuttingDown)
    }

    /// Check if this is a channel-related error
    pub fn is_channel_error(&self) -> bool {
        matches!(
            self,
            Error::ChannelSendError | Error::ChannelReceiveError
        )
    }
}

/// Error context for debugging
#[derive(Debug)]
pub struct ErrorContext {
    pub event_id: Option<Uuid>,
    pub event_type: Option<&'static str>,
    pub handler_name: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl Default for ErrorContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ErrorContext {
    /// Create a new error context
    pub fn new() -> Self {
        Self {
            event_id: None,
            event_type: None,
            handler_name: None,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Set the event ID
    pub fn with_event_id(mut self, id: Uuid) -> Self {
        self.event_id = Some(id);
        self
    }

    /// Set the event type
    pub fn with_event_type(mut self, event_type: &'static str) -> Self {
        self.event_type = Some(event_type);
        self
    }

    /// Set the handler name
    pub fn with_handler(mut self, name: impl Into<String>) -> Self {
        self.handler_name = Some(name.into());
        self
    }
}

impl fmt::Display for ErrorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Error at {}", self.timestamp)?;
        if let Some(id) = &self.event_id {
            write!(f, " [event_id: {}]", id)?;
        }
        if let Some(event_type) = &self.event_type {
            write!(f, " [type: {}]", event_type)?;
        }
        if let Some(handler) = &self.handler_name {
            write!(f, " [handler: {}]", handler)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = Error::PublishError("test error".to_string());
        assert_eq!(err.to_string(), "Failed to publish event: test error");
    }

    #[test]
    fn test_error_is_shutdown() {
        assert!(Error::ShuttingDown.is_shutdown());
        assert!(!Error::internal("test").is_shutdown());
    }

    #[test]
    fn test_error_context() {
        let ctx = ErrorContext::new()
            .with_event_id(Uuid::max())
            .with_event_type("TestEvent")
            .with_handler("test_handler");
        
        let display = ctx.to_string();
        assert!(display.contains("TestEvent"));
        assert!(display.contains("test_handler"));
    }
}