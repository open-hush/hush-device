//! Hush firmware — entry point.
//!
//! Initialises the HAL, sets up the PSRAM allocator, brings up the embassy
//! executor and spawns the per-concern tasks. Inter-task communication lives
//! in [`crate::proto::events`] (broadcast pubsub) and per-task channels such
//! as [`crate::proto::led::LED_CHAN`].
//!
//! Phase 1 currently wires:
//! - HAL bring-up + PSRAM allocator + embassy time driver + logger.
//! - The LED RGB driver (three LEDC PWM channels on GPIO 35/36/37) and the
//!   `led` task that consumes [`crate::proto::led::LedState`] updates.
//!
//! The remaining phase-1 tasks (`rfid`, `audio`, `cache`, `sync`, `input`,
//! `power`) land in subsequent commits.

#![no_std]
#![no_main]
// Required by the embassy::task macro in nightly: it expands to an
// associated-type return that uses `impl Trait`.
#![feature(impl_trait_in_assoc_type)]
// Scaffold modules (audio, cache, rfid, …) remain unused until their
// phase opens. Drop this allow once every module is actively referenced.
#![allow(dead_code)]

use embassy_executor::Spawner;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    gpio::Pin,
    ledc::{LSGlobalClkSource, Ledc, LowSpeed, timer::{self, TimerIFace}},
    time::Rate,
    timer::timg::TimerGroup,
};
use log::info;
use static_cell::StaticCell;

mod api;
mod audio;
mod config;
mod error;
mod hw;
mod proto;
mod storage;
mod tasks;

use crate::{
    hw::led::{self, LedDriver},
    proto::led::{Colour, LED_CHAN, LedState},
    tasks::led::led_task,
};

// ESP-IDF App Descriptor. The second-stage bootloader reads this struct
// (magic, version, project name, build time, secure-boot bits) before
// handing control to our reset vector — without it `espflash` refuses to
// produce a flashable image.
esp_bootloader_esp_idf::esp_app_desc!();

/// Heap region carved out of PSRAM. 64 KiB is enough for phase 1 (logging
/// buffers + room to grow when TLS / JSON tasks land). Bump in phase 2
/// when the WiFi + TLS stacks demand more.
const HEAP_SIZE: usize = 64 * 1024;

// Static cells for peripherals that need to outlive the `main` stack
// frame because an embassy task references them. The LEDC peripheral
// wrapper and its low-speed Timer0 are both kept alive for the program
// lifetime — the three LEDC channels owned by `LedDriver` hold `'static`
// borrows into them.
static LEDC_CELL: StaticCell<Ledc<'static>> = StaticCell::new();
static LEDC_TIMER_CELL: StaticCell<esp_hal::ledc::timer::Timer<'static, LowSpeed>> =
    StaticCell::new();

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // 1. HAL bring-up. Crank the CPU to 240 MHz so audio decode + WiFi
    //    later have headroom; we drop it again in LIGHT_SLEEP (phase 4).
    let peripherals =
        esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));

    // 2. PSRAM heap. Anything we allocate before this call would panic;
    //    place the allocator first.
    esp_alloc::heap_allocator!(HEAP_SIZE);

    // 3. Logger via esp-println over UART0 (USB-C CDC). Level controlled
    //    by the `ESP_LOG` env var at build time (default `info`, see
    //    `.cargo/config.toml`).
    esp_println::logger::init_logger_from_env();

    // 4. Embassy time driver. esp-hal-embassy wires SystimerAlarm or
    //    TIMG; we use TIMG0 because the systimer alarm is also wanted by
    //    esp-wifi when it lands in phase 2.
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_hal_embassy::init(timg0.timer0);

    info!("hush firmware booted — bringing up LED RGB");

    // 5. LEDC peripheral, timer and the three RGB channels.
    let mut ledc_value = Ledc::new(peripherals.LEDC);
    ledc_value.set_global_slow_clock(LSGlobalClkSource::APBClk);
    let ledc: &'static Ledc<'static> = LEDC_CELL.init(ledc_value);

    let mut timer_value = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    timer_value
        .configure(timer::config::Config {
            duty: led::pwm_duty_resolution(),
            clock_source: timer::LSClockSource::APBClk,
            frequency: Rate::from_hz(led::LED_PWM_FREQ_HZ),
        })
        .expect("ledc timer configure failed — LED_PWM_FREQ_HZ out of range");
    let timer = LEDC_TIMER_CELL.init(timer_value);

    let driver = LedDriver::new(
        timer,
        peripherals.GPIO35.degrade(),
        peripherals.GPIO36.degrade(),
        peripherals.GPIO37.degrade(),
    )
    .expect("led driver configure failed");

    spawner
        .spawn(led_task(driver))
        .expect("failed to spawn led task");

    // 6. Initial visible signal: solid green = ready. The pubsub-shaped
    //    `LED_CHAN` is bounded; `try_send` cannot fail here because
    //    `led_task` was just spawned and the channel is empty.
    LED_CHAN
        .try_send(LedState::solid(Colour::Green))
        .expect("led channel rejected initial state");

    info!("phase 1: LED RGB online (solid green)");
}
