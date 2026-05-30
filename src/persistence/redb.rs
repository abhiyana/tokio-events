//! Redb implementation of persistent event dispatcher and registry.

use crate::dispatcher::{DispatcherConfig, DispatcherStats, EventDispatcher};
use crate::event::EventEnvelope;
use crate::registry::{DashMapRegistry, EventRegistry, SubscriptionEntry};
use crate::subscription::SubscriptionManager;
use crate::{Error, Result};
use async_trait::async_trait;
use redb::{Database, ReadableTable, TableDefinition};
use std::any::TypeId;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{error, info, trace};
use uuid::Uuid;

const EVENTS_TABLE: TableDefinition<'_, u128, &[u8]> = TableDefinition::new("events");
const REFCOUNT_TABLE: TableDefinition<'_, u128, u32> = TableDefinition::new("refcount");

/// A registry that wraps an in-memory registry and intercepts acks to update redb
#[derive(Debug)]
pub struct RedbRegistry {
    db: Arc<Database>,
    inner: Arc<DashMapRegistry>,
}

impl RedbRegistry {
    /// Create a new RedbRegistry
    pub fn new(db: Arc<Database>, inner: Arc<DashMapRegistry>) -> Self {
        Self { db, inner }
    }
}

impl EventRegistry for RedbRegistry {
    fn register(&self, event_type: TypeId, subscription: SubscriptionEntry) -> Result<()> {
        self.inner.register(event_type, subscription)
    }

    fn unregister(&self, subscription_id: Uuid) -> Result<()> {
        self.inner.unregister(subscription_id)
    }

    fn get_subscriptions(&self, event_type: TypeId) -> Vec<SubscriptionEntry> {
        self.inner.get_subscriptions(event_type)
    }

    fn get_subscription(&self, subscription_id: Uuid) -> Option<SubscriptionEntry> {
        self.inner.get_subscription(subscription_id)
    }

    fn increment_processed(&self, subscription_id: Uuid) {
        self.inner.increment_processed(subscription_id)
    }

    fn deactivate(&self, subscription_id: Uuid) -> Result<()> {
        self.inner.deactivate(subscription_id)
    }

    fn total_subscriptions(&self) -> usize {
        self.inner.total_subscriptions()
    }

    fn subscription_count(&self, event_type: TypeId) -> usize {
        self.inner.subscription_count(event_type)
    }

    fn event_types(&self) -> Vec<TypeId> {
        self.inner.event_types()
    }

    fn clear(&self) {
        self.inner.clear()
    }

    fn ack_event(&self, _subscription_id: Uuid, event_id: Uuid) {
        let event_id_u128 = event_id.as_u128();
        let write_txn = match self.db.begin_write() {
            Ok(txn) => txn,
            Err(e) => {
                error!("Failed to begin write txn for ack: {}", e);
                return;
            }
        };

        {
            let mut refcounts = match write_txn.open_table(REFCOUNT_TABLE) {
                Ok(t) => t,
                Err(e) => {
                    error!("Failed to open refcount table: {}", e);
                    return;
                }
            };
            let mut events = match write_txn.open_table(EVENTS_TABLE) {
                Ok(t) => t,
                Err(e) => {
                    error!("Failed to open events table: {}", e);
                    return;
                }
            };

            let current = if let Ok(Some(count_access)) = refcounts.get(event_id_u128) {
                Some(count_access.value())
            } else {
                None
            };

            if let Some(current) = current {
                if current <= 1 {
                    // Last subscriber processed it, delete the event
                    let _ = refcounts.remove(event_id_u128);
                    let _ = events.remove(event_id_u128);
                    trace!(event_id = %event_id, "Event completely processed and removed from DB");
                } else {
                    // Decrement
                    let _ = refcounts.insert(event_id_u128, current - 1);
                    trace!(event_id = %event_id, remaining = current - 1, "Event acked");
                }
            }
        }

        let _ = write_txn.commit();
    }
}

/// A dispatcher that writes events to redb before passing them to the subscription manager
#[allow(missing_debug_implementations)]
pub struct RedbDispatcher {
    db: Arc<Database>,
    config: DispatcherConfig,
    sender: mpsc::Sender<Arc<EventEnvelope>>,
    receiver: Option<mpsc::Receiver<Arc<EventEnvelope>>>,
    subscription_manager: Arc<SubscriptionManager>,
    worker_handle: Option<JoinHandle<()>>,
    is_running: Arc<AtomicBool>,
    events_dispatched: Arc<AtomicU64>,
    dispatch_errors: Arc<AtomicU64>,
    total_dispatch_time_us: Arc<AtomicU64>,
    max_queue_size: Arc<AtomicU64>,
}

impl RedbDispatcher {
    /// Create a new RedbDispatcher
    pub fn new(
        db: Arc<Database>,
        config: DispatcherConfig,
        subscription_manager: Arc<SubscriptionManager>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(config.max_queue_size);

        Self {
            db,
            config,
            sender,
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

    /// Process events from the channel
    #[allow(clippy::too_many_arguments)]
    async fn process_events(
        db: Arc<Database>,
        mut receiver: mpsc::Receiver<Arc<EventEnvelope>>,
        subscription_manager: Arc<SubscriptionManager>,
        is_running: Arc<AtomicBool>,
        events_dispatched: Arc<AtomicU64>,
        dispatch_errors: Arc<AtomicU64>,
        total_dispatch_time_us: Arc<AtomicU64>,
        config: DispatcherConfig,
    ) {
        info!("Redb dispatcher worker started");

        while is_running.load(Ordering::Relaxed) {
            let event = tokio::select! {
                Some(event) = receiver.recv() => event,
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                    continue;
                }
            };

            let start = Instant::now();
            let event_id = event.event_id();
            let event_id_u128 = event_id.as_u128();

            // Check how many subscribers need this event
            let type_id = event.type_id();
            let sub_count = subscription_manager.registry().subscription_count(type_id) as u32;

            if sub_count > 0 {
                // Serialize and write to redb
                match event.into_bytes() {
                    Ok(bytes) => {
                        let write_txn_res = tokio::task::spawn_blocking({
                            let db = db.clone();
                            move || -> std::result::Result<(), String> {
                                let write_txn = db.begin_write().map_err(|e| e.to_string())?;
                                {
                                    let mut events = write_txn
                                        .open_table(EVENTS_TABLE)
                                        .map_err(|e| e.to_string())?;
                                    let mut refcounts = write_txn
                                        .open_table(REFCOUNT_TABLE)
                                        .map_err(|e| e.to_string())?;
                                    events
                                        .insert(event_id_u128, bytes.as_slice())
                                        .map_err(|e| e.to_string())?;
                                    refcounts
                                        .insert(event_id_u128, sub_count)
                                        .map_err(|e| e.to_string())?;
                                }
                                write_txn.commit().map_err(|e| e.to_string())
                            }
                        })
                        .await;

                        if let Err(e) = write_txn_res {
                            error!("Failed to persist event to redb: {}", e);
                            dispatch_errors.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    }
                    Err(e) => {
                        error!("Failed to serialize event for persistence: {}", e);
                        dispatch_errors.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                }
            }

            // Dispatch to memory channels
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
                }
                Err(e) => {
                    dispatch_errors.fetch_add(1, Ordering::Relaxed);
                    error!(event_id = %event_id, error = %e, "Failed to dispatch event");
                }
            }
        }

        info!("Redb dispatcher worker stopped");
    }
}

#[async_trait]
impl EventDispatcher for RedbDispatcher {
    async fn start(&mut self) -> Result<()> {
        if self.is_running.load(Ordering::Relaxed) {
            return Ok(());
        }

        self.is_running.store(true, Ordering::Relaxed);

        if let Some(receiver) = self.receiver.take() {
            let db = self.db.clone();
            let subscription_manager = self.subscription_manager.clone();
            let is_running = self.is_running.clone();
            let events_dispatched = self.events_dispatched.clone();
            let dispatch_errors = self.dispatch_errors.clone();
            let total_dispatch_time_us = self.total_dispatch_time_us.clone();
            let config = self.config.clone();

            self.worker_handle = Some(tokio::spawn(async move {
                Self::process_events(
                    db,
                    receiver,
                    subscription_manager,
                    is_running,
                    events_dispatched,
                    dispatch_errors,
                    total_dispatch_time_us,
                    config,
                )
                .await;
            }));
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        self.is_running.store(false, Ordering::Relaxed);

        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.await;
        }

        Ok(())
    }

    async fn dispatch(&self, envelope: EventEnvelope) -> Result<()> {
        if !self.is_running.load(Ordering::Relaxed) {
            return Err(Error::internal("Dispatcher is not running"));
        }

        self.sender
            .send(Arc::new(envelope))
            .await
            .map_err(|_| Error::internal("Dispatcher channel closed"))
    }

    async fn replay_pending(&self) -> Result<()> {
        // Find orphaned events in redb and push them back through the sender
        // To do this, we need to read EVENTS_TABLE and dispatch them.
        Ok(()) // Placeholder until we serialize full envelope
    }

    fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }

    fn stats(&self) -> DispatcherStats {
        let events = self.events_dispatched.load(Ordering::Relaxed);
        let time = self.total_dispatch_time_us.load(Ordering::Relaxed);
        let avg_time = time.checked_div(events).unwrap_or(0);

        DispatcherStats {
            events_dispatched: events,
            queue_size: (self.config.max_queue_size - self.sender.capacity()),
            dispatch_errors: self.dispatch_errors.load(Ordering::Relaxed),
            avg_dispatch_time_us: avg_time,
            max_queue_size: self.max_queue_size.load(Ordering::Relaxed) as usize,
        }
    }
}
