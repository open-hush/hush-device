//! RGB LED driver via three LEDC PWM channels.
//!
//! The board uses a **common-cathode** RGB LED on `LED_R` / `LED_G` /
//! `LED_B`: writing a 0 duty cycle turns a channel off, max duty turns it
//! fully on. Gamma correction is applied so linear 0..=255 input produces
//! visually linear output rather than the wash-out you get when the
//! perceptual response curve goes unmodelled.
//!
//! ## PWM configuration
//!
//! - **Timer**: LEDC low-speed Timer0, clocked off the APB clock.
//! - **Resolution**: 8 bits (256 duty levels). 24-bit colour input means
//!   gamma correction can use the full byte without truncation visible to
//!   the eye.
//! - **Frequency**: [`LED_PWM_FREQ_HZ`] Hz. Well above the human flicker
//!   threshold (~60 Hz at the lowest), well below the I2S audio band so any
//!   stray switching noise is inaudible, and far enough from the LEDC clock
//!   divider's resolution floor at 8-bit duty.
//!
//! ## Trait split and lifetime model
//!
//! [`RgbLed`] is the abstract interface the LED task talks to; the
//! `mock-hardware` feature can ship a host-side substitute that records
//! the last `set_rgb` call into a buffer for unit tests. The concrete
//! [`LedDriver`] is built in `main` from a `'static` LEDC timer reference
//! (so the channels can outlive the constructor stack frame and be moved
//! into an embassy task) and the three GPIOs sourced from
//! [`crate::hw::pins`].

use esp_hal::{
    gpio::AnyPin,
    ledc::{
        LowSpeed,
        channel::{self, Channel, ChannelHW, ChannelIFace},
        timer::{self, Timer},
    },
};

/// PWM carrier frequency. 1 kHz is high enough to be invisible (the eye
/// fuses anything above ~80 Hz), low enough that the LEDC clock divider
/// keeps full 8-bit resolution at the APB clock, and outside the audible
/// band so it does not couple into the speaker amplifier later.
pub const LED_PWM_FREQ_HZ: u32 = 1_000;

/// Duty resolution. 8 bits gives 256 levels per channel, matches the
/// `u8` colour input, and keeps the gamma table lossless on the output
/// side.
pub const LED_PWM_DUTY_BITS: u32 = 8;
const LED_PWM_DUTY_MAX: u32 = (1 << LED_PWM_DUTY_BITS) - 1;

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

/// Errors the LED driver can surface. Configuration errors are the only
/// ones surfaced at runtime; once the channels are configured, `set_rgb`
/// only writes the duty register and cannot fail at the HAL level.
#[derive(Debug)]
pub enum LedError {
    /// `esp-hal` rejected the channel configuration (bad pin, conflicting
    /// timer speed, etc.).
    Configure(channel::Error),
}

impl From<channel::Error> for LedError {
    fn from(value: channel::Error) -> Self {
        Self::Configure(value)
    }
}

/// Convert the bit-resolution constant into the typed enum that
/// `esp-hal` consumes for timer configuration. Pulled out of `main` so
/// the bit count is owned by this module.
pub const fn pwm_duty_resolution() -> timer::config::Duty {
    match LED_PWM_DUTY_BITS {
        5 => timer::config::Duty::Duty5Bit,
        8 => timer::config::Duty::Duty8Bit,
        10 => timer::config::Duty::Duty10Bit,
        12 => timer::config::Duty::Duty12Bit,
        _ => panic!("unsupported LED_PWM_DUTY_BITS — extend pwm_duty_resolution"),
    }
}

/// Abstract RGB LED. The LED task depends on this trait, not on the
/// concrete [`LedDriver`], so a host-side mock can be swapped in under
/// the `mock-hardware` feature.
pub trait RgbLed {
    /// Drive the three channels. `r`, `g`, `b` are linear 0..=255; the
    /// implementation is expected to apply gamma correction before
    /// writing the LEDC duty register.
    fn set_rgb(&mut self, r: u8, g: u8, b: u8) -> Result<(), LedError>;
}

/// Concrete LED driver: owns the three configured LEDC channels.
///
/// The channels internally reference a [`Timer`] that must outlive the
/// driver. In `main` we allocate the timer in a [`static_cell::StaticCell`]
/// so its lifetime is `'static`, then move the driver into the LED
/// embassy task.
pub struct LedDriver {
    channel_r: Channel<'static, LowSpeed>,
    channel_g: Channel<'static, LowSpeed>,
    channel_b: Channel<'static, LowSpeed>,
}

impl LedDriver {
    /// Build the driver from a pre-configured `'static` LEDC timer
    /// reference and the three GPIO pins sourced from
    /// [`crate::hw::pins`]. The pins must be passed in already converted
    /// to [`AnyPin`] (via `pin.degrade()` in main) so this signature
    /// stays free of the per-GPIO marker types.
    pub fn new(
        timer: &'static Timer<'static, LowSpeed>,
        pin_r: AnyPin<'static>,
        pin_g: AnyPin<'static>,
        pin_b: AnyPin<'static>,
    ) -> Result<Self, LedError> {
        let channel_r = configure_channel(timer, channel::Number::Channel0, pin_r)?;
        let channel_g = configure_channel(timer, channel::Number::Channel1, pin_g)?;
        let channel_b = configure_channel(timer, channel::Number::Channel2, pin_b)?;
        Ok(Self {
            channel_r,
            channel_g,
            channel_b,
        })
    }
}

impl RgbLed for LedDriver {
    fn set_rgb(&mut self, r: u8, g: u8, b: u8) -> Result<(), LedError> {
        write_channel(&self.channel_r, GAMMA_LUT[r as usize]);
        write_channel(&self.channel_g, GAMMA_LUT[g as usize]);
        write_channel(&self.channel_b, GAMMA_LUT[b as usize]);
        Ok(())
    }
}

fn configure_channel(
    timer: &'static Timer<'static, LowSpeed>,
    number: channel::Number,
    pin: AnyPin<'static>,
) -> Result<Channel<'static, LowSpeed>, LedError> {
    let mut channel = Channel::new(number, pin);
    channel.configure(channel::config::Config {
        timer,
        duty_pct: 0,
        pin_config: channel::config::PinConfig::PushPull,
    })?;
    Ok(channel)
}

fn write_channel(channel: &Channel<'_, LowSpeed>, gamma_corrected: u8) {
    // Scale the gamma-corrected u8 (0..=255) to the timer's HW duty
    // range. At 8-bit resolution this is the identity; at higher
    // resolutions it scales linearly. We use the HW path so we keep full
    // 256-level granularity instead of the 101 levels the percent-based
    // `set_duty` API would give us.
    let duty = (gamma_corrected as u32) * LED_PWM_DUTY_MAX / 255;
    channel.set_duty_hw(duty);
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
