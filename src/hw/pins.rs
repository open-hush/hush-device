//! Pin map for the Hush hardware.
//!
//! **This file is the single source of truth for GPIO assignments.** If a
//! magic GPIO number appears anywhere else in the codebase, that's a bug —
//! lift the constant up here.
//!
//! Pin choices are constrained by the XIAO ESP32-S3 breakout layout, the
//! requirement to use two distinct SPI peripherals (SPI2 for the RFID
//! reader, SPI3 for the microSD), and the LEDC-capable GPIOs (35-37 are
//! valid LEDC channels on the S3).

// -----------------------------------------------------------------------------
// I2S audio out — MAX98357A (mono class-D amp + DAC).
// -----------------------------------------------------------------------------
pub const I2S_BCLK: u8 = 5;
pub const I2S_LRC: u8 = 6;
pub const I2S_DIN: u8 = 4;
/// MAX98357A SD (Shutdown / Mute) pin. Drive low to mute, high to play.
pub const I2S_SD: u8 = 3;

// -----------------------------------------------------------------------------
// SPI2 — RFID reader (MFRC522, 13.56 MHz MIFARE Classic).
// -----------------------------------------------------------------------------
pub const RFID_SCK: u8 = 7;
pub const RFID_MOSI: u8 = 9;
pub const RFID_MISO: u8 = 8;
pub const RFID_CS: u8 = 44;
pub const RFID_RST: u8 = 43;
/// MFRC522 IRQ — fires on card present (FIFO not empty). Drives the
/// `rfid` task wake.
pub const RFID_IRQ: u8 = 2;

// -----------------------------------------------------------------------------
// SPI3 — microSD card (FAT32, audio cache).
// -----------------------------------------------------------------------------
pub const SD_SCK: u8 = 12;
pub const SD_MOSI: u8 = 11;
pub const SD_MISO: u8 = 13;
pub const SD_CS: u8 = 10;
/// Card-detect switch. Active-low when a card is inserted.
pub const SD_CD: u8 = 1;

// -----------------------------------------------------------------------------
// KY-040 rotary encoder + push button (volume + play/pause).
// -----------------------------------------------------------------------------
pub const ENCODER_CLK: u8 = 17;
pub const ENCODER_DT: u8 = 18;
pub const ENCODER_SW: u8 = 21;

// -----------------------------------------------------------------------------
// Tactile buttons.
// -----------------------------------------------------------------------------
/// Soft reset (short press) / factory reset (10 s held with PAIRING).
pub const BTN_RESET: u8 = 33;
/// Pairing trigger and DEEP_SLEEP wake source.
pub const BTN_PAIRING: u8 = 34;

// -----------------------------------------------------------------------------
// RGB LED — common cathode. Driven via LEDC PWM (three independent channels).
// -----------------------------------------------------------------------------
pub const LED_R: u8 = 35;
pub const LED_G: u8 = 36;
pub const LED_B: u8 = 37;

// -----------------------------------------------------------------------------
// Wake-source GPIO masks.
//
// These bitmasks are passed to the deep-sleep configuration so the device
// can wake from RFID activity, encoder turns, button presses, or card
// insertion without re-entering ACTIVE prematurely.
// -----------------------------------------------------------------------------
pub const LIGHT_SLEEP_WAKE_PINS: &[u8] =
    &[RFID_IRQ, ENCODER_CLK, ENCODER_SW, BTN_PAIRING, BTN_RESET];

pub const DEEP_SLEEP_WAKE_PINS: &[u8] = &[RFID_IRQ, BTN_PAIRING];
