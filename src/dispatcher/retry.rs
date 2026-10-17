//! Retry policies for event handling failures.

use crate::{Error, EventEnvelope, Result};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// Trait for retry policies that determine how to handle handler failures.
///
/// This allows pluggable retry strategies (exponential backoff, fixed delay,
/// circuit breaker, etc.) to be configured at the dispatcher level rather than
/// per-subscription.
#[async_trait]
pub trait RetryPolicy: Send + Sync + 'static {
    /// Determine if a failed event should be retried and after what delay.
    ///
    /// # Arguments
    /// * `attempt` - The current attempt number (1-indexed, first retry is attempt 1)
    /// * `error` - The error that caused the failure
    /// * `envelope` - The event envelope that failed
    ///
    /// # Returns
    /// * `Some(duration)` - Retry after this duration
    /// * `None` - Do not retry, send to DLQ
    async fn should_retry(
        &self,
        attempt: u32,
        error: &Error,
        envelope: &EventEnvelope,
    ) -> Option<Duration>;

    /// Called when an event is permanently failed and sent to DLQ.
    ///
    /// Default implementation does nothing.
    async fn on_permanent_failure(
        &self,
        attempt: u32,
        error: &Error,
        envelope: &EventEnvelope,
    ) {
        let _ = (attempt, error, envelope);
    }

    /// Get the maximum number of retry attempts (for monitoring/config).
    fn max_attempts(&self) -> u32 {
        u32::MAX
    }

    /// Get the name of this retry policy for debugging.
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// Exponential backoff retry policy.
///
/// Starts with `base_delay` and doubles on each retry attempt, up to `max_delay`.
/// Optionally applies jitter to prevent thundering herd.
#[derive(Debug, Clone)]
pub struct ExponentialBackoff {
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub max_attempts: u32,
    pub jitter: bool,
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self {
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
            max_attempts: 5,
            jitter: true,
        }
    }
}

impl ExponentialBackoff {
    pub fn new(base_delay: Duration, max_delay: Duration, max_attempts: u32) -> Self {
        Self {
            base_delay,
            max_delay,
            max_attempts,
            jitter: true,
        }
    }

    pub fn without_jitter(mut self) -> Self {
        self.jitter = false;
        self
    }
}

#[async_trait]
impl RetryPolicy for ExponentialBackoff {
    async fn should_retry(
        &self,
        attempt: u32,
        error: &Error,
        envelope: &EventEnvelope,
    ) -> Option<Duration> {
        let _ = (error, envelope);

        if attempt >= self.max_attempts {
            return None;
        }

        let delay = self.base_delay * (2u32.pow(attempt - 1));
        let delay = delay.min(self.max_delay);

        let final_delay = if self.jitter {
            let jitter_factor = 0.5 + rand::random::<f64>() * 0.5;
            Duration::from_secs_f64(delay.as_secs_f64() * jitter_factor)
        } else {
            delay
        };

        Some(final_delay)
    }

    fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    fn name(&self) -> &'static str {
        "ExponentialBackoff"
    }
}

/// Fixed delay retry policy.
///
/// Waits the same `delay` between each retry attempt, up to `max_attempts`.
#[derive(Debug, Clone)]
pub struct FixedDelay {
    pub delay: Duration,
    pub max_attempts: u32,
}

impl Default for FixedDelay {
    fn default() -> Self {
        Self {
            delay: Duration::from_millis(100),
            max_attempts: 3,
        }
    }
}

#[async_trait]
impl RetryPolicy for FixedDelay {
    async fn should_retry(
        &self,
        attempt: u32,
        error: &Error,
        envelope: &EventEnvelope,
    ) -> Option<Duration> {
        let _ = (error, envelope);

        if attempt >= self.max_attempts {
            return None;
        }

        Some(self.delay)
    }

    fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    fn name(&self) -> &'static str {
        "FixedDelay"
    }
}

/// No retry policy - immediately send to DLQ on first failure.
#[derive(Debug, Clone, Default)]
pub struct NoRetry;

#[async_trait]
impl RetryPolicy for NoRetry {
    async fn should_retry(
        &self,
        _attempt: u32,
        _error: &Error,
        _envelope: &EventEnvelope,
    ) -> Option<Duration> {
        None
    }

    fn max_attempts(&self) -> u32 {
        0
    }

    fn name(&self) -> &'static str {
        "NoRetry"
    }
}

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation - requests pass through
    Closed,
    /// Failure threshold exceeded - requests fail fast
    Open,
    /// Testing if service recovered - limited requests pass
    HalfOpen,
}

/// Circuit breaker retry policy.
///
/// Wraps another retry policy and adds circuit breaker behavior.
/// When failure rate exceeds threshold, circuit opens and fails fast.
#[derive(Debug)]
pub struct CircuitBreaker<P: RetryPolicy> {
    inner: P,
    failure_threshold: u32,
    success_threshold: u32,
    timeout: Duration,
    state: Arc<tokio::sync::RwLock<CircuitState>>,
    failure_count: Arc<tokio::sync::RwLock<u32>>,
    success_count: Arc<tokio::sync::RwLock<u32>>,
    last_failure: Arc<tokio::sync::RwLock<Option<std::time::Instant>>>,
}

impl<P: RetryPolicy> CircuitBreaker<P> {
    pub fn new(inner: P, failure_threshold: u32, success_threshold: u32, timeout: Duration) -> Self {
        Self {
            inner,
            failure_threshold,
            success_threshold,
            timeout,
            state: Arc::new(tokio::sync::RwLock::new(CircuitState::Closed)),
            failure_count: Arc::new(tokio::sync::RwLock::new(0)),
            success_count: Arc::new(tokio::sync::RwLock::new(0)),
            last_failure: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    async fn check_state(&self) -> CircuitState {
        let mut state = self.state.write().await;
        let last_failure = self.last_failure.write().await;

        match *state {
            CircuitState::Open => {
                if let Some(last) = *last_failure {
                    if last.elapsed() >= self.timeout {
                        *state = CircuitState::HalfOpen;
                        *self.success_count.write().await = 0;
                        return CircuitState::HalfOpen;
                    }
                }
                CircuitState::Open
            }
            CircuitState::HalfOpen => CircuitState::HalfOpen,
            CircuitState::Closed => CircuitState::Closed,
        }
    }

    async fn record_success(&self) {
        let state = self.check_state().await;
        if state == CircuitState::HalfOpen {
            let mut success_count = self.success_count.write().await;
            *success_count += 1;
            if *success_count >= self.success_threshold {
                *self.state.write().await = CircuitState::Closed;
                *self.failure_count.write().await = 0;
            }
        } else if state == CircuitState::Closed {
            *self.failure_count.write().await = 0;
        }
    }

    async fn record_failure(&self) {
        let mut failure_count = self.failure_count.write().await;
        *failure_count += 1;

        let mut last_failure = self.last_failure.write().await;
        *last_failure = Some(std::time::Instant::now());

        if *failure_count >= self.failure_threshold {
            *self.state.write().await = CircuitState::Open;
        }
    }
}

#[async_trait]
impl<P: RetryPolicy> RetryPolicy for CircuitBreaker<P> {
    async fn should_retry(
        &self,
        attempt: u32,
        error: &Error,
        envelope: &EventEnvelope,
    ) -> Option<Duration> {
        let state = self.check_state().await;

        if state == CircuitState::Open {
            return None;
        }

        let result = self.inner.should_retry(attempt, error, envelope).await;

        if result.is_some() {
            self.record_failure().await;
        } else {
            self.record_success().await;
        }

        result
    }

    async fn on_permanent_failure(
        &self,
        attempt: u32,
        error: &Error,
        envelope: &EventEnvelope,
    ) {
        self.inner.on_permanent_failure(attempt, error, envelope).await;
        self.record_failure().await;
    }

    fn max_attempts(&self) -> u32 {
        self.inner.max_attempts()
    }

    fn name(&self) -> &'static str {
        "CircuitBreaker"
    }
}

/// Trait for dead letter queue handling.
#[async_trait]
pub trait DeadLetterQueue: Send + Sync + 'static {
    /// Handle a permanently failed event.
    async fn handle(&self, envelope: Arc<EventEnvelope>, error: &Error) -> Result<()>;

    /// Get the name of this DLQ for debugging.
    fn name(&self) -> &'static str;
}

/// In-memory dead letter queue (for testing or low-volume use).
#[derive(Debug)]
pub struct InMemoryDlq {
    sender: tokio::sync::mpsc::UnboundedSender<(Arc<EventEnvelope>, Error)>,
    receiver: tokio::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<(Arc<EventEnvelope>, Error)>>>,
}

impl InMemoryDlq {
    pub fn new() -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            sender,
            receiver: tokio::sync::Mutex::new(Some(receiver)),
        }
    }

    /// Get a receiver for consuming DLQ events (can only be called once).
    pub async fn take_receiver(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<(Arc<EventEnvelope>, Error)>> {
        self.receiver.lock().await.take()
    }

    pub fn sender(&self) -> tokio::sync::mpsc::UnboundedSender<(Arc<EventEnvelope>, Error)> {
        self.sender.clone()
    }
}

impl Default for InMemoryDlq {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DeadLetterQueue for InMemoryDlq {
    async fn handle(&self, envelope: Arc<EventEnvelope>, error: &Error) -> Result<()> {
        self.sender
            .send((envelope, error.clone()))
            .map_err(|_| Error::internal("DLQ channel closed"))?;
        Ok(())
    }

    fn name(&self) -> &'static str {
        "InMemoryDlq"
    }
}

/// Configuration for retry/DLQ behavior at the dispatcher level.
#[derive(Clone)]
pub struct RetryConfig {
    /// The retry policy to use
    pub policy: Arc<dyn RetryPolicy>,

    /// The dead letter queue to use for permanently failed events
    pub dlq: Option<Arc<dyn DeadLetterQueue>>,

    /// Whether to enable metrics for retry/DLQ
    pub enable_metrics: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            policy: Arc::new(ExponentialBackoff::default()),
            dlq: Some(Arc::new(InMemoryDlq::new())),
            enable_metrics: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Event, EventEnvelope};

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct TestEvent {
        value: i32,
    }

    impl Event for TestEvent {
        fn event_type() -> &'static str { "TestEvent" }
        fn serialize_event(&self) -> crate::Result<Vec<u8>> {
            serde_json::to_vec(self).map_err(|e| crate::Error::SerializationError(e.to_string()))
        }
        fn deserialize_event(bytes: &[u8]) -> crate::Result<Self> {
            serde_json::from_slice(bytes).map_err(|e| crate::Error::SerializationError(e.to_string()))
        }
    }

    #[tokio::test]
    async fn test_exponential_backoff() {
        let policy = ExponentialBackoff::default();
        let envelope = EventEnvelope::new(TestEvent { value: 1 });

        let delay = policy.should_retry(1, &Error::handler("test"), &envelope).await;
        assert!(delay.is_some());
        assert!(delay.unwrap() >= Duration::from_millis(50));
        assert!(delay.unwrap() <= Duration::from_millis(100));

        let delay = policy.should_retry(5, &Error::handler("test"), &envelope).await;
        assert!(delay.is_none());
    }

    #[tokio::test]
    async fn test_fixed_delay() {
        let policy = FixedDelay { delay: Duration::from_millis(500), max_attempts: 3 };
        let envelope = EventEnvelope::new(TestEvent { value: 1 });

        let delay = policy.should_retry(1, &Error::handler("test"), &envelope).await;
        assert_eq!(delay, Some(Duration::from_millis(500)));

        let delay = policy.should_retry(3, &Error::handler("test"), &envelope).await;
        assert!(delay.is_none());
    }

    #[tokio::test]
    async fn test_no_retry() {
        let policy = NoRetry;
        let envelope = EventEnvelope::new(TestEvent { value: 1 });

        let delay = policy.should_retry(1, &Error::handler("test"), &envelope).await;
        assert!(delay.is_none());
    }

    #[tokio::test]
    async fn test_circuit_breaker_opens() {
        let inner = FixedDelay { delay: Duration::from_millis(10), max_attempts: 10 };
        let cb = CircuitBreaker::new(inner, 2, 2, Duration::from_secs(60));
        let envelope = EventEnvelope::new(TestEvent { value: 1 });

        let _ = cb.should_retry(1, &Error::handler("test"), &envelope).await;
        let _ = cb.should_retry(2, &Error::handler("test"), &envelope).await;

        let delay = cb.should_retry(3, &Error::handler("test"), &envelope).await;
        assert!(delay.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_dlq() {
        let dlq = InMemoryDlq::new();
        let envelope = Arc::new(EventEnvelope::new(TestEvent { value: 42 }));
        let error = Error::handler("test error");

        dlq.handle(envelope.clone(), &error).await.unwrap();

        let mut rx = dlq.take_receiver().await.unwrap();
        let (received_envelope, received_error) = rx.recv().await.unwrap();
        assert_eq!(received_envelope.get_event::<TestEvent>().unwrap().value, 42);
        assert_eq!(received_error.to_string(), "Handler error: test error");
    }
}