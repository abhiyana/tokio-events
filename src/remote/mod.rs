use crate::error::Result;
use async_trait::async_trait;

#[cfg(feature = "remote")]
/// NATS transport engine implementation
pub mod nats;

/// A transport layer for sending and receiving distributed events.
#[cfg(feature = "remote")]
#[cfg_attr(docsrs, doc(cfg(feature = "remote")))]
#[async_trait]
pub trait RemoteTransport: Send + Sync + 'static {
    /// Publish raw serialized bytes to a network topic.
    ///
    /// The `msg_id` is an optional unique identifier used for exactly-once
    /// deduplication on the broker (e.g., NATS JetStream `Nats-Msg-Id`).
    async fn publish(&self, topic: &str, payload: &[u8], msg_id: Option<&str>) -> Result<()>;

    /// Subscribe to a network topic with a specific consumer queue group.
    ///
    /// The `queue_group` ensures that if multiple instances of this microservice are running,
    /// the broker load-balances the events so each event is only processed by one instance.
    ///
    /// Returns a stream of raw bytes received from the network.
    async fn subscribe(&self, topic: &str, queue_group: &str) -> Result<futures::stream::BoxStream<'static, Vec<u8>>>;
}

#[cfg(test)]
#[cfg(feature = "remote")]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use futures::stream;

    struct MockTransport {
        published: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
    }

    #[async_trait]
    impl RemoteTransport for MockTransport {
        async fn publish(&self, topic: &str, payload: &[u8], _msg_id: Option<&str>) -> Result<()> {
            self.published.lock().unwrap().push((topic.to_string(), payload.to_vec()));
            Ok(())
        }

        async fn subscribe(&self, _topic: &str, _queue_group: &str) -> Result<futures::stream::BoxStream<'static, Vec<u8>>> {
            Ok(Box::pin(stream::empty()))
        }
    }

    #[tokio::test]
    async fn test_mock_transport_implementation() {
        let published = Arc::new(Mutex::new(Vec::new()));
        let transport = MockTransport { published: published.clone() };
        
        transport.publish("user.created", b"{}", Some("uuid")).await.unwrap();
        
        let guard = published.lock().unwrap();
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0].0, "user.created");
        assert_eq!(guard[0].1, b"{}");
    }
}
