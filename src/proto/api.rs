//! Wire types matching the OpenAPI spec in
//! `hush-protocol/hush-api.yaml`.
//!
//! Drift between this file and the spec is a bug. A future CI job will
//! diff the generated types against the spec; until then, treat any
//! mismatch as a release blocker.
//!
//! TODO(phase-2): define `DeviceRegisterRequest`, `DeviceRegisterResponse`,
//! `DeviceSyncResponse`, `DeviceEventsRequest`, `Error`, etc., using
//! `serde` with `#[derive(Serialize, Deserialize)]` and small heapless
//! string types so `serde-json-core` can parse without `alloc`.
