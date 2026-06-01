//! Redb implementation of persistent event dispatcher and registry.

use crate::dispatcher::{DispatcherConfig, DispatcherStats, EventDispatcher};
use crate::event::EventEnvelope;
use crate::registry::{DashMapRegistry, EventRegistry, SubscriptionEntry};
use crate::subscription::SubscriptionManager;
use crate::{Error, Result};
use async_trait::async_trait;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use std::any::TypeId;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{error, info, trace};
use uuid::Uuid;

/// A serialized representation of an event and its metadata for persistent storage
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PersistedEnvelope {
    /// The stable string name of the event type
    pub type_name: String,
    /// The event metadata including IDs and tracing info
    pub metadata: crate::event::EventMetadata,
    /// The priority level of the event
    pub priority: crate::event::EventPriority,
    /// The serialized JSON payload of the event itself
    pub payload: Vec<u8>,
}

const EVENTS_TABLE: TableDefinition<'_, u128, &[u8]> = TableDefinition::new("events");
const REFCOUNT_TABLE: TableDefinition<'_, u128, u32> = TableDefinition::new("refcount");

/// A registry that wraps an in-memory registry and intercepts acks to update redb
#[derive(Debug)]
pub struct RedbRegistry {
    inner: Arc<DashMapRegistry>,
    ack_tx: tokio::sync::mpsc::UnboundedSender<Uuid>,
}

impl RedbRegistry {
    /// Create a new RedbRegistry
    pub fn new(db: Arc<Database>, inner: Arc<DashMapRegistry>) -> Self {
        let (ack_tx, mut ack_rx) = tokio::sync::mpsc::unbounded_channel::<Uuid>();
        let db_clone = db.clone();
        
        // Background worker to process DB acks without blocking the Tokio reactor
        tokio::spawn(async move {
            while let Some(event_id) = ack_rx.recv().await {
                let db_for_task = db_clone.clone();
                let res = tokio::task::spawn_blocking(move || {
                    let event_id_u128 = event_id.as_u128();
                    let write_txn = match db_for_task.begin_write() {
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
                }).await;
                
                if let Err(e) = res {
                    error!("Ack task panicked: {}", e);
                }
            }
        });

        Self { inner, ack_tx }
    }
}

impl EventRegistry for RedbRegistry {
    fn register(
        &self,
        event_type: TypeId,
        type_name: &str,
        subscription: SubscriptionEntry,
    ) -> Result<()> {
        self.inner.register(event_type, type_name, subscription)
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

    fn get_type_id(&self, type_name: &str) -> Option<TypeId> {
        self.inner.get_type_id(type_name)
    }

    fn ack_event(&self, _subscription_id: Uuid, event_id: Uuid) {
        let _ = self.ack_tx.send(event_id);
    }
}

/// Message sent to the background RedbDispatcher worker
#[derive(Debug)]
pub struct RedbDispatcherMessage {
    envelope: Arc<EventEnvelope>,
    ack_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

/// A dispatcher that writes events to redb before passing them to the subscription manager
#[allow(missing_debug_implementations)]
pub struct RedbDispatcher {
    db: Arc<Database>,
    config: DispatcherConfig,
    wait_for_persistence: bool,
    sender: Option<mpsc::Sender<RedbDispatcherMessage>>,
    receiver: Option<mpsc::Receiver<RedbDispatcherMessage>>,
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
        wait_for_persistence: bool,
        subscription_manager: Arc<SubscriptionManager>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(config.max_queue_size);

        Self {
            db,
            config,
            wait_for_persistence,
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

    /// Process events from the channel
    #[allow(clippy::too_many_arguments)]
    async fn process_events(
        db: Arc<Database>,
        mut receiver: mpsc::Receiver<RedbDispatcherMessage>,
        subscription_manager: Arc<SubscriptionManager>,
        is_running: Arc<AtomicBool>,
        events_dispatched: Arc<AtomicU64>,
        dispatch_errors: Arc<AtomicU64>,
        total_dispatch_time_us: Arc<AtomicU64>,
        config: DispatcherConfig,
    ) {
        info!("Redb dispatcher worker started");

        while let Some(msg) = receiver.recv().await {
            if !is_running.load(Ordering::SeqCst) {
                break;
            }

            let event = msg.envelope;
            let ack_tx = msg.ack_tx;

            let start = Instant::now();
            let event_id = event.event_id();
            let event_id_u128 = event_id.as_u128();

            // Check how many subscribers need this event
            let type_id = event.type_id();
            let sub_count = subscription_manager.registry().subscription_count(type_id) as u32;

            if sub_count > 0 {
                // Construct PersistedEnvelope
                let persisted_result = event.into_bytes().map(|payload| PersistedEnvelope {
                    type_name: event.event_type().to_string(),
                    metadata: event.metadata.clone(),
                    priority: event.priority,
                    payload,
                });

                // Serialize and write to redb
                match persisted_result.and_then(|pe| {
                    serde_json::to_vec(&pe)
                        .map_err(|e| crate::Error::SerializationError(e.to_string()))
                }) {
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
                        
                        // If wait_for_persistence is enabled, signal the publisher that the disk write is complete!
                        if let Some(tx) = ack_tx {
                            let _ = tx.send(());
                        }
                        
                        #[cfg(feature = "metrics")]
                        metrics::counter!("tokio_events_persistence_writes_total", "type" => event.event_type().to_string()).increment(1);
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
        if self.is_running.load(Ordering::SeqCst) {
            return Ok(());
        }

        self.is_running.store(true, Ordering::SeqCst);

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
        self.is_running.store(false, Ordering::SeqCst);
        self.sender.take(); // Close channel

        if let Some(handle) = self.worker_handle.take() {
            let _ = tokio::time::timeout(tokio::time::Duration::from_secs(5), handle).await;
        }

        Ok(())
    }

    async fn shutdown_gracefully(&mut self) -> Result<()> {
        info!("Shutting down Redb dispatcher gracefully");

        self.sender.take(); // Close channel so recv() returns None

        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.await.map_err(|e| Error::internal(format!("Worker panicked: {}", e)));
        }

        self.is_running.store(false, Ordering::SeqCst);
        info!("Redb dispatcher graceful shutdown complete");
        Ok(())
    }

    async fn dispatch(&self, envelope: EventEnvelope) -> Result<()> {
        if !self.is_running.load(Ordering::SeqCst) {
            return Err(Error::internal("Dispatcher is not running"));
        }

        let sender = self.sender.as_ref().ok_or_else(|| Error::internal("Dispatcher not initialized"))?;
        let current_size = sender.max_capacity().saturating_sub(sender.capacity());
        let max_size = self.max_queue_size.load(Ordering::Relaxed);
        if current_size as u64 > max_size {
            self.max_queue_size.store(current_size as u64, Ordering::Relaxed);
        }

        if self.wait_for_persistence {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let msg = RedbDispatcherMessage { envelope: Arc::new(envelope), ack_tx: Some(tx) };
            
            if self.config.drop_on_full {
                sender.try_send(msg).map_err(|_| Error::internal("Channel full"))?;
            } else {
                sender.send(msg).await.map_err(|_| Error::internal("Channel closed"))?;
            }
            
            // Wait for physical disk confirmation!
            let _ = rx.await;
            Ok(())
        } else {
            let msg = RedbDispatcherMessage { envelope: Arc::new(envelope), ack_tx: None };
            
            if self.config.drop_on_full {
                sender.try_send(msg).map_err(|_| Error::internal("Channel full"))
            } else {
                sender.send(msg).await.map_err(|_| Error::internal("Channel closed"))
            }
        }
    }

    async fn replay_pending(&self) -> Result<()> {
        let db = self.db.clone();
        let sender = self.sender.clone().ok_or_else(|| Error::internal("Dispatcher not initialized"))?;
        let registry = self.subscription_manager.registry();

        let replay_res =
            tokio::task::spawn_blocking(move || -> std::result::Result<u64, String> {
                let read_txn = db.begin_read().map_err(|e| e.to_string())?;
                let events = match read_txn.open_table(EVENTS_TABLE) {
                    Ok(t) => t,
                    Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
                    Err(e) => return Err(e.to_string()),
                };
                let refcounts = match read_txn.open_table(REFCOUNT_TABLE) {
                    Ok(t) => t,
                    Err(redb::TableError::TableDoesNotExist(_)) => return Ok(0),
                    Err(e) => return Err(e.to_string()),
                };

                let mut count = 0;
                let mut dead_events = Vec::new();
                for event_entry in events.iter().map_err(|e| e.to_string())? {
                    let (event_id_access, payload_access) =
                        event_entry.map_err(|e| e.to_string())?;
                    let event_id_u128 = event_id_access.value();

                    // Check if it has a positive refcount
                    if let Ok(Some(refcount)) = refcounts.get(event_id_u128) {
                        if refcount.value() > 0 {
                            // Deserialize PersistedEnvelope
                            let bytes = payload_access.value();
                            if let Ok(persisted) =
                                serde_json::from_slice::<PersistedEnvelope>(bytes)
                            {
                                // Find the TypeId and check if there are ANY active subscribers
                                let mut has_subscribers = false;
                                if let Some(type_id) = registry.get_type_id(&persisted.type_name) {
                                    if registry.subscription_count(type_id) > 0 {
                                        has_subscribers = true;
                                        
                                        // Reconstruct the EventEnvelope
                                        let envelope = EventEnvelope::from_serialized(
                                            type_id,
                                            persisted.type_name,
                                            persisted.metadata,
                                            persisted.priority,
                                            persisted.payload,
                                        );

                                        // Inject it directly into the memory channel
                                        let msg = RedbDispatcherMessage { envelope: Arc::new(envelope), ack_tx: None };
                                        if sender.blocking_send(msg).is_ok() {
                                            count += 1;
                                        }
                                    }
                                }
                                
                                // ZERO SUBSCRIBER FIX: If no handlers are listening, this event is dead
                                if !has_subscribers {
                                    dead_events.push(event_id_u128);
                                }
                            } else {
                                // Corrupted payload, consider it dead
                                dead_events.push(event_id_u128);
                            }
                        } else {
                            // Refcount is 0, consider it dead
                            dead_events.push(event_id_u128);
                        }
                    }
                }
                
                // Drop read transaction before opening write transaction
                drop(events);
                drop(refcounts);
                drop(read_txn);

                // Garbage collect all dead events!
                if !dead_events.is_empty() {
                    if let Ok(write_txn) = db.begin_write() {
                        if let Ok(mut events) = write_txn.open_table(EVENTS_TABLE) {
                            if let Ok(mut refcounts) = write_txn.open_table(REFCOUNT_TABLE) {
                                for id in dead_events {
                                    let _ = events.remove(id);
                                    let _ = refcounts.remove(id);
                                }
                            }
                        }
                        let _ = write_txn.commit();
                    }
                }

                Ok(count)
            })
            .await;

        match replay_res {
            Ok(Ok(count)) => {
                if count > 0 {
                    info!("Replayed {} pending events from redb", count);
                }
                Ok(())
            }
            Ok(Err(e)) => {
                error!("Failed to replay events: {}", e);
                Err(Error::internal(format!("Failed to replay events: {}", e)))
            }
            Err(_) => Err(Error::internal("Replay task panicked")),
        }
    }

    fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    fn stats(&self) -> DispatcherStats {
        let events = self.events_dispatched.load(Ordering::Relaxed);
        let time = self.total_dispatch_time_us.load(Ordering::Relaxed);
        let avg_time = time.checked_div(events).unwrap_or(0);
        let current_queue = self.sender.as_ref().map(|s| s.max_capacity() - s.capacity()).unwrap_or(0);

        DispatcherStats {
            events_dispatched: events,
            queue_size: current_queue,
            dispatch_errors: self.dispatch_errors.load(Ordering::Relaxed),
            avg_dispatch_time_us: avg_time,
            max_queue_size: self.max_queue_size.load(Ordering::Relaxed) as usize,
        }
    }
}
