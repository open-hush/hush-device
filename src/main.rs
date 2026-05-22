//! Hush firmware — entry point.
//!
//! Initialises the HAL, sets up the PSRAM allocator, brings up the embassy
//! executor and spawns the per-concern tasks. Inter-task communication lives
//! in [`crate::proto::events`].
//!
//! Phase 1 only wires the bring-up: HAL init, PSRAM allocator, embassy time
//! driver, the logger and a heartbeat task that proves the executor runs.
//! Real tasks (`rfid`, `audio`, `cache`, `sync`, `input`, `power`, `led`) land
//! in subsequent phase-1 commits.

#![no_std]
#![no_main]
// Required by the embassy::task macro in nightly: it expands to an
// associated-type return that uses `impl Trait`.
#![feature(impl_trait_in_assoc_type)]
// Phase 1 baseline only wires the heartbeat; the scaffold modules (pins,
// proto, tasks, ...) stay dead until their phase opens. Drop this allow
// once those modules are actively used.
#![allow(dead_code)]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{clock::CpuClock, timer::timg::TimerGroup};
use log::info;

mod api;
mod audio;
mod config;
mod error;
mod hw;
mod proto;
mod storage;
mod tasks;

// ESP-IDF App Descriptor. The second-stage bootloader reads this struct
// (magic, version, project name, build time, secure-boot bits) before
// handing control to our reset vector — without it `espflash` refuses to
// produce a flashable image.
esp_bootloader_esp_idf::esp_app_desc!();

// Heap region carved out of PSRAM. 64 KiB is enough for phase 1 (logging
// buffers + room to grow when TLS / JSON tasks land). Bump in phase 2 when
// the WiFi + TLS stacks demand more.
const HEAP_SIZE: usize = 64 * 1024;

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // 1. HAL bring-up. Crank the CPU to 240 MHz so audio decode + WiFi later
    //    have headroom; we drop it again in LIGHT_SLEEP (phase 4).
    let peripherals =
        esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));

    // 2. PSRAM heap. Anything we allocate before this call would panic; place
    //    the allocator first.
    esp_alloc::heap_allocator!(HEAP_SIZE);

    // 3. Logger via esp-println over UART0 (USB-C CDC). Level controlled by
    //    the `ESP_LOG` env var at build time (default `info`, see
    //    `.cargo/config.toml`).
    esp_println::logger::init_logger_from_env();

    // 4. Embassy time driver. esp-hal-embassy wires SystimerAlarm or TIMG; we
    //    use TIMG0 because the systimer alarm is also wanted by esp-wifi when
    //    it lands in phase 2.
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timg0.timer0);

    info!("hush firmware booted — phase 1 heartbeat only");

    spawner
        .spawn(heartbeat())
        .expect("failed to spawn heartbeat task");

    // The main future returns immediately once the heartbeat task is on the
    // executor. The runtime keeps polling it.
}


/// Logs every 5 s so a UART monitor can confirm the firmware is alive.
/// Replaced by the real `led` task in the next phase-1 commit.
#[embassy_executor::task]
async fn heartbeat() {
    let mut ticks: u64 = 0;
    loop {
        info!("heartbeat tick={ticks}");
        ticks = ticks.wrapping_add(1);
        Timer::after(Duration::from_secs(5)).await;
    }
}
