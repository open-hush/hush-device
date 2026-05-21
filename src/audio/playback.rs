//! Playback orchestrator: glues [`crate::audio::decoder`] →
//! [`crate::audio::mixer`] → I2S DMA buffer.
//!
//! Owns the decode/mix loop, the I2S TX handle, and the current playback
//! state (`Idle`, `Playing(audio_id)`, `Paused`).
//!
//! TODO(phase-1): double-buffer DMA writes; transition states cleanly on
//! play/pause and card-swap events from [`crate::proto::events`].
