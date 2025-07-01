//! Event dispatcher for routing events to handlers.
//!
//! The dispatcher is responsible for receiving events from publishers
//! and routing them to the appropriate handlers via the subscription manager.

use crate::{EventEnvelope, Result};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info, trace};

pub mod channel;
pub mod worker;

pub use channel::ChannelDispatcher;
pub use worker::{WorkerPool, WorkerConfig};

/// Trait for event dispatchers.
///
/// Dispatchers are responsible for receiving events and routing them
/// to the appropriate handlers.
#[async_trait]
pub trait EventDispatcher: Send + Sync {
    /// Start the dispatcher
    async fn start(&mut self) -> Result<()>;
    
    /// Stop the dispatcher
    async fn stop(&mut self) -> Result<()>;
    
    /// Dispatch an event
    async fn dispatch(&self, envelope: EventEnvelope) -> Result<()>;
    
    /// Check if the dispatcher is running
    fn is_running(&self) -> bool;
    
    /// Get dispatcher statistics
    fn stats(&self) -> DispatcherStats;
}

/// Statistics for the event dispatcher
#[derive(Debug, Clone, Default)]
pub struct DispatcherStats {
    /// Total events dispatched
    pub events_dispatched: u64,
    
    /// Events currently in queue
    pub queue_size: usize,
    
    /// Number of dispatch errors
    pub dispatch_errors: u64,
    
    /// Average dispatch time in microseconds
    pub avg_dispatch_time_us: u64,
    
    /// Maximum queue size observed
    pub max_queue_size: usize,
}

impl DispatcherStats {
    /// Create new empty stats
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Update average dispatch time
    pub fn update_dispatch_time(&mut self, time_us: u64) {
        if self.events_dispatched == 0 {
            self.avg_dispatch_time_us = time_us;
        } else {
            // Calculate moving average
            self.avg_dispatch_time_us = 
                (self.avg_dispatch_time_us * self.events_dispatched + time_us) / 
                (self.events_dispatched + 1);
        }
    }
}

/// Configuration for dispatchers
#[derive(Debug, Clone)]
pub struct DispatcherConfig {
    /// Maximum number of events in the queue
    pub max_queue_size: usize,
    
    /// Number of worker threads (if applicable)
    pub worker_threads: usize,
    
    /// Whether to drop events when queue is full
    pub drop_on_full: bool,
    
    /// Event processing timeout in milliseconds
    pub processing_timeout_ms: u64,
    
    /// Enable detailed metrics collection
    pub enable_metrics: bool,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            max_queue_size: 10_000,
            worker_threads: num_cpus::get(),
            drop_on_full: false,
            processing_timeout_ms: 5_000,
            enable_metrics: true,
        }
    }
}

impl DispatcherConfig {
    /// Create a new dispatcher configuration
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Set the maximum queue size
    pub fn max_queue_size(mut self, size: usize) -> Self {
        self.max_queue_size = size;
        self
    }
    
    /// Set the number of worker threads
    pub fn worker_threads(mut self, threads: usize) -> Self {
        self.worker_threads = threads;
        self
    }
    
    /// Set whether to drop events when queue is full
    pub fn drop_on_full(mut self, drop: bool) -> Self {
        self.drop_on_full = drop;
        self
    }
    
    /// Set the processing timeout
    pub fn processing_timeout_ms(mut self, timeout: u64) -> Self {
        self.processing_timeout_ms = timeout;
        self
    }
    
    /// Enable or disable metrics
    pub fn enable_metrics(mut self, enable: bool) -> Self {
        self.enable_metrics = enable;
        self
    }
}

/// A no-op dispatcher for testing
pub struct NoOpDispatcher;

#[async_trait]
impl EventDispatcher for NoOpDispatcher {
    async fn start(&mut self) -> Result<()> {
        Ok(())
    }
    
    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }
    
    async fn dispatch(&self, _envelope: EventEnvelope) -> Result<()> {
        Ok(())
    }
    
    fn is_running(&self) -> bool {
        true
    }
    
    fn stats(&self) -> DispatcherStats {
        DispatcherStats::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_dispatcher_config() {
        let config = DispatcherConfig::new()
            .max_queue_size(5000)
            .worker_threads(4)
            .drop_on_full(true);
        
        assert_eq!(config.max_queue_size, 5000);
        assert_eq!(config.worker_threads, 4);
        assert!(config.drop_on_full);
    }
    
    #[test]
    fn test_dispatcher_stats() {
        let mut stats = DispatcherStats::new();
        
        // Test dispatch time averaging
        stats.update_dispatch_time(100);
        assert_eq!(stats.avg_dispatch_time_us, 100);
        
        stats.events_dispatched = 1;
        stats.update_dispatch_time(200);
        assert_eq!(stats.avg_dispatch_time_us, 150); // (100 + 200) / 2
    }
}