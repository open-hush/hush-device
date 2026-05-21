//! Power task — implements the `ACTIVE → LIGHT_SLEEP → DEEP_SLEEP` state
//! machine and configures wake sources before entering each mode.
//!
//! Stack: ~3 KB target.
//!
//! TODO(phase-4): inactivity timers reset on any `Event` of interest;
//! transitions configured via `DeviceConfig.{lightSleepAfterSec,
//! deepSleepAfterSec}`. Wake masks come from [`crate::hw::pins`].
