//! Persistent storage backed by internal flash NVS (via
//! `sequential-storage`).
//!
//! - [`nvs`] — typed KV store for secrets, WiFi creds, last config.
//! - [`outbox`] — append-only ring buffer of unflushed events.
//! - [`cache_index`] — index of cached audio files on the microSD.
//!
//! microSD is **not** authoritative for anything — it's an evictable
//! cache. NVS is the source of truth for everything that must survive a
//! card swap.

pub mod nvs;
pub mod outbox;
pub mod cache_index;
