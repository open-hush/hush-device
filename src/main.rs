//! Hush firmware — entry point.
//!
//! Initialises the HAL, sets up PSRAM allocation, brings up the embassy
//! executor, and spawns one task per concern. Tasks communicate via
//! `embassy_sync` channels; see [`crate::proto::events`] for the inter-task
//! event types.
//!
//! TODO(phase-1): wire up the actual task spawn calls once each task is real.

#![no_std]
#![no_main]
#![allow(dead_code)]
#![allow(clippy::empty_loop)]

use esp_backtrace as _;
use esp_println as _;

mod config;
mod error;
mod hw;
mod tasks;
mod proto;
mod api;
mod storage;
mod audio;

#[esp_hal_embassy::main]
async fn main(_spawner: embassy_executor::Spawner) {
    // TODO(phase-1):
    //   1. `esp_hal::init` with default `Config`.
    //   2. `esp_alloc::psram_allocator!` for the 8 MB PSRAM region.
    //   3. Bring up `esp-wifi` controller (WiFi + BLE coexistence).
    //   4. Spawn tasks: rfid, audio, cache, sync, input, power, led.
    //   5. Yield to the executor forever.
    loop {
        embassy_time::Timer::after_secs(60).await;
    }
}
