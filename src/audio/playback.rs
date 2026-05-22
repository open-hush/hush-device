//! Phase-1 audio source: a 440 Hz sine-wave tone generator that feeds
//! the I2S task with stereo `i16` samples.
//!
//! ## What this is *not*
//!
//! - Not the MP3 decoder. That lands in phase 3 (audio cache) once the
//!   SD-to-I2S pipeline exists and `minimp3-sys` is wired into the
//!   build. The decision to use `minimp3-sys` is captured in `PLAN.md`.
//! - Not a mixer. There is only one source; the
//!   [`crate::audio::mixer`] module is scaffold for future
//!   crossfading.
//!
//! The job of this module is to prove "the speaker emits sound" so the
//! phase-1 acceptance criterion ("a hardcoded MP3 or raw PCM plays
//! through the speaker") is met without bringing the decoder online.
//!
//! ## Sine table
//!
//! 256 entries, signed 16-bit, generated at compile time via Bhaskara
//! I's approximation `sin(x) ≈ 16·x·(π−x) / (5π² − 4·x·(π−x))`. Error
//! is ~0.2 % — well below what is audible against the MAX98357A's
//! own noise floor.
//!
//! ## Phase accumulator
//!
//! 32-bit fixed-point phase. The high byte indexes the table; the
//! lower 24 bits accumulate the per-sample increment, which is
//! `tone_hz * 2^32 / sample_rate_hz` so the increment exactly matches
//! the desired frequency at any sample rate without floating-point.

/// Output amplitude. Sub-32k so we leave a few dB of headroom against
/// the MAX98357A's hard clip point. 25_000 ≈ −2.3 dBFS — loud but not
/// distorting on a typical 3.7 V Li-Po.
const TONE_AMPLITUDE: i32 = 25_000;

/// Tone frequency. A4 (concert pitch) — distinctive on a bench
/// speaker, low enough that the parabolic-approximated harmonic
/// content stays inaudible.
pub const TONE_FREQ_HZ: u32 = 440;

/// Samples per second. Matches the I2S clock in
/// [`crate::hw::i2s::I2S_SAMPLE_RATE_HZ`]. Mismatch between the two
/// numbers would just retune the tone — both should change together
/// for the right pitch on the speaker.
pub const TONE_SAMPLE_RATE_HZ: u32 = 44_100;

/// Compile-time 256-entry sine table, signed 16-bit. Index range is
/// the full [0, 256) period.
const SINE_TABLE: [i16; 256] = build_sine_table();

const fn build_sine_table() -> [i16; 256] {
    let mut table = [0i16; 256];
    let mut i: i32 = 0;
    while i < 256 {
        // Fold to a single half-period and apply sign explicitly so
        // the approximation runs on values in [0, 128].
        let (n, sign) = if i < 128 { (i, 1i32) } else { (256 - i, -1i32) };

        // Bhaskara I: sin(π·n/128) ≈ 16·n·(128−n) / (5·128² − 4·n·(128−n))
        let bracket = n * (128 - n);
        let num = 16 * bracket;
        let den = 5 * 128 * 128 - 4 * bracket;

        // Scale [0, 1] approximation to [0, TONE_AMPLITUDE]. The
        // multiplication is done in i64 to avoid wrap during the
        // intermediate step before the division reins it back in.
        let scaled = ((num as i64) * (TONE_AMPLITUDE as i64) / (den as i64)) as i32;
        table[i as usize] = (sign * scaled) as i16;

        i += 1;
    }
    table
}

/// Stateful tone generator. One instance lives in the audio task; on
/// each call, `fill_frames` writes a stereo block of samples into a
/// caller-provided buffer.
#[derive(Debug, Clone, Copy)]
pub struct ToneSource {
    /// Q8.24 phase: top 8 bits index the table, low 24 bits accumulate
    /// the sub-sample fraction so the per-sample increment is exact.
    phase: u32,
    /// Per-sample phase increment, precomputed from the configured
    /// frequency and sample rate.
    phase_inc: u32,
}

impl ToneSource {
    /// Build a tone source at the canonical [`TONE_FREQ_HZ`] /
    /// [`TONE_SAMPLE_RATE_HZ`] pair.
    pub const fn new() -> Self {
        // `phase_inc` = freq * 2^32 / sample_rate, computed in u64 so
        // the shift does not overflow at common audio frequencies.
        let inc = ((TONE_FREQ_HZ as u64) << 32) / (TONE_SAMPLE_RATE_HZ as u64);
        Self {
            phase: 0,
            phase_inc: inc as u32,
        }
    }

    /// Fill consecutive stereo (L, R) frames into `dst`. Returns the
    /// number of bytes actually written, which is always a multiple
    /// of 4 (one frame = two `i16` samples) and never exceeds
    /// `dst.len()`.
    ///
    /// The same sample is written to both channels — the MAX98357A is
    /// a mono amp, so L vs R only matters for its SD-pin gain mode.
    /// Duplicating the sample keeps the option of a "stereo summed"
    /// gain mode open.
    pub fn fill_bytes(&mut self, dst: &mut [u8]) -> usize {
        let bytes = dst.len() & !3; // round down to whole frames
        let frame_count = bytes / 4;

        for frame in 0..frame_count {
            let idx = (self.phase >> 24) as usize;
            let sample = SINE_TABLE[idx];
            let le = sample.to_le_bytes();

            let off = frame * 4;
            dst[off] = le[0];
            dst[off + 1] = le[1];
            dst[off + 2] = le[0];
            dst[off + 3] = le[1];

            self.phase = self.phase.wrapping_add(self.phase_inc);
        }

        bytes
    }
}

impl Default for ToneSource {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------
// Compile-time sanity checks.
// ---------------------------------------------------------------------
const _ASSERT_AMPLITUDE_HEADROOM: () = {
    assert!(TONE_AMPLITUDE > 0);
    assert!(TONE_AMPLITUDE < 32_000);
};
