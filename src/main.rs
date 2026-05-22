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
//! - SPI2 → MFRC522 RFID reader and the polling `rfid` task that publishes
//!   [`crate::proto::events::Event::CardScanned`] onto
//!   [`crate::proto::events::EVENT_BUS`].
//!
//! The remaining phase-1 tasks (`audio`, `cache`, `sync`, `input`, `power`)
//! land in subsequent commits.

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
    gpio::{Level, Output, OutputConfig, Pin},
    ledc::{LSGlobalClkSource, Ledc, LowSpeed, timer::{self, TimerIFace}},
    spi::{Mode as SpiMode, master::{Config as SpiConfig, Spi}},
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
    hw::{
        led::{self, LedDriver},
        mfrc522::RfidDriver,
        pins,
    },
    proto::led::{Colour, LED_CHAN, LedState},
    tasks::{led::led_task, rfid::rfid_task},
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

    // 7. SPI2 → MFRC522. Mode 0, 1 MHz first-bring-up clock (well
    //    under the 10 MHz the chip supports; bump after bench
    //    validation if poll latency matters). Pin assignments come
    //    straight from the canonical `hw::pins` constants.
    let _ = pins::RFID_IRQ; // wired in hardware, not consumed yet — see hw::mfrc522 docstring.

    let rfid_spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(1))
            .with_mode(SpiMode::_0),
    )
    .expect("SPI2 config failed")
    .with_sck(peripherals.GPIO7)
    .with_mosi(peripherals.GPIO9)
    .with_miso(peripherals.GPIO8);

    // CS is driven by embedded-hal-bus's ExclusiveDevice; start it
    // high (deasserted) so the first transaction sees a clean edge.
    let rfid_cs = Output::new(
        peripherals.GPIO44,
        Level::High,
        OutputConfig::default(),
    );

    // RST: active-low. Start high to release reset. The `RfidDriver`
    // owns this `Output` for the rest of the program so the pin stays
    // high even after `main` returns into the executor.
    let rfid_rst = Output::new(
        peripherals.GPIO43,
        Level::High,
        OutputConfig::default(),
    );

    match RfidDriver::new(rfid_spi, rfid_cs, rfid_rst) {
        Ok(driver) => {
            spawner
                .spawn(rfid_task(driver))
                .expect("failed to spawn rfid task");
            info!("phase 1: MFRC522 driver up, rfid task polling");
        }
        Err(err) => {
            // Surface the failure without panicking. Without RFID the
            // device is useless, but a hard panic would lock the LED
            // path too; better to leave the green LED on and let the
            // operator see "rfid init failed" in the serial log.
            log::error!("rfid: driver init failed: {err:?}");
        }
    }
}
