# Pin map

Canonical pin assignments for the Hush hardware. **Source of truth is [`src/hw/pins.rs`](../src/hw/pins.rs)** — this document mirrors it for human consumption.

For the system block diagram and pin-to-pin wiring of the breadboard build,
see [`HARDWARE.md`](./HARDWARE.md).

## Board

Seeed Studio XIAO ESP32-S3 (8 MB PSRAM variant). GPIO numbering follows the [Seeed pinout diagram](https://wiki.seeedstudio.com/xiao_pin_multiplexing_esp33s3/).

> **The XIAO exposes only 11 GPIOs.** Its castellated pads map to GPIO
> `1,2,3,4,5,6,7,8,9,43,44` — that's the entire budget. GPIO `10-13, 17, 18,
> 21` and `33-37` are **not** broken out, and `33-37` are wired to the module's
> **octal PSRAM** (driving them corrupts PSRAM). All 11 pads are used. RFID and
> microSD **share one SPI bus** (no room for two).
>
> **v1 input/LED design (chosen 2026-07-07).** The encoder + 2 buttons + PWM LED
> do not fit. v1 drops the rotary encoder, uses a single **WS2812** LED (1 pin)
> and **one multifunction button**, keeps the amp mute (for standby current),
> and drops the MFRC522 RST (soft-reset over SPI) and microSD card-detect.

## D-pad ↔ GPIO reference

| Pad | D0 | D1 | D2 | D3 | D4 | D5 | D6 | D7 | D8 | D9 | D10 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| GPIO | 1 | 2 | 3 | 4 | 5 | 6 | 43 | 44 | 7 | 8 | 9 |

## Peripherals overview

| Peripheral | Signals | GPIOs |
|---|---|---|
| I2S audio out (MAX98357A) | BCLK / LRC / DIN / SD-mute | 5, 6, 4, 3 |
| Shared SPI bus | SCK / MISO / MOSI | 7, 8, 9 |
| RFID (MFRC522) | CS | 44 |
| microSD | CS | 43 |
| Status LED (WS2812) | DIN | 2 |
| Button (multifunction) | — | 1 |

## Detailed table (all 11 pads)

| GPIO | Pad | Function | Direction | Notes |
|---:|---|---|---|---|
| 1  | D0  | BTN_MAIN   | input pull-up | Multifunction button; **RTC-capable = DEEP_SLEEP wake** |
| 2  | D1  | LED_WS2812 | output (RMT)  | Single WS2812/NeoPixel data line |
| 3  | D2  | I2S_SD     | output        | MAX98357A shutdown; high = enabled. **Strapping pin.** |
| 4  | D3  | I2S_DIN    | output        | I2S data into amp |
| 5  | D4  | I2S_BCLK   | output        | I2S bit clock |
| 6  | D5  | I2S_LRC    | output        | I2S left/right (channel select) |
| 7  | D8  | SPI_SCK    | output        | Shared SPI clock (RFID + SD) |
| 8  | D9  | SPI_MISO   | input         | Shared SPI data in |
| 9  | D10 | SPI_MOSI   | output        | Shared SPI data out |
| 43 | D6  | SD_CS      | output        | microSD chip select. Default UART0 TX. |
| 44 | D7  | RFID_CS    | output        | MFRC522 chip select. Default UART0 RX. |

## Dropped in v1 (no free pins)

- **Rotary encoder (KY-040)** — deferred; volume via app / long-press. Reading it through an I2C expander is timing-marginal, so it is not bolted on.
- **Second button** — one multifunction button covers play/pause + pairing + reset. A second would cost `I2S_SD` (amp mute) and thus standby battery.
- **MFRC522 hardware RST** — driver issues a `SoftReset` command over SPI instead.
- **microSD card-detect** — presence inferred at mount.

Adding any of these back means an **I2C GPIO expander** (PCF8575) — which itself
needs 2 free pins the board doesn't have without dropping something above, and
can't PWM the LED or reliably clock a fast encoder. Treat as a board-change
trigger, not a quick add. See `PLAN.md § Decisions open`.

## Strapping / boot considerations

- `GPIO3` (D2, `I2S_SD`) is a strapping pin — safe as an output after boot; don't rely on its level during reset.
- `GPIO43`/`GPIO44` (D6/D7) are the default UART0 TX/RX. Logging goes over the native **USB-CDC**, not UART0, so both are free for chip-selects.
- USB-OTG (GPIO 19/20) is reserved for the USB-C port (flashing + serial monitor).

## Wake sources

- `LIGHT_SLEEP` and `DEEP_SLEEP` both wake on `BTN_MAIN` (GPIO 1, RTC-capable).
- RFID is polled (no IRQ pin) and there is no encoder, so the button is the only direct wake source. A card tap wakes the device via the button press that precedes it in the intended UX (press to wake, then tap).

Wake-source slices live in [`src/hw/pins.rs`](../src/hw/pins.rs) as `LIGHT_SLEEP_WAKE_PINS` / `DEEP_SLEEP_WAKE_PINS`.

## Power notes

- The XIAO ESP32-S3 has an onboard Li-Po charger (~100 mA charge current via USB-C). The battery JST connector on the underside is used as-is.
- `MAX98357A SD` (GPIO 3) **must** be pulled low before entering DEEP_SLEEP — otherwise the amp idles at ~3 mA and dominates standby draw.
- **Amp supply (open):** the XIAO `5V` pad only has voltage over USB, not on battery. Powering the MAX98357A from `3V3` works (~0.5–1 W into the 8 Ω speaker); a 5 V boost gives more headroom. See `PLAN.md`.

## Open questions

> See `PLAN.md` § Decisions open.

- Input + LED strategy (I2C expander vs. board change) — **blocks Phase 1 LED and Phase 4 input/power.**
- Amp supply: 3V3 vs. 5 V boost.
- Battery low-voltage cutoff (3.3 V vs 3.4 V) — to be measured on bench.
