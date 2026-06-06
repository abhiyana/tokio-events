#![cfg(feature = "remote")]

use serde::{Deserialize, Serialize};
use tokio_events::prelude::*;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone, Event, Remote)]
#[remote(topic = "user.created.v1")]
struct UserCreated {
    id: Uuid,
    name: String,
}

#[tokio::test]
#[ignore = "Requires local NATS server running on port 4222"]
async fn test_publish_remote() {
    let bus = EventBusBuilder::new()
        .with_nats_jetstream(
            "nats://127.0.0.1:4222",
            "TEST_EVENTS",
            vec!["user.>".to_string()],
        )
        .build()
        .await
        .unwrap();

    // Verify the "Outbox Pattern" behavior: publish_remote MUST route to local subscribers first!
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    let _handle = bus
        .subscribe(move |event: UserCreated| {
            let tx = tx.clone();
            async move {
                tx.send(event).await.unwrap();
            }
        })
        .await
        .unwrap();

    let unique_name = format!("Alice-{}", Uuid::new_v4());
    let event = UserCreated {
        id: Uuid::new_v4(),
        name: unique_name.clone(),
    };

    // This will publish it to NATS *and* route it locally via the outbox pattern
    bus.publish_remote(event).await.unwrap();

    // Verify it was routed locally
    let mut found = false;
    while let Ok(Some(received)) =
        tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await
    {
        if received.name == unique_name {
            found = true;
            break;
        }
    }
    assert!(found, "Did not receive the published event locally");
}

#[derive(Serialize, Deserialize, Debug, Clone, Event, Remote)]
#[remote(topic = "user.updated.v1")]
struct UserUpdated {
    id: Uuid,
    name: String,
}

#[tokio::test]
#[ignore = "Requires local NATS server running on port 4222"]
async fn test_subscribe_remote() {
    let bus = EventBusBuilder::new()
        .with_nats_jetstream(
            "nats://127.0.0.1:4222",
            "TEST_EVENTS",
            vec!["user.>".to_string()],
        )
        .build()
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    // Subscribe to the network using a queue group
    let _handle = bus
        .subscribe_remote("test_queue_group", move |event: UserUpdated| {
            let tx = tx.clone();
            async move {
                tx.send(event).await.unwrap();
            }
        })
        .await
        .unwrap();

    // To test it, we'll manually push a serialized event to NATS, simulating a different microservice
    let unique_name = format!("Bob-{}", Uuid::new_v4());
    let event = UserUpdated {
        id: Uuid::new_v4(),
        name: unique_name.clone(),
    };

    let nats_client = async_nats::connect("nats://127.0.0.1:4222").await.unwrap();
    let payload = serde_json::to_vec(&event).unwrap();
    nats_client
        .publish(UserUpdated::remote_topic().to_string(), payload.into())
        .await
        .unwrap();

    // Verify we received it via the background consumer loop!
    let mut found = false;
    while let Ok(Some(received)) =
        tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await
    {
        if received.name == unique_name {
            found = true;
            break;
        }
    }
    assert!(found, "Did not receive the event via network loop");
}

#[tokio::test]
#[ignore = "Requires local NATS server running on port 4222"]
async fn test_publish_remote_core_nats_opt_out() {
    // Opt-out of JetStream by using with_nats_transport
    let bus = EventBusBuilder::new()
        .with_nats_transport("nats://127.0.0.1:4222")
        .build()
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let _handle = bus
        .subscribe_remote("core_queue_group", move |event: UserCreated| {
            let tx = tx.clone();
            async move {
                tx.send(event).await.unwrap();
            }
        })
        .await
        .unwrap();

    // Publish manually to bypass outbox local routing
    let unique_name = format!("Core-{}", Uuid::new_v4());
    let nats_client = async_nats::connect("nats://127.0.0.1:4222").await.unwrap();
    let event = UserCreated {
        id: Uuid::new_v4(),
        name: unique_name.clone(),
    };
    nats_client
        .publish(
            UserCreated::remote_topic().to_string(),
            serde_json::to_vec(&event).unwrap().into(),
        )
        .await
        .unwrap();

    let mut found = false;
    while let Ok(Some(received)) =
        tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()).await
    {
        if received.name == unique_name {
            found = true;
            break;
        }
    }
    assert!(found, "Did not receive core NATS event");
}

#[tokio::test]
#[ignore = "Requires local NATS server running on port 4222"]
async fn test_network_poison_pill_dlq() {
    let bus = EventBusBuilder::new()
        .with_nats_jetstream(
            "nats://127.0.0.1:4222",
            "TEST_EVENTS",
            vec!["user.>".to_string()],
        )
        .build()
        .await
        .unwrap();

    let mut dlq_rx = bus
        .take_dlq_receiver()
        .await
        .expect("DLQ receiver was missing");

    let _handle = bus
        .subscribe_remote(
            "poison_queue_group",
            move |_event: UserUpdated| async move {},
        )
        .await
        .unwrap();

    // Send malformed JSON to the network topic!
    let nats_client = async_nats::connect("nats://127.0.0.1:4222").await.unwrap();
    nats_client
        .publish(
            UserUpdated::remote_topic().to_string(),
            b"{ bad json [".to_vec().into(),
        )
        .await
        .unwrap();

    // The consumer loop should catch the deserialization error, package it in an envelope, and send it to DLQ.
    let dlq_envelope = tokio::time::timeout(std::time::Duration::from_secs(2), dlq_rx.recv())
        .await
        .expect("Timeout waiting for DLQ")
        .expect("DLQ closed");

    assert_eq!(
        dlq_envelope.payload_bytes(),
        Some(b"{ bad json [".as_slice())
    );
}

#[tokio::test]
#[ignore = "Requires local NATS server running on port 4222"]
async fn test_jetstream_exactly_once_deduplication() {
    let bus = EventBusBuilder::new()
        .with_nats_jetstream(
            "nats://127.0.0.1:4222",
            "TEST_EVENTS",
            vec!["user.>".to_string()],
        )
        .build()
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(10); // larger channel buffer

    let _handle = bus
        .subscribe_remote("dedup_queue_group", move |event: UserCreated| {
            let tx = tx.clone();
            async move {
                tx.send(event).await.unwrap();
            }
        })
        .await
        .unwrap();

    let nats_client = async_nats::connect("nats://127.0.0.1:4222").await.unwrap();
    let js = async_nats::jetstream::new(nats_client);

    let unique_name = format!("Dedup-{}", Uuid::new_v4());
    let event = UserCreated {
        id: Uuid::new_v4(),
        name: unique_name.clone(),
    };
    let payload = serde_json::to_vec(&event).unwrap();

    // Create headers with a static message ID
    let mut headers = async_nats::HeaderMap::new();
    let msg_id = Uuid::new_v4().to_string();
    headers.insert("Nats-Msg-Id", msg_id.as_str());

    // Publish the EXACT same message TWICE
    js.publish_with_headers(
        UserCreated::remote_topic().to_string(),
        headers.clone(),
        payload.clone().into(),
    )
    .await
    .unwrap();
    js.publish_with_headers(
        UserCreated::remote_topic().to_string(),
        headers,
        payload.into(),
    )
    .await
    .unwrap();

    // Receive the first one
    let mut count = 0;
    while let Ok(Some(received)) =
        tokio::time::timeout(std::time::Duration::from_millis(1500), rx.recv()).await
    {
        if received.name == unique_name {
            count += 1;
        }
    }

    assert_eq!(
        count, 1,
        "Expected exactly 1 delivery of the duplicated message!"
    );
}

#[tokio::test]
#[ignore = "Requires local NATS server running on port 4222"]
async fn test_payload_too_large_rejection() {
    let bus = EventBusBuilder::new()
        .with_nats_jetstream(
            "nats://127.0.0.1:4222",
            "TEST_EVENTS",
            vec!["user.>".to_string()],
        )
        .build()
        .await
        .unwrap();

    // Create an event that serializes to slightly more than 1MB
    let huge_name = "A".repeat(1024 * 1024 + 10);
    let event = UserCreated {
        id: Uuid::new_v4(),
        name: huge_name,
    };

    // This should immediately return a PayloadTooLarge error before hitting NATS
    let result = bus.publish_remote(event).await;

    assert!(result.is_err(), "Expected publish to fail for huge payload");
    if let Err(tokio_events::Error::PayloadTooLarge { size, max }) = result {
        assert!(size > 1024 * 1024);
        assert_eq!(max, 1024 * 1024);
    } else {
        panic!("Expected PayloadTooLarge error, got {:?}", result);
    }
}

#[cfg(feature = "protobuf")]
#[derive(Clone, PartialEq, prost::Message, Event, Remote)]
#[event(format = "protobuf")]
#[remote(topic = "user.protobuf.v1")]
struct ProtobufUser {
    #[prost(uint64, tag = "1")]
    pub id: u64,
    #[prost(string, tag = "2")]
    pub name: String,
}

#[cfg(feature = "protobuf")]
#[tokio::test]
async fn test_protobuf_serialization() {
    let bus = EventBusBuilder::new().build().await.unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    let _handle = bus
        .subscribe(move |event: ProtobufUser| {
            let tx = tx.clone();
            async move {
                tx.send(event).await.unwrap();
            }
        })
        .await
        .unwrap();

    let event = ProtobufUser {
        id: 42,
        name: "Proto Bob".to_string(),
    };

    // Verify it compiles, serializes via prost, and routes back
    bus.publish(event).await.unwrap();

    let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .expect("Did not receive event");

    assert_eq!(received.id, 42);
    assert_eq!(received.name, "Proto Bob");
}

struct MockDelayTransport {
    pub published_topics: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl tokio_events::remote::RemoteTransport for MockDelayTransport {
    async fn publish(
        &self,
        topic: &str,
        _payload: &[u8],
        _msg_id: Option<&str>,
    ) -> tokio_events::Result<()> {
        self.published_topics
            .lock()
            .unwrap()
            .push(topic.to_string());
        Ok(())
    }

    async fn subscribe(
        &self,
        _topic: &str,
        _queue_group: &str,
    ) -> tokio_events::Result<
        futures::stream::BoxStream<'static, (Vec<u8>, tokio::sync::oneshot::Sender<()>)>,
    > {
        Ok(Box::pin(futures::stream::empty()))
    }
}

#[tokio::test]
async fn test_publish_remote_delayed_mock() {
    let published_topics = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mock = MockDelayTransport {
        published_topics: published_topics.clone(),
    };

    let bus = EventBusBuilder::new()
        .with_custom_transport(std::sync::Arc::new(mock))
        .build()
        .await
        .unwrap();

    let event = UserCreated {
        id: Uuid::new_v4(),
        name: "DelayedAlice".to_string(),
    };

    // Publish with 200ms delay
    bus.publish_remote_delayed(event, std::time::Duration::from_millis(200))
        .await
        .unwrap();

    // Check instantly - should be empty
    assert_eq!(published_topics.lock().unwrap().len(), 0);

    // Wait 250ms
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    // Check again - should have been published!
    let guard = published_topics.lock().unwrap();
    assert_eq!(guard.len(), 1);
    assert_eq!(guard[0], "user.created.v1");
}
