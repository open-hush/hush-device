//! I2S0 bring-up for the MAX98357A class-D amplifier.
//!
//! ## Configuration
//!
//! - **Standard**: Philips I2S (standard `LRCK` toggles on
//!   left/right sample boundary, data MSB-first, BCLK falling-edge
//!   sample).
//! - **Data format**: [`I2S_DATA_FORMAT`] (`Data16Channel16`) — 16-bit
//!   signed samples in 16-bit channel slots. Frame size 32 bits =
//!   L (16) + R (16). Matches the `i16` samples produced by
//!   [`crate::audio::playback::ToneSource`] and is the MAX98357A's
//!   native expectation.
//! - **Sample rate**: [`I2S_SAMPLE_RATE_HZ`] (44.1 kHz). CD quality;
//!   well below anything the speaker would reproduce poorly.
//! - **Pins**: from [`crate::hw::pins`] — BCLK 5, LRC 6, DIN 4. The
//!   "SD" pin on the MAX98357A datasheet is a **gain/mode select**,
//!   not an I2S line; we drive it high to enable the amp in
//!   "left channel only" mode (the only mode that makes sense for
//!   our mono content).
//!
//! ## MAX98357A `SD` pin semantics
//!
//! | Voltage on SD | Mode |
//! |---|---|
//! | `0 V` (low / GND) | Shutdown (amp disabled, ~1 µA quiescent) |
//! | `0.16–0.77 V` | Stereo: outputs `(L + R) / 2` |
//! | `0.77–1.4 V` | Right channel only |
//! | `> 1.4 V` (or floating with 100 kΩ pull-up) | Left channel only |
//!
//! We tie SD to logic high so the amp powers on and routes the L
//! channel. The duplicated L=R samples from `ToneSource` make this
//! a no-op for the audible output but keep options open if a stereo
//! source ever lands.

use esp_hal::{
    Blocking,
    dma::{DmaChannelFor, DmaDescriptor},
    gpio::{Output, interconnect::PeripheralOutput},
    i2s::{
        AnyI2s,
        master::{DataFormat, I2s, I2sTx, Standard},
    },
    peripherals::I2S0,
    time::Rate,
};

/// PCM sample rate, in Hz. Mirrored by
/// [`crate::audio::playback::TONE_SAMPLE_RATE_HZ`]; the two constants
/// must move together so the rendered tone keeps its intended pitch.
pub const I2S_SAMPLE_RATE_HZ: u32 = 44_100;

/// Data format. 16-bit samples in 16-bit channel slots is the only
/// combination the MAX98357A reads cleanly without a bit-width
/// reconfigure on its end (and it's also what the `i16` samples
/// produced by `ToneSource` map to one-to-one).
pub const I2S_DATA_FORMAT: DataFormat = DataFormat::Data16Channel16;

/// Frame size in bytes: two channels × 16 bits.
pub const I2S_FRAME_BYTES: usize = 4;

/// Concrete TX-only I2S handle the audio task takes ownership of.
pub type I2sTxHandle = I2sTx<'static, Blocking>;

/// I2S output bundle: the TX-only channel plus the GPIO that keeps
/// the MAX98357A enabled. The amp-enable handle has a leading
/// underscore because nothing reads it back — its purpose is to keep
/// the [`Output`] alive (and thus the pin high) for the lifetime of
/// the audio task. Dropping the `Output` would let the pin revert
/// and silence the speaker.
pub struct AudioOutput {
    pub i2s_tx: I2sTxHandle,
    _amp_enable: Output<'static>,
}

impl AudioOutput {
    /// Build the I2S TX channel and tie the amp-enable pin high.
    ///
    /// The DMA channel argument is the platform-specific GDMA
    /// channel (`peripherals.DMA_CH0` on ESP32-S3). Passing it in
    /// rather than picking one inside this module keeps the channel
    /// choice visible at the call site so SPI / I2S DMA assignments
    /// stay easy to audit.
    pub fn new(
        i2s_peripheral: I2S0<'static>,
        dma_channel: impl DmaChannelFor<AnyI2s<'static>>,
        bclk: impl PeripheralOutput<'static>,
        ws: impl PeripheralOutput<'static>,
        dout: impl PeripheralOutput<'static>,
        amp_enable: Output<'static>,
        tx_descriptors: &'static mut [DmaDescriptor],
    ) -> Self {
        let i2s = I2s::new(
            i2s_peripheral,
            Standard::Philips,
            I2S_DATA_FORMAT,
            Rate::from_hz(I2S_SAMPLE_RATE_HZ),
            dma_channel,
        );

        let i2s_tx = i2s
            .i2s_tx
            .with_bclk(bclk)
            .with_ws(ws)
            .with_dout(dout)
            .build(tx_descriptors);

        Self {
            i2s_tx,
            _amp_enable: amp_enable,
        }
    }
}
