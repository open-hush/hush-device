//! HTTPS client over `reqwless` + `embedded-tls`.
//!
//! Every device-side request is HMAC-signed via [`crate::api::hmac`].
//! TLS workspace lives in PSRAM (see comments in `src/main.rs`).
//!
//! TODO(phase-2): provide async functions for each device-facing endpoint:
//! `register`, `sync`, `post_events`, `download_audio`. All accept the
//! deserialised wire types from [`crate::proto::api`].
