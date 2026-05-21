//! Audio decoding and playback.
//!
//! - [`decoder`] wraps the (TBD) MP3 decoder crate.
//! - [`mixer`] handles volume scaling and the fade-in/out used on
//!   pause/resume and card swap.
//! - [`playback`] glues decoder + mixer + I2S DMA buffer.

pub mod decoder;
pub mod mixer;
pub mod playback;
