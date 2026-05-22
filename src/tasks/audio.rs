//! Audio task — phase-1 bring-up.
//!
//! Owns the I2S TX channel and a [`ToneSource`], runs a circular DMA
//! transfer, and refills the buffer with sine-wave samples whenever
//! the DMA controller has consumed enough to make room. The result is
//! a continuous 440 Hz tone on the MAX98357A speaker — the audible
//! half of the phase-1 acceptance criterion.
//!
//! The decoder + SD-streaming pipeline (the MP3 path) lands in
//! phase 3; this task will then either swap its `ToneSource` for a
//! decoded-PCM source or be replaced wholesale by the cache task.
//!
//! Stack: 4 KiB. The body operates entirely on the stack-resident
//! [`ToneSource`] (a 12-byte struct) and the DMA descriptor list, so
//! the embassy default 1 KiB is enough; we sit at 4 KiB for backtrace
//! headroom if a panic ever fires.

use embassy_time::{Duration, Timer};
use log::{info, warn};

use crate::{
    audio::playback::{TONE_FREQ_HZ, ToneSource},
    hw::i2s::AudioOutput,
};

/// Stack-size justification: see module docstring.
const AUDIO_TASK_STACK: usize = 4096;

/// How often the refill loop wakes up. The circular DMA buffer is
/// 16 KiB → 4 KiB per quarter; at 176 KB/s (44.1 kHz × 4 bytes/frame)
/// each quarter drains in ~23 ms. Sleeping 10 ms keeps us well ahead
/// of underrun without spinning when the buffer is full.
const REFILL_TICK_MS: u64 = 10;

#[embassy_executor::task]
pub async fn audio_task(mut output: AudioOutput, tx_buffer: &'static mut [u8]) {
    let mut source = ToneSource::new();

    // `write_dma_circular` borrows `output.i2s_tx` and `tx_buffer` for
    // the rest of the task. The error path here only triggers on
    // misconfiguration (bad descriptor list, wrong DMA channel) — log
    // and exit cleanly rather than panic so the LED / RFID / SD paths
    // keep running.
    let mut transfer = match output.i2s_tx.write_dma_circular(&tx_buffer) {
        Ok(t) => t,
        Err(err) => {
            warn!("audio: write_dma_circular failed: {err:?}; audio task exiting");
            return;
        }
    };

    info!("audio: I2S DMA active, emitting {} Hz tone", TONE_FREQ_HZ);

    loop {
        // `available()` reports how many bytes of the circular buffer
        // have been drained by the DMA controller and can be safely
        // refilled. A `0` return means the DMA hasn't caught up yet,
        // which is the steady-state case immediately after our last
        // `push`.
        let avail = transfer.available().unwrap_or(0);

        if avail >= crate::hw::i2s::I2S_FRAME_BYTES {
            // `push_with` hands us a writable slice into the head of
            // the circular buffer. The closure must return the number
            // of bytes actually written; `ToneSource::fill_bytes`
            // returns the rounded-down-to-whole-frame count, so the
            // DMA never reads a half frame.
            if let Err(err) = transfer.push_with(|slot| source.fill_bytes(slot)) {
                // Underruns / overruns shouldn't happen at our refill
                // cadence, but if they do, log and keep going — the
                // next `available()` poll resyncs.
                warn!("audio: push_with failed: {err:?}");
            }
        }

        Timer::after(Duration::from_millis(REFILL_TICK_MS)).await;
    }
}

// Keep the constant referenced so future bench-driven stack sizing
// has a single place to update.
const _AUDIO_TASK_STACK_REF: usize = AUDIO_TASK_STACK;
