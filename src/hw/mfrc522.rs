//! MFRC522 (RFID reader) bring-up over SPI2.
//!
//! Uses the `mfrc522` crate. Card-present interrupts are wired to
//! [`crate::hw::pins::RFID_IRQ`]; the `rfid` task awaits that signal and
//! reads the UID.
//!
//! TODO(phase-1): `pub fn init(...)` returning a `Mfrc522<...>` handle plus
//! the IRQ future for the task to await.
