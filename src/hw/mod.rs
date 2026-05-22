//! Hardware abstraction layer.
//!
//! Pin definitions, peripheral bring-up helpers, and traits that decouple
//! tasks from concrete `esp-hal` types so the `mock-hardware` feature can
//! provide host-side substitutes for unit tests.

pub mod pins;
pub mod i2s;
pub mod mfrc522;
pub mod sdcard;
pub mod led;
pub mod wifi;
