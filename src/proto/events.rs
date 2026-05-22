//! Inter-task event enum and the single broadcast channel that carries
//! it.
//!
//! Producers — typically a per-concern task — call
//! [`EVENT_BUS::publisher`] once at boot and `publisher.publish(event).await`
//! per event. Consumers call [`EVENT_BUS::subscriber`] and
//! `subscriber.next_message().await` in their loop. The mutex is
//! [`CriticalSectionRawMutex`] because future producers may be ISR
//! handlers (brown-out detector, RFID IRQ once we wire it).
//!
//! ## Sizing
//!
//! - **Capacity 8**: a card scan can fan out three or four events
//!   (CardScanned + LED state + audio decoder start); a power
//!   transition adds two more. Eight slots absorbs the worst case
//!   without back-pressuring a publisher.
//! - **4 subscribers**: led, audio, sync, power. Encoder/input and
//!   rfid are publishers only.
//! - **4 publishers**: rfid, input, power, sync.

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, pubsub::PubSubChannel};

/// Compile-time capacities of [`EVENT_BUS`]. Bumped only when a new
/// task subscribes / publishes.
pub const EVENT_BUS_CAPACITY: usize = 8;
pub const EVENT_BUS_SUBSCRIBERS: usize = 4;
pub const EVENT_BUS_PUBLISHERS: usize = 4;

/// The single broadcast channel for cross-task events.
pub static EVENT_BUS: PubSubChannel<
    CriticalSectionRawMutex,
    Event,
    EVENT_BUS_CAPACITY,
    EVENT_BUS_SUBSCRIBERS,
    EVENT_BUS_PUBLISHERS,
> = PubSubChannel::new();

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
