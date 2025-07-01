//! Subscription handle for managing subscription lifecycle.

use crate::Result;
use std::fmt;
use std::sync::Arc;
use tokio::sync::oneshot;
use uuid::Uuid;

/// A handle to an active subscription.
///
/// When dropped, the subscription is automatically unsubscribed.
#[derive(Clone)]
pub struct SubscriptionHandle {
    /// Unique ID for this subscription
    id: Uuid,
    
    /// Channel to signal unsubscription
    unsubscribe_sender: Arc<tokio::sync::Mutex<Option<oneshot::Sender<()>>>>,
    
    /// Optional name for debugging
    name: Option<String>,
}

impl SubscriptionHandle {
    /// Create a new subscription handle
    pub(crate) fn new(id: Uuid) -> (Self, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        
        let handle = Self {
            id,
            unsubscribe_sender: Arc::new(tokio::sync::Mutex::new(Some(tx))),
            name: None,
        };
        
        (handle, rx)
    }
    
    /// Create a new subscription handle with a name
    pub(crate) fn with_name(id: Uuid, name: impl Into<String>) -> (Self, oneshot::Receiver<()>) {
        let (mut handle, rx) = Self::new(id);
        handle.name = Some(name.into());
        (handle, rx)
    }
    
    /// Get the subscription ID
    pub fn id(&self) -> Uuid {
        self.id
    }
    
    /// Get the subscription name if set
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
    
    /// Unsubscribe this subscription
    pub async fn unsubscribe(self) -> Result<()> {
        let mut sender = self.unsubscribe_sender.lock().await;
        if let Some(tx) = sender.take() {
            // Send unsubscribe signal
            let _ = tx.send(());
        }
        Ok(())
    }
    
    /// Check if this subscription is still active
    pub async fn is_active(&self) -> bool {
        self.unsubscribe_sender.lock().await.is_some()
    }
}

impl fmt::Debug for SubscriptionHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubscriptionHandle")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish()
    }
}

impl fmt::Display for SubscriptionHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name) => write!(f, "Subscription '{}' ({})", name, self.id),
            None => write!(f, "Subscription {}", self.id),
        }
    }
}

/// Builder for creating subscription handles with configuration
pub struct SubscriptionBuilder {
    name: Option<String>,
    auto_unsubscribe: bool,
}

impl SubscriptionBuilder {
    /// Create a new subscription builder
    pub fn new() -> Self {
        Self {
            name: None,
            auto_unsubscribe: true,
        }
    }
    
    /// Set the subscription name
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
    
    /// Set whether to automatically unsubscribe on drop
    pub fn auto_unsubscribe(mut self, auto: bool) -> Self {
        self.auto_unsubscribe = auto;
        self
    }
    
    /// Build the subscription handle
    pub(crate) fn build(self) -> (SubscriptionHandle, oneshot::Receiver<()>) {
        let id = Uuid::new_v4();
        match self.name {
            Some(name) => SubscriptionHandle::with_name(id, name),
            None => SubscriptionHandle::new(id),
        }
    }
}

impl Default for SubscriptionBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A weak reference to a subscription handle
pub struct WeakSubscriptionHandle {
    id: Uuid,
    name: Option<String>,
}

impl WeakSubscriptionHandle {
    /// Create a weak handle from a subscription handle
    pub fn from_handle(handle: &SubscriptionHandle) -> Self {
        Self {
            id: handle.id,
            name: handle.name.clone(),
        }
    }
    
    /// Get the subscription ID
    pub fn id(&self) -> Uuid {
        self.id
    }
    
    /// Get the subscription name
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_subscription_handle() {
        let (handle, mut rx) = SubscriptionHandle::new(Uuid::new_v4());
        let _id = handle.id();
        
        assert!(handle.is_active().await);
        assert!(rx.try_recv().is_err());
        
        // Unsubscribe
        handle.unsubscribe().await.unwrap();
        
        // Should receive signal
        assert!(rx.try_recv().is_ok());
    }
    
    #[tokio::test]
    async fn test_subscription_with_name() {
        let (handle, _rx) = SubscriptionHandle::with_name(Uuid::new_v4(), "test-subscription");
        
        assert_eq!(handle.name(), Some("test-subscription"));
        assert_eq!(handle.to_string(), format!("Subscription 'test-subscription' ({})", handle.id()));
    }
    
    #[tokio::test]
    async fn test_subscription_builder() {
        let (handle, _rx) = SubscriptionBuilder::new()
            .name("built-subscription")
            .auto_unsubscribe(true)
            .build();
        
        assert_eq!(handle.name(), Some("built-subscription"));
    }
}