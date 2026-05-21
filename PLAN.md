# `hush-device` — plan

This is the firmware roadmap for Hush. The device is a Seeed Studio **XIAO ESP32-S3** running Rust `no_std` with [embassy](https://embassy.dev/) on top of [`esp-hal`](https://github.com/esp-rs/esp-hal).

## Purpose

A battery-powered, child-friendly audio player that:

- Reads RFID cards (MIFARE Classic 13.56 MHz) and plays the linked audio.
- Streams audio from local microSD cache; falls back to HTTP download (presigned URL) on miss.
- Pairs over BLE (Improv-WiFi) on first boot, syncs over WiFi/HTTPS.
- Sleeps aggressively. Target: > 8 hours active playback, > 4 weeks on standby.

---

## Stack

| Layer | Crate / tech |
|---|---|
| Toolchain | `esp` toolchain via `espup` |
| Target | `xtensa-esp32s3-none-elf` |
| HAL | `esp-hal` 0.22 |
| Async runtime | `embassy-executor`, `embassy-time`, `embassy-sync` |
| WiFi + BLE | `esp-wifi` (radio split between WiFi and BT host) |
| HTTPS | `reqwless` + `embedded-tls` |
| JSON | `serde-json-core` |
| Crypto | `hmac` + `sha2` (HMAC-SHA256 for device auth) |
| SD card | `embedded-sdmmc` |
| RFID | `mfrc522` |
| Internal flash KV store | `sequential-storage` + `esp-storage` |
| Logging | `esp-println` + `log` |
| MP3 decoder | **TBD** (see decisions below) |

---

## Pin map

Canonical: [`src/hw/pins.rs`](./src/hw/pins.rs). Tabular: [`docs/PIN_MAP.md`](./docs/PIN_MAP.md).

```
I2S audio (MAX98357A):
  BCLK GPIO 5, LRC GPIO 6, DIN GPIO 4, SD (mute) GPIO 3

SPI #1 RFID (MFRC522) — SPI2:
  SCK GPIO 7, MOSI GPIO 9, MISO GPIO 8, CS GPIO 44, RST GPIO 43, IRQ GPIO 2

SPI #2 microSD — SPI3:
  SCK GPIO 12, MOSI GPIO 11, MISO GPIO 13, CS GPIO 10, CD GPIO 1

Encoder KY-040: CLK GPIO 17, DT GPIO 18, SW GPIO 21
Buttons:        Reset GPIO 33, Pairing/Wake GPIO 34
LED RGB:        R GPIO 35, G GPIO 36, B GPIO 37
```

---

## Power model

| Mode | Trigger | Power | Wake | Recovery |
|---|---|---|---|---|
| `ACTIVE` | Playing / interacting | full | — | — |
| `LIGHT_SLEEP` | > 30 s idle | ~1 mA, RAM preserved | GPIO | < 50 ms |
| `DEEP_SLEEP` | > 5 min idle | ~14 µA, RTC only | GPIO | ~500 ms boot |

Timeouts dashboard-configurable (`DeviceConfig.lightSleepAfterSec`, `deepSleepAfterSec`).

---

## Phase 1 — Hello world hardware (~1-2 weeks)

Acceptance: tap a card, see UID in the serial log, the LED changes colour, a hardcoded MP3 plays through the speaker.

- [ ] `espup install` + `cargo check --target xtensa-esp32s3-none-elf` clean on a fresh checkout.
- [ ] LED RGB bring-up with LEDC PWM (3 channels).
- [ ] UART logging via `esp-println` at 115200.
- [ ] WiFi STA basic connect (credentials hardcoded for now).
- [ ] MFRC522 SPI bring-up, IRQ-driven UID read.
- [ ] microSD SPI bring-up, FAT32 mount.
- [ ] I2S out to MAX98357A: play a hardcoded MP3 (or raw PCM) from SD.
- [ ] Decision: select MP3 decoder crate (see decisions below).

## Phase 2 — Sync with backend (~2 weeks)

Acceptance: device appears in the dashboard, syncs configuration, posts events.

- [ ] HMAC-SHA256 signing module (host-tested).
- [ ] HTTPS client over `reqwless` + `embedded-tls`.
- [ ] `POST /v1/device/register` on first boot, persist returned `deviceId`.
- [ ] `GET /v1/device/sync` every `syncIntervalSec` (default 600).
- [ ] `POST /v1/device/events` flushing buffered events.
- [ ] Persist secret, deviceId and last config to NVS via `sequential-storage`.

## Phase 3 — Audio cache on SD (~2 weeks)

Acceptance: tapping a known card always plays within < 500 ms when cached; cache miss triggers download.

- [ ] `embedded-sdmmc` mount and basic file I/O.
- [ ] Streaming download from presigned URL into SD with atomic write + fsync.
- [ ] SHA-256 verification on every read; mismatch → evict.
- [ ] LRU eviction when free space < 10 %.
- [ ] Cache index in `/cache/index.bin` updated on every change.

## Phase 4 — Encoder, buttons, power (~1-2 weeks)

Acceptance: 24 hours of mixed use on a single charge without crash; idle drain matches spec.

- [ ] KY-040 rotation → volume; press → play/pause.
- [ ] Reset button + Pairing button handlers.
- [ ] State machine: `ACTIVE → LIGHT_SLEEP → DEEP_SLEEP` with GPIO wake.
- [ ] Validate idle power on bench (< 1 mA LIGHT, < 20 µA DEEP).

## Phase 5 — BLE Improv WiFi (~2 weeks)

Acceptance: user sets up the device from the mobile app with no cables and no edited files.

- [ ] GATT server with Improv WiFi service UUIDs.
- [ ] Receive SSID + password over BLE.
- [ ] Attempt WiFi connect; report status back over BLE characteristic.
- [ ] Trigger `POST /v1/device/register` on success.
- [ ] Tear down BLE stack after pairing (frees ~30 KB RAM).

## Phase 6 — OTA (~2 weeks)

Acceptance: backend can push a new firmware build to a device; rollback is automatic on boot failure.

- [ ] A/B partition scheme already declared in `partitions.csv`; verify use.
- [ ] OTA endpoint contract (see `hush-protocol`).
- [ ] Download, signature-verify (Ed25519), write inactive slot.
- [ ] Set boot flag, reboot, validate boot, commit or rollback.

---

## Conventions

- `#![no_std]` strict. Anything that needs `alloc` is **explicit** and lives in PSRAM.
- No `unwrap()` or `panic!` in hot paths (audio task, RFID task, sync task). Errors propagate with `?` or degrade gracefully (LED red blink + log).
- Every `embassy::task` has its **stack size** declared and a comment justifying the value.
- Critical sections are short and never contain `.await`.
- Large buffers go to PSRAM via `#[link_section = ".ext_ram.bss"]`.
- Hardware-touching code lives in `src/hw/`; tasks consume traits from there so they can be unit-tested on the host with the `mock-hardware` feature.

---

## Decisions taken

- **MCU**: XIAO ESP32-S3 (8 MB PSRAM, USB-C native, small form factor, well-supported by `esp-hal`).
- **Audio**: I2S to MAX98357A. No DAC over PWM (poor SNR for spoken word).
- **RFID**: MFRC522 over SPI. The clones are cheap and the `mfrc522` crate is reliable.
- **Storage source of truth**: NVS (internal flash) for secrets and config; microSD is **only** an audio cache.
- **WiFi pairing**: BLE Improv WiFi (open standard, supported by ESPHome and home automation tools).
- **Auth to backend**: HMAC-SHA256 with a per-device 32-byte secret baked into NVS at provisioning.
- **OTA scheme**: A/B partitions, Ed25519 signature on the binary.

## Decisions open

- **MP3 decoder**:
  - **Candidate A**: bindings to **Helix MP3** (battle-tested at Adafruit, C code, ~13 KB code + ~30 KB RAM).
  - **Candidate B**: `minimp3` via FFI (small, public domain, single header).
  - **Candidate C**: `puremp3` (pure Rust, less mature, slower).
  - **Resolution**: bench A vs B in phase 1; pick the one that decodes 128 kbps mono in real time with < 60 % CPU.
- **Battery low-voltage cutoff**: cut at 3.4 V or 3.3 V? Sparkfun PCM cuts at 2.5 V (too low). We probably want to refuse to start below 3.4 V to protect the cells. Confirm on bench.
- **SD card spec**: 8 GB or 16 GB high-endurance as recommended default? Document the part number families known to work.
- **TLS root certs**: bundle ISRG Root X1 only (Let's Encrypt), or DigiCert + Amazon Root too? Smaller = faster boot. Default ISRG only unless we hit issues.

---

## Cross-repo touch points

- `hush-protocol/hush-api.yaml` — wire types in `src/proto/api.rs` and `src/proto/events.rs` must match. Drift is caught by CI (TODO: add `oapi-codegen` style check).
- `hush-backend` — `POST /v1/device/register` must return `claimCode` for the user dashboard.
- `hush-app` — BLE Improv WiFi pairing flow (phase 5) must agree on service UUIDs and characteristic shapes.

---

## Out of scope (forever)

- DRM. The device can play any locally cached MP3 the user provides.
- Voice recognition / wake words.
- Streaming from arbitrary HTTP servers — only via the Hush backend's presigned URLs (security boundary).
- Multi-room audio sync.
