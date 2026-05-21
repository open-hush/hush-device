//! Append-only ring buffer of unflushed device events.
//!
//! Events are buffered here when the network is down or the sync task
//! hasn't run yet. Drained on every successful
//! `POST /v1/device/events`.
//!
//! TODO(phase-2): bounded capacity (drop-oldest on overflow), fsync after
//! every append, recovery on boot.
