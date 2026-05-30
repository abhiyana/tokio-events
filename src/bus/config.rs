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
        }
    }
}

impl EventBusConfig {
    /// Create a new configuration with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum retry attempts
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Set retry backoff duration
    pub fn retry_backoff(mut self, backoff: Duration) -> Self {
        self.retry_backoff = backoff;
        self
    }

    /// Set shutdown timeout
    pub fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    /// Enable tracing
    pub fn enable_tracing(mut self, enable: bool) -> Self {
        self.enable_tracing = enable;
        self
    }

    /// Configure dispatcher
    pub fn dispatcher_config<F>(mut self, f: F) -> Self
    where
        F: FnOnce(DispatcherConfig) -> DispatcherConfig,
    {
        self.dispatcher = f(self.dispatcher);
        self
    }

    /// Set per-handler channel buffer size
    pub fn handler_channel_size(mut self, size: usize) -> Self {
        self.handler_channel_size = size;
        self
    }

    /// Set DLQ channel buffer size
    pub fn dlq_channel_size(mut self, size: usize) -> Self {
        self.dlq_channel_size = size;
        self
    }
}

/// Preset configurations for common use cases
impl EventBusConfig {
    /// Configuration for high-throughput scenarios
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

    /// Configuration for reliable processing
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
    }

    /// Configuration for ordered processing (single worker)
    pub fn ordered() -> Self {
        Self::default()
            .dispatcher_config(|d| {
                d.worker_threads(1)
                    .drop_on_full(false)
            })
    }

    /// Configuration for testing
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

