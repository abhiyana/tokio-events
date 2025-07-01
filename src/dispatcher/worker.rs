//! Worker pool for processing events in parallel.

use crate::{EventEnvelope, Result};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info};

/// Configuration for the worker pool
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Number of worker threads
    pub num_workers: usize,
    
    /// Size of the task queue per worker
    pub queue_size: usize,
    
    /// Worker thread name prefix
    pub name_prefix: String,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            num_workers: num_cpus::get(),
            queue_size: 1000,
            name_prefix: "event-worker".to_string(),
        }
    }
}

/// A pool of workers for processing events
pub struct WorkerPool {
    /// Worker configuration
    config: WorkerConfig,
    
    /// Sender channels for each worker
    senders: Vec<mpsc::Sender<Arc<EventEnvelope>>>,
    
    /// Worker task handles
    handles: Vec<JoinHandle<()>>,
    
    /// Next worker to send to (round-robin)
    next_worker: std::sync::atomic::AtomicUsize,
}

impl WorkerPool {
    /// Create a new worker pool
    pub fn new(config: WorkerConfig) -> Self {
        Self {
            config,
            senders: Vec::new(),
            handles: Vec::new(),
            next_worker: std::sync::atomic::AtomicUsize::new(0),
        }
    }
    
    /// Start the worker pool
    pub fn start<F>(&mut self, processor: F) -> Result<()>
    where
        F: Fn(Arc<EventEnvelope>) -> futures::future::BoxFuture<'static, Result<()>> + Send + Sync + 'static + Clone,
    {
        info!(
            "Starting worker pool with {} workers",
            self.config.num_workers
        );
        
        for i in 0..self.config.num_workers {
            let (tx, mut rx) = mpsc::channel::<Arc<EventEnvelope>>(self.config.queue_size);
            self.senders.push(tx);
            
            let processor = processor.clone();
            let worker_name = format!("{}-{}", self.config.name_prefix, i);
            
            let handle = tokio::spawn(async move {
                debug!("Worker {} started", worker_name);
                
                while let Some(envelope) = rx.recv().await {
                    if let Err(e) = processor(envelope).await {
                        tracing::error!("Worker {} error: {}", worker_name, e);
                    }
                }
                
                debug!("Worker {} stopped", worker_name);
            });
            
            self.handles.push(handle);
        }
        
        Ok(())
    }
    
    /// Send an event to a worker (round-robin)
    pub async fn send(&self, envelope: Arc<EventEnvelope>) -> Result<()> {
        if self.senders.is_empty() {
            return Err(crate::Error::internal("Worker pool not started"));
        }
        
        // Get next worker index (round-robin)
        let index = self.next_worker.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % self.senders.len();
        
        self.senders[index]
            .send(envelope)
            .await
            .map_err(|_| crate::Error::internal("Worker channel closed"))
    }
    
    /// Send an event to a specific worker
    pub async fn send_to(&self, worker_index: usize, envelope: Arc<EventEnvelope>) -> Result<()> {
        if worker_index >= self.senders.len() {
            return Err(crate::Error::internal("Invalid worker index"));
        }
        
        self.senders[worker_index]
            .send(envelope)
            .await
            .map_err(|_| crate::Error::internal("Worker channel closed"))
    }
    
    /// Get the number of workers
    pub fn num_workers(&self) -> usize {
        self.senders.len()
    }
    
    /// Stop all workers
    pub async fn stop(mut self) -> Result<()> {
        info!("Stopping worker pool");
        
        // Close all channels
        self.senders.clear();
        
        // Wait for workers to finish
        for handle in self.handles {
            handle.await.map_err(|e| crate::Error::internal(format!("Worker join error: {}", e)))?;
        }
        
        info!("Worker pool stopped");
        Ok(())
    }
}

/// A worker pool that uses consistent hashing for event distribution
pub struct HashingWorkerPool {
    pool: WorkerPool,
}

impl HashingWorkerPool {
    /// Create a new hashing worker pool
    pub fn new(config: WorkerConfig) -> Self {
        Self {
            pool: WorkerPool::new(config),
        }
    }
    
    /// Start the worker pool
    pub fn start<F>(&mut self, processor: F) -> Result<()>
    where
        F: Fn(Arc<EventEnvelope>) -> futures::future::BoxFuture<'static, Result<()>> + Send + Sync + 'static + Clone,
    {
        self.pool.start(processor)
    }
    
    /// Send an event using consistent hashing based on correlation ID
    pub async fn send(&self, envelope: Arc<EventEnvelope>) -> Result<()> {
        let worker_index = if let Some(correlation_id) = envelope.correlation_id() {
            // Use correlation ID for consistent routing
            let hash = fxhash::hash64(&correlation_id);
            (hash as usize) % self.pool.num_workers()
        } else {
            // Fall back to event ID
            let hash = fxhash::hash64(&envelope.event_id());
            (hash as usize) % self.pool.num_workers()
        };
        
        self.pool.send_to(worker_index, envelope).await
    }
    
    /// Stop the worker pool
    pub async fn stop(self) -> Result<()> {
        self.pool.stop().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Event};
    use std::sync::atomic::{AtomicUsize, Ordering};
    
    #[derive(Debug, Clone)]
    struct TestEvent;
    
    impl Event for TestEvent {
        fn event_type() -> &'static str {
            "TestEvent"
        }
    }
    
    #[tokio::test]
    async fn test_worker_pool() {
        let config = WorkerConfig {
            num_workers: 2,
            queue_size: 10,
            name_prefix: "test-worker".to_string(),
        };
        
        let mut pool = WorkerPool::new(config);
        let counter = Arc::new(AtomicUsize::new(0));
        
        // Start workers
        let counter_clone = counter.clone();
        pool.start(move |_envelope| {
            let counter = counter_clone.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
        }).unwrap();
        
        // Send events
        for _ in 0..10 {
            let envelope = Arc::new(EventEnvelope::new(TestEvent));
            pool.send(envelope).await.unwrap();
        }
        
        // Wait for processing
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        // Check all events were processed
        assert_eq!(counter.load(Ordering::Relaxed), 10);
        
        // Stop pool
        pool.stop().await.unwrap();
    }
}