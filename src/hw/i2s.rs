//! I2S bring-up for the MAX98357A.
//!
//! 16-bit PCM, mono, 44.1 kHz, MSB-first. The MAX98357A is a *transmit-only*
//! device for us; we never read from I2S.
//!
//! TODO(phase-1): provide `pub fn init(...) -> I2sTx<...>` that configures
//! pins per [`crate::hw::pins`] and returns a handle the `audio` task can
//! push samples into via DMA.
