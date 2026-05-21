//! Audio task — consumes `PlayAudio { audio_id }` commands, decodes MP3,
//! pushes PCM frames into the I2S DMA buffer.
//!
//! Stack: ~12 KB target (decoder workspace lives in PSRAM, but decoder
//! state machine touches local stack).
//!
//! TODO(phase-1): pull audio bytes from the SD cache, decode with the
//! chosen MP3 crate (`PLAN.md` decision pending), write to I2S TX.
