//! Persistence layer for the event bus.
//!
//! This module provides durable storage for events to ensure they survive application crashes.

pub mod redb;
pub use self::redb::{RedbDispatcher, RedbRegistry};
