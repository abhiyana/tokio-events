//! Configuration for the event bus.

use crate::dispatcher::DispatcherConfig;
use std::time::Duration;

/// Configuration for the event bus
#[derive(Debug, Clone)]
pub struct EventBusConfig {
    /// Dispatcher configuration
    pub dispatcher: DispatcherConfig,
    
    /// Enable event deduplication
    pub enable_deduplication: bool,
    
    /// Deduplication window (how long to remember event IDs)
    pub deduplication_window: Duration,
    
    /// Maximum number of retry attempts for failed handlers
    pub max_retries: u32,
    
    /// Retry backoff base duration
    pub retry_backoff: Duration,
    
    /// Enable event ordering by correlation ID
    pub enable_ordering: bool,
    
    /// Shutdown timeout
    pub shutdown_timeout: Duration,
    
    /// Enable tracing
    pub enable_tracing: bool,
}

impl Default for EventBusConfig {
    fn default() -> Self {
        Self {
            dispatcher: DispatcherConfig::default(),
            enable_deduplication: false,
            deduplication_window: Duration::from_secs(300), // 5 minutes
            max_retries: 3,
            retry_backoff: Duration::from_millis(100),
            enable_ordering: false,
            shutdown_timeout: Duration::from_secs(30),
            enable_tracing: true,
        }
    }
}

impl EventBusConfig {
    /// Create a new configuration with defaults
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Enable event deduplication
    pub fn enable_deduplication(mut self, enable: bool) -> Self {
        self.enable_deduplication = enable;
        self
    }
    
    /// Set deduplication window
    pub fn deduplication_window(mut self, window: Duration) -> Self {
        self.deduplication_window = window;
        self
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
    
    /// Enable event ordering
    pub fn enable_ordering(mut self, enable: bool) -> Self {
        self.enable_ordering = enable;
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
            .enable_deduplication(false)
            .max_retries(1)
    }
    
    /// Configuration for reliable processing
    pub fn reliable() -> Self {
        Self::default()
            .dispatcher_config(|d| {
                d.max_queue_size(10_000)
                    .drop_on_full(false)
                    .processing_timeout_ms(30_000)
            })
            .enable_deduplication(true)
            .max_retries(5)
            .retry_backoff(Duration::from_millis(500))
    }
    
    /// Configuration for ordered processing
    pub fn ordered() -> Self {
        Self::default()
            .dispatcher_config(|d| {
                d.worker_threads(1) // Single worker for ordering
                    .drop_on_full(false)
            })
            .enable_ordering(true)
            .enable_deduplication(true)
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
    }
}