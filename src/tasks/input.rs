//! Input task — debounces the rotary encoder, the encoder push button, and
//! the two tactile buttons. Publishes high-level events: `VolumeChanged`,
//! `PlayPausePressed`, `ResetPressed`, `PairingPressed`.
//!
//! Stack: ~2 KB target.
//!
//! TODO(phase-4): debounce algorithm (5 ms), long-press detection for
//! reset (10 s), and chord detection for factory reset (reset + pairing
//! held 10 s).
