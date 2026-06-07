//! Configuration for the event bus.

use crate::dispatcher::DispatcherConfig;
use std::time::Duration;

/// Configuration for the event bus
#[derive(Debug, Clone)]
pub struct EventBusConfig {
    /// Dispatcher configuration
    pub dispatcher: DispatcherConfig,

    /// Maximum number of retry attempts for failed handlers
    pub max_retries: u32,

    /// Retry backoff base duration
    pub retry_backoff: Duration,

    /// Shutdown timeout
    pub shutdown_timeout: Duration,

    /// Enable tracing
    pub enable_tracing: bool,

    /// Per-handler channel buffer size.
    ///
    /// Each subscription gets its own channel from the dispatcher. This controls
    /// how many events can be buffered per handler before backpressure kicks in.
    /// Too small → deadlocks under load. Too large → memory waste.
    pub handler_channel_size: usize,

    /// Dead Letter Queue channel buffer size.
    ///
    /// Controls how many permanently-failed events can be buffered in the DLQ
    /// before backpressure cascades into the retry loop.
    pub dlq_channel_size: usize,

    /// Whether publish() should wait for disk persistence
    ///
    /// If true, `.publish()` will block until the event is durably written to the
    /// physical hard drive. This guarantees zero data loss but reduces publish throughput.
    /// If false (default), `.publish()` returns instantly as soon as the event hits the
    /// memory queue.
    pub wait_for_persistence: bool,

    /// How often the persistent scheduler polls for delayed events.
    /// Default is 1 second. Lowering this increases precision but uses more CPU.
    pub scheduler_tick_rate: Duration,
}

impl Default for EventBusConfig {
    fn default() -> Self {
        Self {
            dispatcher: DispatcherConfig::default(),
            max_retries: 3,
            retry_backoff: Duration::from_millis(100),
            shutdown_timeout: Duration::from_secs(30),
            enable_tracing: true,
            handler_channel_size: 256,
            dlq_channel_size: 1000,
            wait_for_persistence: false,
            scheduler_tick_rate: Duration::from_secs(1),
        }
    }
}

impl EventBusConfig {
    /// Create a new configuration with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of retry attempts for failed handlers.
    ///
    /// If an event handler returns an `Err(_)`, the event bus will wait for the
    /// `retry_backoff` duration and attempt to execute the handler again.
    /// Once this limit is reached, the event is permanently sent to the Dead Letter Queue.
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Set the fixed retry backoff duration.
    ///
    /// This is the exact amount of time the dispatcher will wait between retry
    /// attempts for a failed event handler.
    pub fn retry_backoff(mut self, backoff: Duration) -> Self {
        self.retry_backoff = backoff;
        self
    }

    /// Set the timeout duration for graceful shutdown.
    ///
    /// When `bus.shutdown_gracefully()` is called, the bus will wait up to this duration
    /// for all pending events in the queues to be fully processed by their handlers.
    /// If the timeout is reached, the bus will forcefully abort remaining tasks.
    ///
    /// Default is `30 seconds`.
    pub fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    /// Enable or disable `tracing` span generation.
    ///
    /// If `true` (default), the event bus will emit detailed structured logs and spans
    /// for every published and processed event using the `tracing` crate. This is 
    /// highly recommended for debugging but can be disabled for absolute maximum performance.
    pub fn enable_tracing(mut self, enable: bool) -> Self {
        self.enable_tracing = enable;
        self
    }

    /// Tune the core underlying `DispatcherConfig`.
    ///
    /// The dispatcher is the engine that routes events to their target channels.
    /// Use this method to configure extremely low-level details like the thread pool size,
    /// absolute maximum queue capacity, and load shedding behavior.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let config = EventBusConfig::new()
    ///     .dispatcher_config(|d| {
    ///         d.worker_threads(4)
    ///          .max_queue_size(10_000)
    ///          .drop_on_full(false)
    ///     });
    /// ```
    pub fn dispatcher_config<F>(mut self, f: F) -> Self
    where
        F: FnOnce(DispatcherConfig) -> DispatcherConfig,
    {
        self.dispatcher = f(self.dispatcher);
        self
    }

    /// Set the per-handler channel buffer size.
    ///
    /// Each subscription gets its own Tokio MPSC channel from the dispatcher. This size
    /// determines how many events can queue up for a single slow handler before backpressure 
    /// is applied to the dispatcher.
    ///
    /// - **Too small**: The dispatcher might block or drop events under sudden load spikes.
    /// - **Too large**: Wastes memory if you have thousands of subscriptions.
    pub fn handler_channel_size(mut self, size: usize) -> Self {
        self.handler_channel_size = size;
        self
    }

    /// Set the Dead Letter Queue (DLQ) channel buffer size.
    ///
    /// Controls how many permanently failed events can buffer in the DLQ channel
    /// before backpressure cascades back into the main retry loop. If you expect
    /// heavy failure rates and use a slow DLQ handler, increase this value.
    pub fn dlq_channel_size(mut self, size: usize) -> Self {
        self.dlq_channel_size = size;
        self
    }

    /// Set whether publish should wait for disk persistence
    pub fn wait_for_persistence(mut self, wait: bool) -> Self {
        self.wait_for_persistence = wait;
        self
    }
}

/// Preset configurations for common use cases
impl EventBusConfig {
    /// Create a configuration tailored for high-throughput scenarios.
    ///
    /// This preset prioritizes maximum event processing speed by:
    /// - Increasing the maximum queue size to 50,000.
    /// - Setting worker threads to double the available CPU cores.
    /// - Allowing events to be dropped if the queue is completely full (`drop_on_full = true`).
    /// - Limiting retries to 1 (fast failure).
    /// - Increasing handler channel buffer sizes.
    ///
    /// # Returns
    ///
    /// Returns a tuned `EventBusConfig`.
    pub fn high_throughput() -> Self {
        Self::default()
            .dispatcher_config(|d| {
                d.max_queue_size(50_000)
                    .worker_threads(num_cpus::get() * 2)
                    .drop_on_full(true)
                    .processing_timeout_ms(1000)
            })
            .max_retries(1)
            .handler_channel_size(1024)
    }

    /// Create a configuration tailored for maximum reliability.
    ///
    /// This preset guarantees no event loss under heavy load by:
    /// - Forbidding dropping events on full queues (`drop_on_full = false`).
    /// - Enabling disk persistence waits (`wait_for_persistence = true`).
    /// - Increasing maximum retries to 5 with a 500ms backoff.
    /// - Allocating a larger Dead Letter Queue (DLQ) channel size.
    ///
    /// # Returns
    ///
    /// Returns a tuned `EventBusConfig`.
    pub fn reliable() -> Self {
        Self::default()
            .dispatcher_config(|d| {
                d.max_queue_size(10_000)
                    .drop_on_full(false)
                    .processing_timeout_ms(30_000)
            })
            .max_retries(5)
            .retry_backoff(Duration::from_millis(500))
            .handler_channel_size(512)
            .dlq_channel_size(5000)
            .wait_for_persistence(true)
    }

    /// Create a configuration tailored for strict ordered processing.
    ///
    /// This preset guarantees that events are processed strictly in the order
    /// they are received by limiting the dispatcher to a single worker thread.
    ///
    /// # Returns
    ///
    /// Returns a tuned `EventBusConfig`.
    pub fn ordered() -> Self {
        Self::default().dispatcher_config(|d| d.worker_threads(1).drop_on_full(false))
    }

    /// Create a configuration tailored for unit and integration testing.
    ///
    /// This preset minimizes timeout durations and buffer sizes to ensure
    /// tests run quickly and fail fast when issues occur.
    ///
    /// # Returns
    ///
    /// Returns a tuned `EventBusConfig`.
    pub fn test() -> Self {
        Self::default()
            .dispatcher_config(|d| {
                d.max_queue_size(100)
                    .worker_threads(2)
                    .processing_timeout_ms(1000)
            })
            .shutdown_timeout(Duration::from_secs(5))
            .enable_tracing(false)
            .handler_channel_size(64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_high_throughput_config() {
        let config = EventBusConfig::high_throughput();
        assert_eq!(config.dispatcher.worker_threads, num_cpus::get() * 2);
        assert_eq!(config.dispatcher.max_queue_size, 50_000);
        assert_eq!(config.handler_channel_size, 1024);
        assert_eq!(config.max_retries, 1); // Fast failure
        assert!(!config.wait_for_persistence); // Maximum speed over reliability
    }

    #[test]
    fn test_reliable_config() {
        let config = EventBusConfig::reliable();
        assert!(config.wait_for_persistence); // Strict disk synching
        assert_eq!(config.max_retries, 5); // Must retry heavily
        assert!(!config.dispatcher.drop_on_full); // Cannot drop events
    }

    #[test]
    fn test_ordered_config() {
        let config = EventBusConfig::ordered();
        assert_eq!(config.dispatcher.worker_threads, 1); // Exact ordered single-threading
        assert!(!config.dispatcher.drop_on_full);
    }
}
