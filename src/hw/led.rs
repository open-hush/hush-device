//! RGB LED control via three LEDC PWM channels.
//!
//! Common-cathode wiring: writing duty 0 turns the channel off, duty MAX
//! turns it fully on. Gamma correction is applied so that linear input
//! produces visually linear output.
//!
//! TODO(phase-1): bring up three LEDC channels for [`crate::hw::pins::LED_R`],
//! `LED_G`, `LED_B`. Provide `fn set_rgb(r: u8, g: u8, b: u8)`.
