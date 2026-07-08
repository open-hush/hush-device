# hush-device

> Firmware for the Hush RFID-activated audio device. Rust `no_std`, [embassy](https://embassy.dev/), running on a Seeed Studio **XIAO ESP32-S3**.

[![license](https://img.shields.io/badge/license-MIT-green)](./LICENSE)
[![target](https://img.shields.io/badge/target-xtensa--esp32s3--none--elf-orange)](./.cargo/config.toml)

Hush is an open-source RFID-activated audio device for children — see [open-hush.com](https://open-hush.com).

---

## Hardware

| Component | Part |
|---|---|
| MCU | Seeed Studio XIAO ESP32-S3 (8 MB PSRAM) |
| Audio | MAX98357A I2S amp + 8 Ω 3 W passive speaker |
| RFID | MFRC522 (SPI, MIFARE Classic 13.56 MHz) |
| Storage | microSD via SPI, **shared bus with RFID** (high-endurance, 8–32 GB) |
| Input | 1 multifunction button (v1; rotary encoder deferred — no free pins) |
| Feedback | Single WS2812 status LED (one data pin, RMT) |
| Power | Li-Po 3.7 V 2000 mAh (Sparkfun 505060), USB-C charging |

Full pin map: [`docs/PIN_MAP.md`](./docs/PIN_MAP.md). The canonical pin definitions live in [`src/hw/pins.rs`](./src/hw/pins.rs).

---

## Toolchain setup

The Xtensa target (`xtensa-esp32s3-none-elf`) is not supported by stable `rustc`. You need the **Espressif Rust toolchain** via `espup`.

```bash
# Once
cargo install espup espflash
espup install

# Every shell (or add to ~/.zshrc)
source $HOME/export-esp.sh
```

Verify:

```bash
rustc --version --verbose | grep host
# host: should mention the esp toolchain
cargo check --target xtensa-esp32s3-none-elf
```

If you cannot install `espup` (some CI runners), see [`docs/DEVELOPMENT.md`](./docs/DEVELOPMENT.md) for the workaround.

---

## Build, flash, run

```bash
# Build
cargo build --release

# Flash + open serial monitor (requires a XIAO ESP32-S3 plugged in via USB-C)
cargo run --release

# Just monitor an already-flashed device
espflash monitor

# Wipe flash (factory reset)
espflash erase-flash
```

`cargo run` invokes the `espflash` runner configured in [`.cargo/config.toml`](./.cargo/config.toml) and uses the partition table in [`partitions.csv`](./partitions.csv).

---

## Host-side tests

Pure-logic modules (HMAC canonicalization, JSON shapes, state machines) are testable on the host:

```bash
cargo test --features mock-hardware --target x86_64-apple-darwin
# or: aarch64-apple-darwin, x86_64-unknown-linux-gnu — pick your host target.
```

The `mock-hardware` feature stubs out HAL-dependent code so tests compile on a non-Xtensa host.

---

## Project layout

```
src/
├── main.rs            # Entry: init HAL, spawn tasks, run executor
├── config.rs          # Compile-time + runtime config
├── error.rs           # Crate-wide error enum
├── hw/                # Hardware abstraction (pins, I2S, MFRC522, microSD, LED)
├── tasks/             # One embassy task per concern
├── audio/             # Decoding + playback
├── api/               # HTTP client + HMAC signing
├── proto/             # Wire types matching hush-protocol/hush-api.yaml
└── storage/           # NVS, event outbox, cache index
```

Architecture details and patterns: [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md).

---

## Status

This is **phase 0** — scaffolding only. Most modules are stubs. The [`PLAN.md`](./PLAN.md) defines six phases; one phase per session is the norm. Do not implement out-of-phase work.

---

## License

MIT — see [`LICENSE`](./LICENSE).
