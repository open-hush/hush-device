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

- [x] `espup install` + `cargo check --target xtensa-esp32s3-none-elf` clean on a fresh checkout.
- [x] **Link green end to end**: `cargo build --release --target xtensa-esp32s3-none-elf` produces an ELF, `espflash save-image --chip esp32s3` produces a flashable image (~112 KB, ~2.7 % of the 4 MB app partition with the LED + RFID + SD + I2S bring-ups in).
- [x] **LED RGB bring-up with LEDC PWM (3 channels)**: `src/hw/led.rs` wraps three LEDC low-speed channels (Timer0, 1 kHz, 8-bit duty, compile-time quadratic gamma LUT) behind the `RgbLed` trait. `src/tasks/led.rs` consumes `LedState { colour, pattern }` from `LED_CHAN` and renders Solid / SlowBlink (1 Hz) / FastBlink (4 Hz) over the canonical palette (Off / Red / Amber / Green / Blue). `main.rs` boots the driver and posts `Colour::Green Solid` once the executor is up. **Bench verification (eye-check the LED actually lights green on the XIAO ESP32-S3) still pending.**
- [ ] UART logging via `esp-println` at 115200 — boot lines emit (`hush firmware booted — bringing up LED RGB` / `phase 1: LED RGB online (solid green)` / `phase 1: MFRC522 driver up, rfid task polling` / `rfid: mfrc522 reported version 0x91` / `sd: card present=true, size N MiB` / `sd: FAT32 volume 0 mounted` / `audio: I2S DMA active, emitting 440 Hz tone` / `phase 1: I2S audio task spawned (440 Hz tone)` / `phase 1: WiFi STA task spawned, joining "<ssid>"` / `wifi: radio started, joining "<ssid>"` / `wifi: associated with "<ssid>"`); bench verification still pending.
- [x] **WiFi STA basic connect (credentials hardcoded)**: `src/hw/wifi.rs` exposes a `WifiCredentials` struct fed from `env!("HUSH_WIFI_SSID")` / `env!("HUSH_WIFI_PSK")` at build time, with a `const _: () = assert!(SSID.len() <= 32, …)` guard so an over-long SSID/PSK fails the build with a precise message rather than panicking at boot. `src/tasks/wifi.rs` owns the `WifiController`, applies a WPA2-Personal `ClientConfiguration`, runs `start_async` once and then loops on `connect_async → wait_for_event(StaDisconnected) → 2 s back-off`; a `connect_async` error backs off 5 s instead. `main.rs` brings up `esp_wifi::init(timg1.timer0, Rng::new(RNG), RADIO_CLK)` (TIMG0 stays with the embassy executor), parks the returned `EspWifiController` in `WIFI_INIT_CELL` so the `'static` borrow survives `main`, then `wifi::new` hands back `(WifiController, Interfaces)` — the STA + AP `WifiDevice` halves of `Interfaces` are intentionally dropped because no IP stack exists yet (embassy-net + DHCP lands in Phase 2). `HEAP_SIZE` bumped 64 KiB → 96 KiB to cover esp-wifi's working set. **Pinning decisions:** `esp-wifi = "=0.14.0"` (the first tag whose `esp-hal = "1.0.0-beta.1"` requirement *and* `xtensa-lx-rt 0.19 / xtensa-lx 0.11` lineage actually link against ours — `0.13.0` advertised the same `esp-hal` range but transitively pulled `xtensa-lx-rt 0.18 / xtensa-lx 0.10`, tripping the `links = "xtensa-lx"` rule); `esp-alloc` bumped 0.6 → 0.8 to unify with the version esp-wifi pulls in (two `#[global_allocator]` macros in the same binary is a link error), which also moves the `heap_allocator!` macro to the keyword-arg form (`heap_allocator!(size: HEAP_SIZE)`); the `log-04` feature on esp-wifi replaces the renamed `log` feature it used pre-beta.1. **No-IP-stack scope is intentional for Phase 1** — Phase 2 adds embassy-net / DHCP / DNS / TCP on top of the same `Interfaces` handle. **Release image grew 112 KiB → 492 KiB (23.5 % of the 2 MiB OTA slot), entirely from `esp-wifi-sys`'s FreeRTOS-style adapters; the audio + RFID + SD paths still link clean.** **Bench verification (eye-check `wifi: associated with "<ssid>"` over UART, then yank power on the AP and confirm the 2 s reconnect line) still pending.**
- [x] **MFRC522 SPI bring-up, UID read (polling)**: `src/hw/mfrc522.rs` wraps `mfrc522 = 0.7` over an `ExclusiveDevice<Spi<…, Blocking>, Output, NoDelay>` from `embedded-hal-bus`, behind an `RfidReader` trait. SPI2 on SCK 7 / MOSI 9 / MISO 8 / CS 44, mode 0, 1 MHz first-bring-up clock. `src/tasks/rfid.rs` polls every 100 ms, dedupes consecutive identical UIDs, halts the card after a successful select, and publishes `Event::CardScanned { uid, uid_len }` onto the new `proto::events::EVENT_BUS` (`PubSubChannel`, capacity 8, 4 subscribers, 4 publishers, `CriticalSectionRawMutex` for future ISR producers). **Bench verification (eye-check that a real Mifare card publishes a UID over UART) still pending.** **IRQ-driven path deferred** — `mfrc522` 0.7 keeps its register read/write methods private and offers no IRQ enable/clear surface, so going IRQ-driven would need either a fork or raw-SPI register pokes; the 100 ms polling latency is well inside the "tap and it plays" UX target and the IRQ pin (`pins::RFID_IRQ`) is reserved for a follow-up PR.
- [x] **microSD SPI bring-up, FAT32 mount**: `src/hw/sdcard.rs` wraps `embedded-sdmmc = 0.9` over an `ExclusiveDevice<Spi<…, Blocking>, Output, NoDelay>` with `esp_hal::delay::Delay` for the blocking `DelayNs` impl required by the SD init handshake. SPI3 on SCK 12 / MOSI 11 / MISO 13 / CS 10 at 400 kHz (SD-spec ≤ 400 kHz for init; phase 3 will re-clock via `SdCard::spi(\|spi\| …)` when cache throughput matters). Card-detect on GPIO 1 with internal pull-up (active-low when seated). `StubTimeSource` hard-codes 2000-01-01 to satisfy `embedded-sdmmc::TimeSource` until phase 4 wires an RTC. `SdCardDriver::new` eagerly drives `num_bytes` so a missing card surfaces immediately, then wraps the `SdCard` in a `VolumeManager`. `main` logs card size and probes `VolumeIdx(0)`. Driver lives in `SDCARD_CELL: StaticCell<SdCardDriver>` so the phase-3 cache task can borrow it without re-claiming SPI3 from the consumed `Peripherals`. **Bench verification (FAT32-formatted microSD inserted, real "size N MiB" + "volume 0 mounted" lines over UART) still pending.** **Workaround note:** `embedded-sdmmc 0.9.0::VolumeManager::device` has an upstream-API bug — the closure return is constrained to the `TimeSource` generic, making it unusable. We capture `card_size_bytes` at construction time and never call `device`; revisit when `embedded-sdmmc` 0.10 / 0.11 fixes the signature.
- [x] **I2S out to MAX98357A, hardcoded raw PCM**: `src/hw/i2s.rs` wraps `esp_hal`'s I2S0 in Philips standard / `Data16Channel16` / 44.1 kHz with circular GDMA (`peripherals.DMA_CH0`, 16 KiB TX buffer ≈ 93 ms of audio). `src/audio/playback.rs` defines `ToneSource`: a compile-time 256-entry Bhaskara-I sine table at ~25 000 amplitude (−2.3 dBFS) driven by a Q8.24 phase accumulator so 440 Hz at 44.1 kHz lands without floating-point math. `src/tasks/audio.rs` runs a 10 ms refill cadence against `DmaTransferTxCircular::available` / `push_with`; misconfigured DMA logs `warn` and the task exits cleanly. `main.rs` ties the MAX98357A `SD` pin (GPIO 3) high to enable the amp in "left channel only" mode. **The "from SD" half is deferred to phase 3** — the SD-to-MP3-to-I2S pipeline + `minimp3-sys` wiring belong with the cache task, not phase 1, and the parenthetical `or raw PCM` in this checkbox covers the current state. **Bench verification (actually hear the 440 Hz tone over a speaker) still pending.**
- [x] Decision: select MP3 decoder crate — **`minimp3-sys`** (FFI to the public-domain C library). Picked over Helix MP3 (more bindings work) and `puremp3`/`rinimp3` (immature, risk for the < 60 % CPU budget). Wired in when the audio task lands.

### Resolution of the 2026-05-22 link blocker

The previous baseline `cargo build` failed at link with two visible symptoms that we wrongly attributed to upstream packaging:

1. An avalanche of undefined references against `xtensa-lx-rt` (`__exception`, `__pre_init`, `_bss_end`, `_init_start`, …) and PAC peripheral aliases (`FROM_CPU_INTR0`, …).
2. Three `dangerous relocation: l32r: literal placed after use: .init.literal / .fini.literal` errors out of `crtbegin.o` / `crtend.o`.

The real root cause was the linker invocation, not the `esp-hal-embassy` / `esp-hal` version pair:

- **`esp-hal 1.0.0-beta.1` does not auto-inject `linkall.x`** (the umbrella linker script that pulls in `rom-functions.x`, `esp32s3-link.x`, the interrupt vector table and the `xtensa-lx-rt` symbols). The stable `1.0` line does, and the comment in `.cargo/config.toml` claiming "esp-hal 1.0+ injects linkall.x" was simply wrong for beta. Adding `-C link-arg=-Tlinkall.x` removed the avalanche.
- **`no_std` + `no_main` doesn't want libc's `crtbegin.o` / `crtend.o`** (they emit `.init` / `.fini` constructors that the xtensa assembler cannot relocate without `l32r` literals — hence the `dangerous relocation` triplet). `-nodefaultlibs` (set automatically by rustc) only removes libc; `-C link-arg=-nostartfiles` removes the crt startup objects too.
- The earlier `--no-gc-sections` workaround was misdiagnosed and has been removed: with the linker script in scope, default `--gc-sections` works fine; PAC PROVIDE aliases were never the issue.
- **`espflash save-image` additionally requires `esp_bootloader_esp_idf::esp_app_desc!()`** in `main.rs` — without it the ESP-IDF second-stage bootloader rejects the image. Added the crate (`esp-bootloader-esp-idf = "0.1"`) and the macro call right next to the module declarations.

Net change: two extra link args in `.cargo/config.toml`, one extra dep in `Cargo.toml`, one extra macro invocation in `main.rs`. No version bumps, no patches against crates.io. The next phase-1 tasks (LED, MFRC522, SD, I2S) can land on top of this baseline.

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
