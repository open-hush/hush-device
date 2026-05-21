//! Sync task — periodically calls `GET /v1/device/sync`, applies the
//! returned `DeviceConfig`, refreshes presigned URLs, and flushes the
//! event outbox via `POST /v1/device/events`.
//!
//! Stack: ~8 KB target (TLS workspace dominates).
//!
//! TODO(phase-2): full sync loop with backoff, jitter, and offline-tolerant
//! retry behaviour. Uses HMAC-signed requests via [`crate::api`].
