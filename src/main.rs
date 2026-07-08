//! Hush firmware — entry point.
//!
//! Initialises the HAL, sets up the PSRAM allocator, brings up the embassy
//! executor and spawns the per-concern tasks. Inter-task communication lives
//! in [`crate::proto::events`] (broadcast pubsub) and per-task channels such
//! as [`crate::proto::led::LED_CHAN`].
//!
//! Phase 1 currently wires (v1 XIAO ESP32-S3 pin map — see `hw::pins`):
//! - HAL bring-up + PSRAM allocator + embassy time driver + logger.
//! - A single WS2812 status LED (RMT, GPIO 2) and the `led` task that consumes
//!   [`crate::proto::led::LedState`] updates.
//! - One **shared** SPI bus (GPIO 7/8/9) carrying both the MFRC522 RFID reader
//!   (CS GPIO 44) and the microSD (CS GPIO 43); each gets a
//!   `CriticalSectionDevice` handle on the `'static` bus. The polling `rfid`
//!   task publishes [`crate::proto::events::Event::CardScanned`] onto
//!   [`crate::proto::events::EVENT_BUS`]; the [`crate::hw::sdcard::SdCardDriver`]
//!   lives in a `'static` cell so the phase-3 cache task can borrow it.
//! - I2S0 → MAX98357A and the `audio` task that streams a hardcoded
//!   440 Hz sine-wave tone through the speaker. Proves the I2S DMA
//!   path works; the SD-to-MP3-to-I2S pipeline lands in phase 3.
//! - The main multifunction button (GPIO 1) is claimed; its handling lands in
//!   Phase 4 (`input`/`power`).
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

use core::cell::RefCell;

use critical_section::Mutex;
use embassy_executor::Spawner;
use embedded_hal_bus::spi::CriticalSectionDevice;
use esp_backtrace as _;
use esp_hal::{
    Blocking,
    clock::CpuClock,
    delay::Delay,
    dma_buffers,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pin, Pull},
    rmt::Rmt,
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
        led::LedDriver,
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

// The one shared SPI bus lives for the program lifetime: both the RFID device
// handle (moved into the rfid task) and the SD device handle (parked in
// `SDCARD_CELL`) borrow it, so it must outlive `main`. It is guarded by a
// critical-section `Mutex<RefCell<…>>` so the two `SpiDevice` handles can share
// it safely across tasks.
static SPI_BUS_CELL: StaticCell<Mutex<RefCell<Spi<'static, Blocking>>>> = StaticCell::new();

// Compile-time guard tying the typed HAL pin handles used in `main` back to the
// canonical `hw::pins` map. The esp-hal peripheral singletons (`peripherals.
// GPIOn`) are the only way to name a pin, so these asserts fail the build if a
// GPIO here ever drifts from `pins.rs`.
const _: () = assert!(pins::I2S_DIN == 4 && pins::I2S_BCLK == 5 && pins::I2S_LRC == 6);
const _: () = assert!(pins::I2S_SD == 3);
const _: () = assert!(pins::LED_WS2812 == 2);
const _: () = assert!(pins::SPI_SCK == 7 && pins::SPI_MISO == 8 && pins::SPI_MOSI == 9);
const _: () = assert!(pins::RFID_CS == 44 && pins::SD_CS == 43);
const _: () = assert!(pins::BTN_MAIN == 1);

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

    info!("hush firmware booted — bringing up status LED");

    // 5. RMT → single WS2812 status LED on GPIO 2 (pins::LED_WS2812). The
    //    WS2812's on-die controller does the PWM, so one data pin gives full
    //    colour — which is what lets the whole device fit the XIAO's 11 pads.
    //    80 MHz RMT source clock gives the ~1.25 µs WS2812 bit period enough
    //    resolution.
    let rmt = Rmt::new(peripherals.RMT, Rate::from_mhz(80)).expect("rmt init failed");
    let led_driver = LedDriver::new(rmt.channel0, peripherals.GPIO2.degrade());

    spawner
        .spawn(led_task(led_driver))
        .expect("failed to spawn led task");

    // 6. Initial visible signal: solid green = ready. The pubsub-shaped
    //    `LED_CHAN` is bounded; `try_send` cannot fail here because
    //    `led_task` was just spawned and the channel is empty.
    LED_CHAN
        .try_send(LedState::solid(Colour::Green))
        .expect("led channel rejected initial state");

    info!("phase 1: status LED online (solid green)");

    // 7. One shared SPI bus for RFID + microSD. The XIAO has no room for two
    //    SPI buses, so both peripherals hang off SPI2 on the canonical shared
    //    pins (pins::SPI_SCK/MISO/MOSI) with a dedicated CS each. Mode 0,
    //    started at the SD init clock (400 kHz) — the slowest requirement on
    //    the bus; the MFRC522 tolerates it fine for first bring-up.
    let spi_bus = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_hz(SD_INIT_SPI_HZ))
            .with_mode(SpiMode::_0),
    )
    .expect("shared SPI config failed")
    .with_sck(peripherals.GPIO7)
    .with_mosi(peripherals.GPIO9)
    .with_miso(peripherals.GPIO8);

    // Park the bus in a 'static critical-section mutex so both device handles
    // can share it (the SD handle outlives `main`).
    let spi_bus: &'static Mutex<RefCell<Spi<'static, Blocking>>> =
        SPI_BUS_CELL.init(Mutex::new(RefCell::new(spi_bus)));

    // `CriticalSectionDevice::new` returns `Result<_, Infallible>` (its only
    // failure would be the CS pin's `Error`, which is `Infallible` for an
    // esp-hal `Output`), so the `expect`s below can never actually fire.

    // 7a. MFRC522 on the shared bus. CS on GPIO 44 (pins::RFID_CS), started
    //     high (deasserted). No hardware RST pin — the driver soft-resets over
    //     SPI in `init()`. No IRQ pin — the `rfid` task polls.
    let rfid_cs = Output::new(peripherals.GPIO44, Level::High, OutputConfig::default());
    let rfid_device = CriticalSectionDevice::new(spi_bus, rfid_cs, Delay::new())
        .expect("rfid SPI device (CS error is Infallible)");

    match RfidDriver::new(rfid_device) {
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

    // 7b. microSD on the same shared bus. CS on GPIO 43 (pins::SD_CS). No
    //     card-detect pin — presence is inferred from the init handshake.
    let sd_cs = Output::new(peripherals.GPIO43, Level::High, OutputConfig::default());
    let sd_device = CriticalSectionDevice::new(spi_bus, sd_cs, Delay::new())
        .expect("sd SPI device (CS error is Infallible)");

    match SdCardDriver::new(sd_device) {
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

    // 8. Main multifunction button on GPIO 1 (pins::BTN_MAIN), RTC-capable so
    //    it can wake from DEEP_SLEEP. Pull-up: the button shorts to GND when
    //    pressed. Input handling (short = play/pause, long = pairing, held =
    //    factory reset) lands with the `input`/`power` tasks in Phase 4; the
    //    pad is claimed here so it is reserved and documented.
    let _btn_main = Input::new(
        peripherals.GPIO1,
        InputConfig::default().with_pull(Pull::Up),
    );

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
}
