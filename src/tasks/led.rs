//! LED task — owns the [`crate::hw::led::LedDriver`] and renders the
//! current [`LedState`] onto the three LEDC channels.
//!
//! Listens on [`crate::proto::led::LED_CHAN`] for state updates. While a
//! blink pattern is active, the task wakes itself every half-period to
//! toggle the channels on and off; while the pattern is `Solid` it
//! sleeps on the channel only, so an idle task consumes nothing.
//!
//! Stack size: 2 KiB. The body does not allocate, does not recurse, and
//! its widest call (`embassy_futures::select::select` over two futures)
//! takes ~400 B of frame; 2 KiB leaves comfortable headroom for backtrace
//! frames if a panic ever fires.

use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};
use log::warn;

use crate::{
    hw::led::{LedDriver, RgbLed},
    proto::led::{Colour, LED_CHAN, LedState},
};

/// Stack size justified above. Embassy tasks default to 1 KiB which is
/// too tight for the select + log path.
const LED_TASK_STACK: usize = 2048;

#[embassy_executor::task]
pub async fn led_task(mut driver: LedDriver) {
    // Initial state: LED off. A producer (typically `main` right after
    // spawn) sends the real first state — `Off` here just guarantees we
    // don't ship random PWM values out of the boot configuration.
    let mut state = LedState::solid(Colour::Off);
    apply_static(&mut driver, state.colour);

    // `blink_on` tracks the half-period of the current blink: when true,
    // the LED is showing `state.colour`; when false, it is showing
    // `Colour::Off`. Reset to `true` whenever the state changes.
    let mut blink_on = true;

    loop {
        match state.pattern.phase_ms() {
            None => {
                // Solid: just block on the channel until something
                // changes. No internal timer, no CPU cost.
                state = LED_CHAN.receive().await;
                blink_on = true;
                apply_static(&mut driver, state.colour);
            }
            Some(phase_ms) => {
                // Blinking: race the channel against the next toggle.
                let next_toggle = Timer::after(Duration::from_millis(phase_ms));
                match select(LED_CHAN.receive(), next_toggle).await {
                    Either::First(new_state) => {
                        state = new_state;
                        blink_on = true;
                        apply_static(&mut driver, state.colour);
                    }
                    Either::Second(()) => {
                        blink_on = !blink_on;
                        let visible = if blink_on { state.colour } else { Colour::Off };
                        apply_static(&mut driver, visible);
                    }
                }
            }
        }
    }
}

/// Tiny wrapper that writes a `Colour` to the driver and logs (without
/// panicking) if the write fails. LED writes are not on the audio /
/// RFID / sync hot paths — per the conventions in CLAUDE.md, the
/// graceful-degradation rule is "log and keep going", not propagate.
fn apply_static(driver: &mut LedDriver, colour: Colour) {
    let (r, g, b) = colour.rgb();
    if let Err(err) = driver.set_rgb(r, g, b) {
        warn!("led: set_rgb({r},{g},{b}) failed: {err:?}");
    }
}

// Keep the stack-size constant referenced so the value cannot be
// silently deleted; embassy does not consume it directly from the task
// macro yet, but we want to revisit it when `cargo size` measurements
// against real hardware come in.
const _LED_TASK_STACK_REF: usize = LED_TASK_STACK;
