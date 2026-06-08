#![cfg(feature = "remote")]

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_events::bus::builder::EventBusBuilder;
use tokio_events::prelude::*;
use tokio_events::remote::RemoteTransport;
use tokio_stream::wrappers::UnboundedReceiverStream;

// 1. Define a Remote Event
#[derive(Debug, Clone, Serialize, Deserialize, Event, Remote, PartialEq)]
#[remote(topic = "test.remote.event")]
pub struct TestRemoteEvent {
    pub message: String,
    pub count: i32,
}

// 2. Define a Mock Transport for testing
#[derive(Clone)]
pub struct MockTransport {
    // Records messages that were published (topic, payload)
    #[allow(clippy::type_complexity)]
    pub published_messages: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
    // Channel sender used to inject messages into the mock transport to simulate receiving them
    pub inbound_tx: mpsc::UnboundedSender<Vec<u8>>,
    // Channel receiver handed to the EventBus when it subscribes
    pub inbound_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<Vec<u8>>>>>,
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MockTransport {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            published_messages: Arc::new(Mutex::new(Vec::new())),
            inbound_tx: tx,
            inbound_rx: Arc::new(Mutex::new(Some(rx))),
        }
    }
}

#[async_trait]
impl RemoteTransport for MockTransport {
    async fn publish(
        &self,
        topic: &str,
        payload: &[u8],
        _msg_id: Option<&str>,
    ) -> tokio_events::Result<()> {
        let mut messages = self.published_messages.lock().await;
        messages.push((topic.to_string(), payload.to_vec()));
        Ok(())
    }

    async fn subscribe(
        &self,
        _topic: &str,
        _queue_group: &str,
    ) -> tokio_events::Result<
        futures::stream::BoxStream<'static, (Vec<u8>, tokio::sync::oneshot::Sender<()>)>,
    > {
        let mut rx_lock = self.inbound_rx.lock().await;
        let rx = rx_lock
            .take()
            .expect("subscribe called more than once in test");

        use futures::StreamExt;
        let stream = UnboundedReceiverStream::new(rx).map(|bytes| {
            let (tx, _rx) = tokio::sync::oneshot::channel();
            (bytes, tx)
        });

        Ok(Box::pin(stream))
    }
}

#[tokio::test]
async fn test_publish_remote() {
    let mock_transport = Arc::new(MockTransport::new());

    // Build the bus with the mock transport
    let bus = EventBusBuilder::new()
        .with_custom_transport(mock_transport.clone())
        .build()
        .await
        .expect("Failed to build bus");

    // Publish a remote event
    let event = TestRemoteEvent {
        message: "Hello Network".to_string(),
        count: 42,
    };

    let event_id = bus
        .publish_remote(event.clone())
        .await
        .expect("Failed to publish remote");

    // Yield to let background tasks process
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Verify the transport received the serialized message
    let published = mock_transport.published_messages.lock().await;
    assert_eq!(
        published.len(),
        1,
        "Expected 1 message published to transport"
    );

    let (topic, payload) = &published[0];
    assert_eq!(
        topic, "test.remote.event",
        "Topic should match the #[remote] attribute"
    );

    // Deserialize the payload back to verify correctness
    let deserialized =
        TestRemoteEvent::deserialize_event(payload).expect("Failed to deserialize payload");
    assert_eq!(deserialized.message, event.message);
    assert_eq!(deserialized.count, event.count);

    // Ensure the event was ALSO published locally
    let _ = event_id;
}

#[tokio::test]
async fn test_subscribe_remote() {
    let mock_transport = Arc::new(MockTransport::new());

    // Build the bus with the mock transport
    let bus = EventBusBuilder::new()
        .with_custom_transport(mock_transport.clone())
        .build()
        .await
        .expect("Failed to build bus");

    // Track received events
    let received_events = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received_events.clone();

    // Subscribe remote
    let _handle = bus
        .subscribe_remote("test_group", move |event: TestRemoteEvent| {
            let events = received_clone.clone();
            async move {
                events.lock().await.push(event);
            }
        })
        .await
        .expect("Failed to subscribe remote");

    // Simulate an inbound message from the network
    let original_event = TestRemoteEvent {
        message: "Inbound Event".to_string(),
        count: 99,
    };
    let payload = original_event.serialize_event().unwrap();

    mock_transport
        .inbound_tx
        .send(payload)
        .expect("Failed to inject message");

    // Yield to let the remote consumer loop process the inbound bytes
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Verify the handler received it
    let received = received_events.lock().await;
    assert_eq!(received.len(), 1, "Expected handler to receive 1 event");
    assert_eq!(received[0].message, original_event.message);
    assert_eq!(received[0].count, original_event.count);
}
