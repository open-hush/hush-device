//! RGB status LED driver — a single WS2812 / NeoPixel over the RMT peripheral.
//!
//! v1 replaced the earlier 3-GPIO common-cathode LED (LEDC PWM) with one
//! addressable WS2812 on a single data pin ([`crate::hw::pins::LED_WS2812`]).
//! The WS2812's on-die controller does the PWM, so full 24-bit colour comes
//! from one GPIO — which is what lets the whole device fit the XIAO's 11 pads.
//!
//! The [`RgbLed`] trait seam is unchanged, so [`crate::tasks::led`] does not
//! care that the backend switched from LEDC to WS2812: it still calls
//! [`RgbLed::set_rgb`] with linear 0..=255 components and this module applies
//! the gamma LUT before handing bytes to the LED.
//!
//! TODO(bench): `esp-hal-smartled` + `smart-leds` must be version-pinned
//! against `esp-hal 1.0.0-beta.1` the same way `esp-wifi` was (see PLAN.md /
//! Cargo.toml). The [`SmartLedsAdapter`] channel type and [`LED_RMT_BUFFER`]
//! const below are written without a compiler in the loop and need a
//! `cargo check --target xtensa-esp32s3-none-elf` pass to confirm.

use esp_hal::{
    Blocking,
    gpio::AnyPin,
    rmt::{Channel, ChannelCreator},
};
use esp_hal_smartled::{SmartLedsAdapter, buffer_size};
use smart_leds::{RGB8, SmartLedsWrite};

/// Number of LEDs on the status chain — one.
pub const LED_COUNT: usize = 1;

/// RMT symbol-buffer length for [`LED_COUNT`] WS2812 pixels. `buffer_size` is
/// a `const fn`, so this stays a plain associated const (no
/// `generic_const_exprs` needed) and can be used as the adapter's const
/// generic argument.
pub const LED_RMT_BUFFER: usize = buffer_size(LED_COUNT);

/// Gamma look-up table built at compile time. A quadratic curve
/// (`out = i² / 255`) approximates a 2.0 gamma closely enough for the
/// "indicator LED" use case; the perceptual midpoint lands near input 180
/// instead of input 128, so set_rgb(128, …) looks roughly half-bright
/// rather than uncomfortably bright.
const GAMMA_LUT: [u8; 256] = {
    let mut lut = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        lut[i] = ((i * i) / 255) as u8;
        i += 1;
    }
    lut
};

/// Errors the LED driver can surface.
#[derive(Debug)]
pub enum LedError {
    /// The RMT transmit of the WS2812 bit-stream failed. Not fatal — the LED
    /// task logs and keeps going (LED writes are never on a hot path).
    Write,
}

/// Abstract RGB LED. The LED task depends on this trait, not on the
/// concrete [`LedDriver`], so a host-side mock can be swapped in under
/// the `mock-hardware` feature.
pub trait RgbLed {
    /// Drive the LED. `r`, `g`, `b` are linear 0..=255; the implementation
    /// applies gamma correction before emitting the WS2812 frame.
    fn set_rgb(&mut self, r: u8, g: u8, b: u8) -> Result<(), LedError>;
}

/// Concrete LED driver: owns the WS2812 RMT adapter.
///
/// Built in `main` from RMT channel 0 and the [`crate::hw::pins::LED_WS2812`]
/// data pin, then moved into the LED embassy task. The RMT symbol buffer lives
/// inside the adapter, sized by [`LED_RMT_BUFFER`].
pub struct LedDriver {
    adapter: SmartLedsAdapter<Channel<Blocking, 0>, LED_RMT_BUFFER>,
}

impl LedDriver {
    /// Build the driver from RMT channel-0's `ChannelCreator` (what
    /// `Rmt::channel0` hands out) and the WS2812 data pin (already converted to
    /// [`AnyPin`] via `pin.degrade()` in `main`). `SmartLedsAdapter::new`
    /// consumes the creator and configures it into the `Channel<Blocking, 0>`
    /// stored in [`Self::adapter`].
    pub fn new(channel: ChannelCreator<Blocking, 0>, pin: AnyPin<'static>) -> Self {
        let adapter = SmartLedsAdapter::new(channel, pin, [0u32; LED_RMT_BUFFER]);
        Self { adapter }
    }
}

impl RgbLed for LedDriver {
    fn set_rgb(&mut self, r: u8, g: u8, b: u8) -> Result<(), LedError> {
        let colour = RGB8::new(
            GAMMA_LUT[r as usize],
            GAMMA_LUT[g as usize],
            GAMMA_LUT[b as usize],
        );
        self.adapter
            .write([colour].into_iter())
            .map_err(|_| LedError::Write)
    }
}

// ---------------------------------------------------------------------
// Host-side mock for unit tests.
//
// Available only under `--features mock-hardware`. Records the last
// `set_rgb` call so test code can assert on it; the LED task can be
// driven against this mock without an esp32 target.
// ---------------------------------------------------------------------
#[cfg(feature = "mock-hardware")]
pub mod mock {
    use super::{LedError, RgbLed};

    /// Trivial mock that remembers the most recent `set_rgb(...)` call.
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub struct MockLed {
        /// Most recent (r, g, b) written. `None` before the first call.
        pub last: Option<(u8, u8, u8)>,
        /// Number of `set_rgb` invocations seen.
        pub writes: u32,
    }

    impl RgbLed for MockLed {
        fn set_rgb(&mut self, r: u8, g: u8, b: u8) -> Result<(), LedError> {
            self.last = Some((r, g, b));
            self.writes = self.writes.wrapping_add(1);
            Ok(())
        }
    }
}
