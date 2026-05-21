//! Volume and fade.
//!
//! Volume is applied in software as a log-curve multiplier on PCM
//! samples. Fades are 200 ms linear ramps on pause/resume and card swap
//! to avoid speaker pops.
//!
//! TODO(phase-1): apply gain in a tight loop without alloc; precompute a
//! 256-entry gain table indexed by the user-facing 0–100 volume value.
