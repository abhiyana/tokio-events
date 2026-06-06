//! Channel-based event dispatcher implementation.

use super::{DispatcherConfig, DispatcherStats, EventDispatcher};
use crate::subscription::SubscriptionManager;
use crate::{Error, EventEnvelope, Result};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{error, info, trace, warn};

/// A channel-based event dispatcher.
///
/// This dispatcher uses Tokio channels to queue events and process them
/// asynchronously. It provides backpressure and can be configured to
/// drop events when the queue is full.
#[allow(missing_debug_implementations)]
pub struct ChannelDispatcher {
    /// Configuration
    config: DispatcherConfig,

    /// Event queue sender
    sender: Option<mpsc::Sender<Arc<EventEnvelope>>>,

    /// Channel for receiving events (moved to worker)
    receiver: Option<mpsc::Receiver<Arc<EventEnvelope>>>,

    /// Subscription manager
    subscription_manager: Arc<SubscriptionManager>,

    /// Worker task handle
    worker_handle: Option<JoinHandle<()>>,

    /// Running state
    is_running: Arc<AtomicBool>,

    /// Statistics
    events_dispatched: Arc<AtomicU64>,
    dispatch_errors: Arc<AtomicU64>,
    total_dispatch_time_us: Arc<AtomicU64>,
    max_queue_size: Arc<AtomicU64>,
}

impl ChannelDispatcher {
    /// Create a new channel dispatcher
    pub fn new(config: DispatcherConfig, subscription_manager: Arc<SubscriptionManager>) -> Self {
        let (sender, receiver) = mpsc::channel(config.max_queue_size);

        Self {
            config,
            sender: Some(sender),
            receiver: Some(receiver),
            subscription_manager,
            worker_handle: None,
            is_running: Arc::new(AtomicBool::new(false)),
            events_dispatched: Arc::new(AtomicU64::new(0)),
            dispatch_errors: Arc::new(AtomicU64::new(0)),
            total_dispatch_time_us: Arc::new(AtomicU64::new(0)),
            max_queue_size: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Get a sender for this dispatcher
    pub fn sender(&self) -> Option<mpsc::Sender<Arc<EventEnvelope>>> {
        self.sender.clone()
    }

    /// Process events from the channel
    async fn process_events(
        mut receiver: mpsc::Receiver<Arc<EventEnvelope>>,
        subscription_manager: Arc<SubscriptionManager>,
        is_running: Arc<AtomicBool>,
        events_dispatched: Arc<AtomicU64>,
        dispatch_errors: Arc<AtomicU64>,
        total_dispatch_time_us: Arc<AtomicU64>,
        config: DispatcherConfig,
    ) {
        info!("Event dispatcher worker started");

        // Continuously process events until the channel is closed
        while let Some(event) = receiver.recv().await {
            // Also stop if is_running becomes false abruptly
            if !is_running.load(Ordering::SeqCst) {
                break;
            }

            trace!(
                event_id = %event.event_id(),
                event_type = %event.event_type(),
                "Processing event from queue"
            );

            let start = Instant::now();

            // Dispatch with timeout
            let dispatch_result = if config.processing_timeout_ms > 0 {
                tokio::time::timeout(
                    tokio::time::Duration::from_millis(config.processing_timeout_ms),
                    subscription_manager.dispatch(event.clone()),
                )
                .await
                .unwrap_or_else(|_| {
                    error!("Event dispatch timed out");
                    Err(Error::internal("Dispatch timeout"))
                })
            } else {
                subscription_manager.dispatch(event.clone()).await
            };

            let elapsed_us = start.elapsed().as_micros() as u64;

            match dispatch_result {
                Ok(()) => {
                    events_dispatched.fetch_add(1, Ordering::Relaxed);
                    total_dispatch_time_us.fetch_add(elapsed_us, Ordering::Relaxed);

                    trace!(
                        event_id = %event.event_id(),
                        dispatch_time_us = elapsed_us,
                        "Event dispatched successfully"
                    );
                }
                Err(e) => {
                    dispatch_errors.fetch_add(1, Ordering::Relaxed);
                    error!(
                        event_id = %event.event_id(),
                        error = %e,
                        "Failed to dispatch event"
                    );
                }
            }
        }

        info!("Event dispatcher worker stopped");
    }
}

#[async_trait]
impl EventDispatcher for ChannelDispatcher {
    async fn start(&mut self) -> Result<()> {
        if self.is_running.load(Ordering::SeqCst) {
            return Err(Error::internal("Dispatcher already running"));
        }

        info!("Starting channel dispatcher");

        // Take the receiver (can only start once)
        let receiver = self
            .receiver
            .take()
            .ok_or_else(|| Error::internal("Dispatcher already started"))?;

        // Mark as running
        self.is_running.store(true, Ordering::SeqCst);

        // Start worker task
        let subscription_manager = self.subscription_manager.clone();
        let is_running = self.is_running.clone();
        let events_dispatched = self.events_dispatched.clone();
        let dispatch_errors = self.dispatch_errors.clone();
        let total_dispatch_time_us = self.total_dispatch_time_us.clone();
        let config = self.config.clone();

        let handle = tokio::spawn(async move {
            Self::process_events(
                receiver,
                subscription_manager,
                is_running,
                events_dispatched,
                dispatch_errors,
                total_dispatch_time_us,
                config,
            )
            .await;
        });

        self.worker_handle = Some(handle);

        info!("Channel dispatcher started");
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if !self.is_running.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Stopping channel dispatcher");

        // Signal stop
        self.is_running.store(false, Ordering::SeqCst);
        self.sender.take(); // Close the channel to unblock the worker

        // Wait for worker to finish
        if let Some(handle) = self.worker_handle.take() {
            // Give it some time to finish processing
            let _ = tokio::time::timeout(tokio::time::Duration::from_secs(5), handle)
                .await
                .map_err(|_| Error::internal("Worker shutdown timeout"))?;
        }

        info!("Channel dispatcher stopped");
        Ok(())
    }

    async fn shutdown_gracefully(&mut self) -> Result<()> {
        info!("Shutting down channel dispatcher gracefully");

        // Close the sender channel so the receiver will get `None` when empty
        self.sender.take();

        // Wait for worker to finish draining the queue
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle
                .await
                .map_err(|e| Error::internal(format!("Worker panicked: {}", e)));
        }

        self.is_running.store(false, Ordering::SeqCst);
        info!("Channel dispatcher graceful shutdown complete");
        Ok(())
    }

    async fn dispatch(&self, envelope: EventEnvelope) -> Result<()> {
        if !self.is_running.load(Ordering::SeqCst) {
            return Err(Error::internal("Dispatcher not running"));
        }

        // If the event is scheduled for the future, spawn a sleep task.
        if let Some(deliver_at) = envelope.metadata.deliver_at {
            let now = chrono::Utc::now();
            if deliver_at > now {
                if let Ok(delay) = (deliver_at - now).to_std() {
                    tracing::warn!(
                        "Event {} scheduled for {:?} using in-memory ChannelDispatcher. \
                        If the process crashes before this time, the event will be lost permanently. \
                        Enable the 'persistence' feature and use RedbDispatcher for durable scheduled events.",
                        envelope.event_id(),
                        deliver_at
                    );
                    let sender_opt = self.sender.clone();
                    let envelope_arc = Arc::new(envelope);

                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        // After waking up, send it to the queue
                        if let Some(sender) = sender_opt {
                            let _ = sender.send(envelope_arc).await;
                        }
                    });

                    return Ok(());
                }
            }
        }

        let envelope = Arc::new(envelope);

        let sender = self.sender.as_ref().ok_or_else(|| Error::ShuttingDown)?;

        // Update max queue size
        let current_size = sender.max_capacity().saturating_sub(sender.capacity());
        let max_size = self.max_queue_size.load(Ordering::Relaxed);
        if current_size as u64 > max_size {
            self.max_queue_size
                .store(current_size as u64, Ordering::Relaxed);
        }

        if self.config.drop_on_full {
            match sender.try_send(envelope) {
                Ok(()) => Ok(()),
                Err(mpsc::error::TrySendError::Full(_)) => {
                    warn!("Event queue full, dropping event");
                    self.dispatch_errors.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    Err(Error::internal("Event channel closed"))
                }
            }
        } else {
            match sender.send(envelope).await {
                Ok(()) => Ok(()),
                Err(_) => Err(Error::internal("Event channel closed")),
            }
        }
    }

    fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    fn stats(&self) -> DispatcherStats {
        let events_dispatched = self.events_dispatched.load(Ordering::Relaxed);
        let total_time = self.total_dispatch_time_us.load(Ordering::Relaxed);
        let current_queue = self
            .sender
            .as_ref()
            .map(|s| s.max_capacity() - s.capacity())
            .unwrap_or(0);

        DispatcherStats {
            events_dispatched,
            queue_size: current_queue,
            dispatch_errors: self.dispatch_errors.load(Ordering::Relaxed),
            avg_dispatch_time_us: total_time.checked_div(events_dispatched).unwrap_or(0),
            max_queue_size: self.max_queue_size.load(Ordering::Relaxed) as usize,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::DashMapRegistry;
    use crate::Event;

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct TestEvent {
        value: i32,
    }

    impl Event for TestEvent {
        fn event_type() -> &'static str {
            "TestEvent"
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
    async fn test_channel_dispatcher() {
        // Create components
        let registry = Arc::new(DashMapRegistry::new());
        let subscription_manager = Arc::new(SubscriptionManager::new(
            registry,
            0,
            std::time::Duration::from_millis(10),
        ));

        let config = DispatcherConfig::new()
            .max_queue_size(100)
            .processing_timeout_ms(1000);

        let mut dispatcher = ChannelDispatcher::new(config, subscription_manager.clone());

        // Start dispatcher
        dispatcher.start().await.unwrap();
        assert!(dispatcher.is_running());

        // Subscribe a handler
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();

        let _handle = subscription_manager
            .subscribe_fn::<TestEvent, _, _>(move |event| {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(event.value as u64, Ordering::Relaxed);
                }
            })
            .await
            .unwrap();

        // Dispatch some events
        for i in 1..=5 {
            let event = TestEvent { value: i };
            let envelope = EventEnvelope::new(event);
            dispatcher.dispatch(envelope).await.unwrap();
        }

        // Wait for processing
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Check results
        assert_eq!(counter.load(Ordering::Relaxed), 15); // 1+2+3+4+5

        let stats = dispatcher.stats();
        assert_eq!(stats.events_dispatched, 5);
        assert_eq!(stats.dispatch_errors, 0);

        // Stop dispatcher
        dispatcher.stop().await.unwrap();
        assert!(!dispatcher.is_running());
    }
}
