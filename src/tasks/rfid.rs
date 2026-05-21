//! RFID task — waits for the MFRC522 IRQ, reads the UID, publishes a
//! `CardScanned` event.
//!
//! Stack: ~4 KB target (small — no allocation, fixed-size buffers).
//!
//! TODO(phase-1): `#[embassy_executor::task] async fn run(...)` that loops
//! awaiting IRQ → reads UID → publishes via the `Event` pubsub channel.
