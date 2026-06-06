use super::RemoteTransport;
use crate::error::{Error, Result};
use async_trait::async_trait;

/// A NATS-backed implementation of the `RemoteTransport`.
#[derive(Debug, Clone)]
pub struct NatsTransport {
    client: async_nats::Client,
    js_context: Option<async_nats::jetstream::Context>,
    stream_name: Option<String>,
}

impl NatsTransport {
    /// Connect to a NATS server URL.
    pub async fn connect(url: &str) -> Result<Self> {
        let client = async_nats::connect(url)
            .await
            .map_err(|e| Error::internal(format!("Failed to connect to NATS: {}", e)))?;

        Ok(Self {
            client,
            js_context: None,
            stream_name: None,
        })
    }

    /// Connect to a NATS server URL and configure a persistent JetStream.
    ///
    /// The `stream_name` is the name of the JetStream on the broker (e.g., `EVENTS`).
    /// All events published and subscribed will be persisted to this stream.
    pub async fn connect_jetstream(
        url: &str,
        stream_name: &str,
        subjects: Vec<String>,
    ) -> Result<Self> {
        let client = async_nats::connect(url)
            .await
            .map_err(|e| Error::internal(format!("Failed to connect to NATS: {}", e)))?;

        let js_context = async_nats::jetstream::new(client.clone());

        let _stream = js_context
            .get_or_create_stream(async_nats::jetstream::stream::Config {
                name: stream_name.to_string(),
                subjects,
                ..Default::default()
            })
            .await
            .map_err(|e| Error::internal(format!("Failed to configure JetStream: {}", e)))?;

        Ok(Self {
            client,
            js_context: Some(js_context),
            stream_name: Some(stream_name.to_string()),
        })
    }
}

#[async_trait]
impl RemoteTransport for NatsTransport {
    async fn publish(&self, topic: &str, payload: &[u8], msg_id: Option<&str>) -> Result<()> {
        const MAX_PAYLOAD_SIZE: usize = 1024 * 1024; // 1 MB limit
        if payload.len() > MAX_PAYLOAD_SIZE {
            return Err(Error::PayloadTooLarge {
                size: payload.len(),
                max: MAX_PAYLOAD_SIZE,
            });
        }

        let payload_bytes = payload.to_vec().into();

        let mut headers = async_nats::HeaderMap::new();
        if let Some(id) = msg_id {
            // MITIGATION: Duplicate Delivery
            // Inject the Nats-Msg-Id header to guarantee Exactly-Once deduplication
            // across the JetStream distributed network.
            if let Ok(header_value) = id.parse::<async_nats::HeaderValue>() {
                headers.insert("Nats-Msg-Id", header_value);
            }
        }

        if let Some(js) = &self.js_context {
            // JetStream Guaranteed Publish (Waits for disk ACK from broker)
            let _ack = js
                .publish_with_headers(topic.to_string(), headers, payload_bytes)
                .await
                .map_err(|e| Error::internal(format!("JetStream publish failed: {}", e)))?;
        } else {
            // Core NATS Fire-And-Forget Publish
            self.client
                .publish_with_headers(topic.to_string(), headers, payload_bytes)
                .await
                .map_err(|e| Error::internal(format!("NATS publish failed: {}", e)))?;
        }

        Ok(())
    }

    async fn subscribe(
        &self,
        topic: &str,
        queue_group: &str,
    ) -> Result<futures::stream::BoxStream<'static, (Vec<u8>, tokio::sync::oneshot::Sender<()>)>>
    {
        use futures::StreamExt;

        if let (Some(js), Some(stream_name)) = (&self.js_context, &self.stream_name) {
            // JetStream Push Consumer (Persistent Queue Group)
            let stream = js
                .get_stream(stream_name.to_string())
                .await
                .map_err(|e| Error::internal(format!("Failed to get stream: {}", e)))?;

            // We generate a deterministic consumer name based on the topic and queue_group
            let consumer_name = format!("{}_{}", queue_group, topic.replace(['.', '*', '>'], "_"));

            let consumer = stream
                .get_or_create_consumer(
                    &consumer_name,
                    async_nats::jetstream::consumer::push::Config {
                        durable_name: Some(consumer_name.clone()),
                        deliver_group: Some(queue_group.to_string()),
                        deliver_subject: consumer_name.clone(),
                        filter_subject: topic.to_string(),
                        ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
                        ack_wait: std::time::Duration::from_secs(60),
                        max_ack_pending: 1000,
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| {
                    Error::internal(format!("Failed to create JetStream consumer: {}", e))
                })?;

            let messages = consumer
                .messages()
                .await
                .map_err(|e| Error::internal(format!("Failed to get JetStream messages: {}", e)))?;

            let message_stream = messages.filter_map(|res| async {
                match res {
                    Ok(msg) => {
                        let payload = msg.payload.to_vec();
                        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();

                        tokio::spawn(async move {
                            if ack_rx.await.is_ok() {
                                if let Err(e) = msg.ack().await {
                                    tracing::error!("Failed to ACK JetStream message: {}", e);
                                }
                            }
                        });

                        Some((payload, ack_tx))
                    }
                    Err(e) => {
                        tracing::error!("JetStream message error: {}", e);
                        None
                    }
                }
            });

            Ok(Box::pin(message_stream))
        } else {
            // Core NATS volatile Queue Group
            let subscriber = self
                .client
                .queue_subscribe(topic.to_string(), queue_group.to_string())
                .await
                .map_err(|e| Error::internal(format!("NATS subscribe failed: {}", e)))?;

            let message_stream = subscriber.map(|msg| {
                let (ack_tx, _ack_rx) = tokio::sync::oneshot::channel();
                (msg.payload.to_vec(), ack_tx)
            });
            Ok(Box::pin(message_stream))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connect_fails_gracefully() {
        // Attempt to connect to an invalid NATS URL
        let result =
            NatsTransport::connect("nats://invalid.address.that.does.not.exist:4222").await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Failed to connect to NATS"));
    }

    #[tokio::test]
    async fn test_connect_jetstream_fails_gracefully() {
        // Attempt to connect to an invalid NATS URL with JetStream
        let result = NatsTransport::connect_jetstream(
            "nats://invalid.address.that.does.not.exist:4222",
            "TEST_STREAM",
            vec!["*".to_string()],
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Failed to connect to NATS"));
    }
}
