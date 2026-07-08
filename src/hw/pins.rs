//! Pin map for the Hush hardware — Seeed Studio **XIAO ESP32-S3**.
//!
//! **This file is the single source of truth for GPIO assignments.** If a
//! magic GPIO number appears anywhere else in the codebase, that's a bug —
//! lift the constant up here.
//!
//! # Hard constraint: the XIAO only breaks out 11 GPIOs
//!
//! The XIAO ESP32-S3 exposes exactly **11** usable GPIOs on its castellated
//! pads:
//!
//! ```text
//!   D0=GPIO1  D1=GPIO2  D2=GPIO3  D3=GPIO4  D4=GPIO5  D5=GPIO6
//!   D6=GPIO43 D7=GPIO44 D8=GPIO7  D9=GPIO8  D10=GPIO9
//! ```
//!
//! GPIO 10-13, 17, 18, 21 and 33-37 are **not** available on this board.
//! Worse, GPIO 33-37 are wired to the module's **octal PSRAM** — driving them
//! as I/O corrupts PSRAM. Every assignment below stays inside the 11-pad
//! budget, and that budget is **fully consumed** (all 11 pads used).
//!
//! # v1 input/LED design (chosen 2026-07-07)
//!
//! The full feature set (rotary encoder + 2 buttons + PWM RGB LED) does not fit
//! the XIAO's 11 pads. The v1 build therefore:
//! - drops the KY-040 rotary **encoder** (volume moves to the app / long-press);
//! - uses a single **WS2812 / NeoPixel** RGB LED on ONE data pin (its own
//!   controller does the PWM) instead of a 3-GPIO common-cathode LED;
//! - uses ONE **multifunction button** ([`BTN_MAIN`]) on an RTC-capable pin so
//!   it can wake the MCU from DEEP_SLEEP;
//! - keeps the amp **mute** ([`I2S_SD`]) on a GPIO — pulling it low in
//!   DEEP_SLEEP is what makes the ~14 µA standby target reachable;
//! - drops the MFRC522 hardware **RST** (soft-reset over SPI instead) and the
//!   microSD **card-detect** (probe on mount) to free those pads.
//!
//! # One shared SPI bus (not two)
//!
//! There is no room for two independent SPI buses. The MFRC522 RFID reader and
//! the microSD therefore **share a single SPI bus** ([`SPI_SCK`] / [`SPI_MISO`]
//! / [`SPI_MOSI`]) with a dedicated chip-select each ([`RFID_CS`], [`SD_CS`]).
//! `main.rs` must build one bus, wrap it in an `embassy`/`embedded-hal-bus`
//! shared-bus `SpiDevice`, and hand each driver its own device + CS.
//!
//! # Strapping / default-function pins in use
//!
//! - `GPIO3` (D2, [`I2S_SD`]) is a **strapping** pin. Fine as an output after
//!   boot; just don't rely on its level during reset.
//! - `GPIO43`/`GPIO44` (D6/D7) are the default **UART0** TX/RX. We log over the
//!   native USB-CDC, not UART0, so they are free for [`SD_CS`] / [`RFID_CS`].

// -----------------------------------------------------------------------------
// I2S audio out — MAX98357A (mono class-D amp + DAC).
// -----------------------------------------------------------------------------
pub const I2S_BCLK: u8 = 5; // D4
pub const I2S_LRC: u8 = 6; // D5
pub const I2S_DIN: u8 = 4; // D3
/// MAX98357A SD (shutdown / gain-mode select). Drive low to mute, high to
/// play. `GPIO3` is a strapping pin — see module docs.
pub const I2S_SD: u8 = 3; // D2

// -----------------------------------------------------------------------------
// Shared SPI bus — MFRC522 RFID reader *and* microSD hang off this one bus.
// -----------------------------------------------------------------------------
pub const SPI_SCK: u8 = 7; // D8
pub const SPI_MISO: u8 = 8; // D9
pub const SPI_MOSI: u8 = 9; // D10

// -----------------------------------------------------------------------------
// RFID reader (MFRC522, 13.56 MHz MIFARE Classic) — chip-select only.
// -----------------------------------------------------------------------------
pub const RFID_CS: u8 = 44; // D7
// NOTE: no hardware RST pin in the 11-pad budget — the driver issues the
// MFRC522 SoftReset command over SPI at init instead. No IRQ pin either; the
// `rfid` task polls. Do not add RST/IRQ constants: there are no free GPIOs.

// -----------------------------------------------------------------------------
// microSD card (FAT32, audio cache) — chip-select only.
// -----------------------------------------------------------------------------
pub const SD_CS: u8 = 43; // D6
// NOTE: no card-detect pin in the 11-pad budget — presence is probed at mount
// time (a failed init == "no card"). Do not add SD_CD: no free GPIO.

// -----------------------------------------------------------------------------
// Status LED — single WS2812 / NeoPixel (one data line, driven over RMT).
// Replaces the 3-GPIO common-cathode LED, which did not fit and whose old pins
// (35/36/37) were the octal-PSRAM lines. The WS2812's own controller does the
// PWM, so full colour + brightness come from one pin.
// -----------------------------------------------------------------------------
pub const LED_WS2812: u8 = 2; // D1

// -----------------------------------------------------------------------------
// User input — ONE multifunction button.
// Short press = play/pause, long = pairing, extra-long = factory reset.
// MUST stay on an RTC-capable GPIO (0-21 on the S3) so it can wake the MCU from
// DEEP_SLEEP. GPIO1 qualifies.
//
// A second button would need a 12th pad we do not have; adding one means giving
// up [`I2S_SD`] (amp mute), which tanks DEEP_SLEEP standby current — don't,
// unless that trade is made deliberately. The KY-040 encoder is dropped in v1
// (see module docs).
// -----------------------------------------------------------------------------
pub const BTN_MAIN: u8 = 1; // D0 — RTC-capable, DEEP_SLEEP wake source

// -----------------------------------------------------------------------------
// Wake-source GPIO masks. The only direct wake source is the main button;
// RFID is polled (no IRQ pin) and there is no encoder.
// -----------------------------------------------------------------------------
pub const LIGHT_SLEEP_WAKE_PINS: &[u8] = &[BTN_MAIN];

pub const DEEP_SLEEP_WAKE_PINS: &[u8] = &[BTN_MAIN];
