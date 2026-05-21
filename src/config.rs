//! Compile-time and runtime configuration.
//!
//! Compile-time values live as `const`s here. Runtime values come from
//! [`crate::storage::nvs`] (last-known device config from the backend) or
//! are baked at provisioning time (per-device secret).
//!
//! TODO(phase-2): load `RuntimeConfig` from NVS at boot; expose a mutable
//! handle that the `sync` task updates when the backend sends a new
//! `DeviceConfig` payload.

/// API base URL. Overridable at build time with `HUSH_API_URL`.
pub const API_BASE_URL: &str = match option_env!("HUSH_API_URL") {
    Some(u) => u,
    None => "https://api.open-hush.com",
};

/// Sync poll interval, in seconds. Lower-bounded by 60.
pub const DEFAULT_SYNC_INTERVAL_SEC: u32 = 600;

/// Idle threshold before transitioning to LIGHT_SLEEP.
pub const DEFAULT_LIGHT_SLEEP_AFTER_SEC: u32 = 30;

/// Idle threshold before transitioning to DEEP_SLEEP.
pub const DEFAULT_DEEP_SLEEP_AFTER_SEC: u32 = 300;

/// Maximum number of events buffered in RAM before forcing a flush.
pub const EVENT_BUFFER_HIGH_WATER: usize = 8;

/// PSRAM region size we expect on the XIAO ESP32-S3.
pub const PSRAM_SIZE_BYTES: usize = 8 * 1024 * 1024;
