//! Inter-task event enum.
//!
//! The single broadcast channel for events flowing between tasks. Tasks
//! subscribe via `PubSubChannel::subscriber()` and publish via
//! `PubSubChannel::publish()`. Keep the variants small — they are copied
//! into every subscriber's mailbox.
//!
//! TODO(phase-1): finalise variant list as tasks come online.

#[derive(Debug, Clone, Copy)]
pub enum Event {
    /// RFID card present, UID bytes captured.
    CardScanned { uid: [u8; 10], uid_len: u8 },
    /// Card removed (no card present for > 300 ms).
    CardLost,
    /// User rotated the encoder.
    VolumeDelta(i8),
    /// User pressed the encoder push button.
    PlayPausePressed,
    /// Pairing button pressed (short).
    PairingPressed,
    /// Reset button held long.
    ResetLongPress,
    /// Power state about to change.
    PowerTransition { from: PowerState, to: PowerState },
    /// Battery voltage drops below the low-water mark.
    LowBattery { millivolts: u16 },
    /// Sync cycle completed.
    SyncCompleted { ok: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    Active,
    LightSleep,
    DeepSleep,
}
