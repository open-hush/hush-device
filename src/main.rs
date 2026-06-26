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
//! - SPI3 → microSD bring-up (400 kHz init clock, MBR + FAT32 probe of the
//!   first partition). The [`crate::hw::sdcard::SdCardDriver`] lives in a
//!   `'static` cell so the phase-3 cache task can pick it up without
//!   re-claiming SPI3 from the consumed `Peripherals`.
//! - I2S0 → MAX98357A and the `audio` task that streams a hardcoded
//!   440 Hz sine-wave tone through the speaker. Proves the I2S DMA
//!   path works; the SD-to-MP3-to-I2S pipeline lands in phase 3.
//!
//! The remaining phase-1 tasks (`cache`, `sync`, `input`, `power`) land
//! in subsequent commits.

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
    dma_buffers,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pin, Pull},
    ledc::{
        LSGlobalClkSource, Ledc, LowSpeed,
        timer::{self, TimerIFace},
    },
    rng::Rng,
    spi::{
        Mode as SpiMode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_wifi::EspWifiController;
use log::info;
use static_cell::StaticCell;

mod api;
mod audio;
mod certs;
mod config;
mod error;
mod hw;
mod proto;
mod storage;
mod tasks;

use crate::{
    hw::{
        i2s::AudioOutput,
        led::{self, LedDriver},
        mfrc522::RfidDriver,
        pins,
        sdcard::{SD_INIT_SPI_HZ, SdCardDriver},
        wifi::WifiCredentials,
    },
    proto::led::{Colour, LED_CHAN, LedState},
    tasks::{audio::audio_task, led::led_task, rfid::rfid_task, wifi::wifi_task},
};

// ESP-IDF App Descriptor. The second-stage bootloader reads this struct
// (magic, version, project name, build time, secure-boot bits) before
// handing control to our reset vector — without it `espflash` refuses to
// produce a flashable image.
esp_bootloader_esp_idf::esp_app_desc!();

/// Heap region carved out of PSRAM. 96 KiB covers Phase 1: logging
/// buffers + the ~64 KiB working set `esp-wifi` itself allocates for
/// the STA path (control blocks, scan buffers, supplicant context). The
/// TLS + JSON workspaces that Phase 2 adds will likely push this to
/// 128–160 KiB; revisit then rather than now.
const HEAP_SIZE: usize = 96 * 1024;

// Static cells for peripherals that need to outlive the `main` stack
// frame because an embassy task references them. The LEDC peripheral
// wrapper and its low-speed Timer0 are both kept alive for the program
// lifetime — the three LEDC channels owned by `LedDriver` hold `'static`
// borrows into them.
static LEDC_CELL: StaticCell<Ledc<'static>> = StaticCell::new();
static LEDC_TIMER_CELL: StaticCell<esp_hal::ledc::timer::Timer<'static, LowSpeed>> =
    StaticCell::new();

// SD driver lives forever once initialised so the phase-3 cache task
// can borrow it. `embedded-sdmmc` keeps a partition-table cache inside
// the `VolumeManager`; keeping the same handle avoids re-probing the
// MBR on every cache touch.
static SDCARD_CELL: StaticCell<SdCardDriver> = StaticCell::new();

// `esp-wifi`'s `init()` returns an `EspWifiController<'d>` whose
// lifetime drives the `'d` of every `WifiController` / `Interfaces`
// derived from it. The wifi task holds those derived handles for the
// program lifetime, so the controller itself must be `'static` — park
// it in a `StaticCell` and hand out a `&'static` borrow.
static WIFI_INIT_CELL: StaticCell<EspWifiController<'static>> = StaticCell::new();

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // 1. HAL bring-up. Crank the CPU to 240 MHz so audio decode + WiFi
    //    later have headroom; we drop it again in LIGHT_SLEEP (phase 4).
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));

    // 2. PSRAM heap. Anything we allocate before this call would panic;
    //    place the allocator first. The `size:` keyword form is from
    //    esp-alloc 0.8 — older versions took the size positionally.
    esp_alloc::heap_allocator!(size: HEAP_SIZE);

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
    let rfid_cs = Output::new(peripherals.GPIO44, Level::High, OutputConfig::default());

    // RST: active-low. Start high to release reset. The `RfidDriver`
    // owns this `Output` for the rest of the program so the pin stays
    // high even after `main` returns into the executor.
    let rfid_rst = Output::new(peripherals.GPIO43, Level::High, OutputConfig::default());

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

    // 8. SPI3 → microSD. 400 kHz first-bring-up clock (SD spec mandates
    //    ≤ 400 kHz for the init handshake; phase 3 re-clocks via the
    //    SdCard::spi(|spi| ...) closure once cache throughput matters).
    let sd_spi = Spi::new(
        peripherals.SPI3,
        SpiConfig::default()
            .with_frequency(Rate::from_hz(SD_INIT_SPI_HZ))
            .with_mode(SpiMode::_0),
    )
    .expect("SPI3 config failed")
    .with_sck(peripherals.GPIO12)
    .with_mosi(peripherals.GPIO11)
    .with_miso(peripherals.GPIO13);

    let sd_cs = Output::new(peripherals.GPIO10, Level::High, OutputConfig::default());

    // Card-detect is active-low when a card is seated. Enable the
    // internal pull-up so the line floats high (= "no card") when no
    // breakout is wired, instead of reading random states.
    let sd_cd = Input::new(
        peripherals.GPIO1,
        InputConfig::default().with_pull(Pull::Up),
    );

    match SdCardDriver::new(sd_spi, sd_cs, sd_cd) {
        Ok(driver) => {
            let driver = SDCARD_CELL.init(driver);
            info!(
                "sd: card present={}, size {} MiB",
                driver.card_present(),
                driver.card_size_bytes() / (1024 * 1024)
            );
            match driver.probe_first_volume() {
                Ok(()) => info!("sd: FAT32 volume 0 mounted"),
                Err(err) => log::error!("sd: FAT32 mount failed: {err:?}"),
            }
        }
        Err(err) => {
            // The most common cause here is "no card inserted" —
            // `SdCard::num_bytes` returns `SdCardError::CardNotFound`
            // in that case. Log and continue: the LED + RFID paths
            // still work without an SD.
            log::error!("sd: driver init failed: {err:?}");
        }
    }

    // 9. I2S0 → MAX98357A. Circular DMA, 16 KiB TX buffer (~93 ms of
    //    audio at 44.1 kHz × 4 bytes/frame) so the refill cadence in
    //    `audio_task` has comfortable headroom against underrun.
    //    `dma_buffers!(rx, tx)` allocates static arrays and gives us
    //    `&'static mut` handles, which is exactly what the I2S
    //    builder + circular transfer want.
    //
    // `clippy::manual_div_ceil` fires inside the macro expansion
    // (esp-hal's own descriptor-count math) — we can't fix it from
    // here, so allow it locally.
    #[allow(clippy::manual_div_ceil)]
    let (_, _, tx_buffer, tx_descriptors) = dma_buffers!(0, 16 * 1024);

    // SD pin on the MAX98357A is the gain/mode selector, not I2S
    // data. Drive high → "left channel only" mode (the only one that
    // makes sense for our mono content; the duplicated L=R samples
    // in `ToneSource` then play out on L).
    let amp_enable = Output::new(peripherals.GPIO3, Level::High, OutputConfig::default());

    let audio_output = AudioOutput::new(
        peripherals.I2S0,
        peripherals.DMA_CH0,
        peripherals.GPIO5, // BCLK
        peripherals.GPIO6, // LRC / WS
        peripherals.GPIO4, // DIN
        amp_enable,
        tx_descriptors,
    );

    spawner
        .spawn(audio_task(audio_output, tx_buffer))
        .expect("failed to spawn audio task");

    info!("phase 1: I2S audio task spawned (440 Hz tone)");

    // 10. WiFi STA bring-up. `esp_wifi::init` needs an independent
    //     timer source (TIMG0 already drives the embassy executor, so
    //     we hand it TIMG1.timer0), an RNG peripheral for PHY
    //     calibration, and the virtual RADIO_CLK. The returned
    //     `EspWifiController` owns the preempt-scheduler threads that
    //     drive the WiFi MAC; we park it in a `StaticCell` so the
    //     `WifiController` borrow that `wifi::new` hands out is
    //     `'static` — required because `wifi_task` outlives `main`.
    //
    //     A failure here is non-fatal: the LED + RFID + SD + I2S
    //     paths still work without WiFi (Phase 1 bench check can
    //     still complete), so we log and continue rather than
    //     panicking and losing the green LED.
    let timg1 = TimerGroup::new(peripherals.TIMG1);
    match esp_wifi::init(
        timg1.timer0,
        Rng::new(peripherals.RNG),
        peripherals.RADIO_CLK,
    ) {
        Ok(init) => {
            let init: &'static EspWifiController<'static> = WIFI_INIT_CELL.init(init);
            match esp_wifi::wifi::new(init, peripherals.WIFI) {
                // `_interfaces` (sta + ap WifiDevice handles) is the
                // hook embassy-net plugs into for the IP stack in
                // Phase 2. Drop it here and Phase 2 just takes the
                // device from `wifi::new` instead.
                Ok((controller, _interfaces)) => {
                    let creds = WifiCredentials::from_env();
                    info!(
                        "phase 1: WiFi STA task spawned, joining \"{}\"",
                        creds.ssid.as_str()
                    );
                    spawner
                        .spawn(wifi_task(controller, creds))
                        .expect("failed to spawn wifi task");
                }
                Err(err) => {
                    log::error!("wifi: controller construction failed: {err:?}");
                }
            }
        }
        Err(err) => {
            log::error!("wifi: esp-wifi init failed: {err:?}");
        }
    }

    // TODO(phase-5): on first boot (no Wi-Fi creds in NVS), bring the BLE
    // radio up and spawn the Improv pairing task instead of / alongside the
    // STA join above:
    //
    //   let controller = crate::hw::ble::ble_controller(init, peripherals.BT);
    //   let gatt = /* concrete ImprovGatt over the chosen host stack */;
    //   spawner.spawn(ble_pairing_task(gatt, provisioner));
    //
    // The Improv protocol core (`crate::proto::improv`) and the pairing
    // orchestration (`crate::tasks::ble::run_pairing`) are ready; the
    // concrete GATT server is the bench-pending piece (see
    // `docs/adr/0001-ble-host-stack.md`).
}
