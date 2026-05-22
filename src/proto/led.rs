//! LED command protocol — the message the rest of the firmware sends to
//! the `led` task to change what the indicator LED shows.
//!
//! Kept deliberately tiny: a colour (one of a fixed palette) and a
//! pattern (solid or one of two blink speeds). Producers post a new
//! [`LedState`] to [`LED_CHAN`] every time the visible state should
//! change; the consumer is `crate::tasks::led::led_task`.

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};

/// Indicator colours.
///
/// The palette is intentionally small — the LED is a status indicator,
/// not a display. Each colour is mapped to gamma-uncorrected linear RGB
/// in [`Colour::rgb`]; the [`crate::hw::led::LedDriver`] applies the
/// gamma curve before driving the LEDC duty registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    /// All channels off.
    Off,
    /// Hard error / battery critical.
    Red,
    /// Warning / pairing in progress.
    Amber,
    /// Ready / nominal.
    Green,
    /// Activity / syncing.
    Blue,
}

impl Colour {
    /// Linear 0..=255 RGB triple for this colour.
    pub const fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Off => (0, 0, 0),
            Self::Red => (255, 0, 0),
            // Amber sits at roughly 100 % red, 45 % green to read as
            // orange rather than yellow on the common-cathode LED.
            Self::Amber => (255, 115, 0),
            Self::Green => (0, 255, 0),
            Self::Blue => (0, 0, 255),
        }
    }
}

/// Blink pattern. The numeric periods are picked so that "slow" reads as
/// "thinking" and "fast" reads as "attention".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// Always on at the requested colour.
    Solid,
    /// On / off, 1 Hz (500 ms per phase).
    SlowBlink,
    /// On / off, 4 Hz (125 ms per phase).
    FastBlink,
}

impl Pattern {
    /// Milliseconds per phase. `Solid` returns `None`: the task does not
    /// arm an internal timer when no blinking is needed.
    pub const fn phase_ms(self) -> Option<u64> {
        match self {
            Self::Solid => None,
            Self::SlowBlink => Some(500),
            Self::FastBlink => Some(125),
        }
    }
}

/// Composite "what the LED should be doing". Posted to [`LED_CHAN`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedState {
    pub colour: Colour,
    pub pattern: Pattern,
}

impl LedState {
    pub const fn new(colour: Colour, pattern: Pattern) -> Self {
        Self { colour, pattern }
    }

    /// Convenience constructor for the most common case.
    pub const fn solid(colour: Colour) -> Self {
        Self::new(colour, Pattern::Solid)
    }
}

/// Bounded MPSC channel feeding the LED task.
///
/// `CriticalSectionRawMutex` is the right mutex flavour here: producers
/// may be embassy tasks **or** interrupt handlers (e.g. the future power
/// transition path may flash the LED red when a brown-out fires), and
/// only `CriticalSectionRawMutex` is sound from inside an ISR. Capacity
/// 4 absorbs the small burst that a power-state change can produce
/// without back-pressuring the producer side.
pub static LED_CHAN: Channel<CriticalSectionRawMutex, LedState, 4> = Channel::new();
