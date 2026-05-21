//! MP3 decoding.
//!
//! Decoder crate is **TBD**: see `PLAN.md` § Decisions open. The chosen
//! decoder will be wrapped in a thin trait so swapping is cheap.
//!
//! TODO(phase-1): pick decoder, wrap as `Decoder` trait with `read_frame
//! -> Result<PcmFrame>` plus an init routine that reports sample rate,
//! channels and bitrate to the mixer.
