//! Hardware abstraction layer.
//!
//! Pin definitions, peripheral bring-up helpers, and traits that decouple
//! tasks from concrete `esp-hal` types so the `mock-hardware` feature can
//! provide host-side substitutes for unit tests.

/// BLE radio bring-up + Improv GATT-server seam (Phase 5). On-target only,
/// gated behind the `ble-improv` feature; the Improv protocol it drives
/// lives in `crate::proto::improv` and is host-tested without the radio.
#[cfg(feature = "ble-improv")]
pub mod ble;
pub mod i2s;
pub mod led;
pub mod mfrc522;
pub mod pins;
pub mod sdcard;
pub mod wifi;
