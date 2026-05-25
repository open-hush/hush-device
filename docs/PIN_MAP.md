# Pin map

Canonical pin assignments for the Hush hardware. **Source of truth is [`src/hw/pins.rs`](../src/hw/pins.rs)** — this document mirrors it for human consumption.

For the system block diagram and pin-to-pin wiring of the breadboard build,
see [`HARDWARE.md`](./HARDWARE.md).

## Board

Seeed Studio XIAO ESP32-S3 (8 MB PSRAM variant). GPIO numbering follows the [Seeed pinout diagram](https://wiki.seeedstudio.com/xiao_pin_multiplexing_esp33s3/).

## Peripherals overview

| Peripheral | Signals | GPIOs |
|---|---|---|
| I2S audio out (MAX98357A) | BCLK / LRC / DIN / SD-mute | 5, 6, 4, 3 |
| SPI2 — RFID (MFRC522) | SCK / MOSI / MISO / CS / RST / IRQ | 7, 9, 8, 44, 43, 2 |
| SPI3 — microSD | SCK / MOSI / MISO / CS / CD | 12, 11, 13, 10, 1 |
| Encoder KY-040 | CLK / DT / SW | 17, 18, 21 |
| Buttons | Reset / Pairing-Wake | 33, 34 |
| RGB LED (common cathode) | R / G / B | 35, 36, 37 |

## Detailed table

| GPIO | Function | Direction | Notes |
|---:|---|---|---|
| 1  | SD_CD       | input pull-up   | Card detect (active-low) |
| 2  | RFID_IRQ    | input pull-up   | MFRC522 IRQ — wakes RFID task |
| 3  | I2S_SD      | output          | MAX98357A shutdown; high = enabled |
| 4  | I2S_DIN     | output          | I2S data into amp |
| 5  | I2S_BCLK    | output          | I2S bit clock |
| 6  | I2S_LRC     | output          | I2S left/right (channel select) |
| 7  | RFID_SCK    | output          | SPI2 clock |
| 8  | RFID_MISO   | input           | SPI2 data in |
| 9  | RFID_MOSI   | output          | SPI2 data out |
| 10 | SD_CS       | output          | microSD chip select |
| 11 | SD_MOSI     | output          | SPI3 data out |
| 12 | SD_SCK      | output          | SPI3 clock |
| 13 | SD_MISO     | input           | SPI3 data in |
| 17 | ENCODER_CLK | input pull-up   | KY-040 A — wake source |
| 18 | ENCODER_DT  | input pull-up   | KY-040 B |
| 21 | ENCODER_SW  | input pull-up   | KY-040 push — wake source |
| 33 | BTN_RESET   | input pull-up   | Soft reset / factory chord |
| 34 | BTN_PAIRING | input pull-up   | Pairing trigger / DEEP_SLEEP wake |
| 35 | LED_R       | output (LEDC)   | RGB LED red |
| 36 | LED_G       | output (LEDC)   | RGB LED green |
| 37 | LED_B       | output (LEDC)   | RGB LED blue |
| 43 | RFID_RST    | output          | MFRC522 hardware reset |
| 44 | RFID_CS     | output          | MFRC522 chip select |

## Strapping / boot considerations

- GPIO 0 is reserved for the strapping (boot mode select). **Not used by Hush.**
- GPIO 45 / 46 are strapping pins on the ESP32-S3 — **avoid** for general I/O.
- USB-OTG (GPIO 19 / 20) is reserved for the USB-C port. We rely on it for flashing and the serial monitor.

## Wake sources

- `LIGHT_SLEEP` wakes on: `RFID_IRQ`, `ENCODER_CLK`, `ENCODER_SW`, `BTN_PAIRING`, `BTN_RESET`.
- `DEEP_SLEEP` wakes on: `RFID_IRQ`, `BTN_PAIRING`. (Encoder and reset are intentionally NOT deep-sleep wake sources — they require a more decisive press.)

Wake-source bitmasks live in [`src/hw/pins.rs`](../src/hw/pins.rs) as `LIGHT_SLEEP_WAKE_PINS` and `DEEP_SLEEP_WAKE_PINS`.

## Power notes

- The XIAO ESP32-S3 has an onboard Li-Po charger (~100 mA charge current via USB-C). The battery JST connector on the underside is used as-is.
- `MAX98357A SD` (GPIO 3) **must** be pulled low before entering DEEP_SLEEP — otherwise the amp idles at ~3 mA and dominates standby draw.

## Open questions

> See `PLAN.md` § Decisions open.

- Battery low-voltage cutoff (3.3 V vs 3.4 V) — to be measured on bench.
- Should we expose a hardware "USB only / battery only" jumper? Currently no.
