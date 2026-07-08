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

> **Reconciled 2026-07-07 with the real XIAO ESP32-S3.** The board exposes only
> **11 GPIOs** (`1,2,3,4,5,6,7,8,9,43,44`). The previous map assigned GPIO
> `10-13,17,18,21,33-37`, none of which exist on this board — and `33-37` are
> the module's **octal PSRAM** lines (driving them corrupts PSRAM). RFID and
> microSD now **share one SPI bus**; the encoder, buttons and RGB LED have no
> pins left (see Decisions open).

```
I2S audio (MAX98357A):
  BCLK GPIO 5 (D4), LRC GPIO 6 (D5), DIN GPIO 4 (D3), SD/mute GPIO 3 (D2)

Shared SPI bus (RFID + microSD):
  SCK GPIO 7 (D8), MISO GPIO 8 (D9), MOSI GPIO 9 (D10)

RFID (MFRC522):   CS GPIO 44 (D7)   [RST: soft-reset over SPI; IRQ: polling]
microSD:          CS GPIO 43 (D6)   [card-detect: probed at mount]

Status LED:       WS2812 DIN GPIO 2 (D1)   [single NeoPixel, RMT-driven]
Button:           BTN_MAIN GPIO 1 (D0)     [multifunction, RTC DEEP_SLEEP wake]

Dropped in v1: KY-040 encoder, 2nd button. See "Decisions open".
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

> ⚠️ **Hardware reconciliation done in code 2026-07-07 — NOT yet compiled/bench-verified.**
> The bring-up was originally written against a pin map that does not fit the
> real XIAO ESP32-S3. Now reconciled to the v1 layout across `hw/pins.rs`,
> `main.rs`, `hw/led.rs`, `hw/mfrc522.rs`, `hw/sdcard.rs` and the docs:
>
> - **LED**: rewritten from 3-GPIO LEDC (GPIO 35/36/37 = octal-PSRAM, would
>   corrupt PSRAM) to a single **WS2812** on GPIO 2 over RMT. The `RgbLed`
>   trait seam is unchanged so `tasks/led.rs` is untouched.
> - **SPI**: two buses (`SPI2` + `SPI3`) collapsed to **one shared bus**; RFID
>   and microSD each get a `CriticalSectionDevice` handle on it.
> - **RFID RST / microSD card-detect dropped** (no pins): soft-reset over SPI,
>   presence inferred at mount.
> - **`main.rs`** now ties each hardcoded `peripherals.GPIOn` to `hw::pins` via
>   `const _: () = assert!(...)` drift guards.
>
> **`cargo check --target xtensa-esp32s3-none-elf` is GREEN (2026-07-08).**
> `esp-hal-smartled 0.15.0` links cleanly against `esp-hal 1.0.0-beta.1` (no
> git-rev pinning needed); `SmartLedsAdapter::new` takes the RMT
> `ChannelCreator` (not `Channel`); `CriticalSectionDevice::new` returns
> `Result<_, Infallible>` and is `.expect`-ed. This proves it links + typechecks
> — **not** that it works on hardware.
>
> **Remaining before the `[x]` items can be checked (bench, hardware in hand):**
> - Eye-check each bring-up on the real XIAO: WS2812 lights solid green; a
>   MIFARE card publishes a UID over USB-CDC; a FAT32 microSD mounts; the
>   440 Hz tone is audible through the MAX98357A + 8 Ω speaker.
> - **Amp supply** (`3V3` vs 5 V boost) to confirm on bench.

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

- [x] **HMAC-SHA256 signing module (host-tested)**: `src/api/hmac.rs` is now a self-contained `no_std` module exposing `body_sha256_hex`, `canonical_request`, `sign`, `signature_hex` and `authorization_header_value`. Canonical request is built into a `heapless::String<320>` (method uppercased, single-LF separators, no trailing newline, lowercase hex SHA-256 of the body); the Authorization header value uses the exact `HMAC keyId=…,signature=…,ts=…` format from `hush-protocol/docs/auth.md`. **Cross-target build setup**: pure-logic deps (`hmac`, `sha2`, `heapless`, `serde`, …) sit in `[dependencies]`; every Xtensa-only dep (`esp-hal`, `esp-wifi`, `embassy-*`, `mfrc522`, `embedded-sdmmc`, …) moved to `[target.'cfg(target_arch = "xtensa")'.dependencies]` so the host triple no longer pulls esp-hal's build script (which `panic!`s on any non-Xtensa target). A new `[lib] name = "hush_device", path = "src/lib.rs"` re-exports `hmac` via `#[path = "api/hmac.rs"] pub mod hmac;` so the firmware `bin` keeps owning the file (single source of truth) while host tests get a slim entry point. **Host-test runner workflow**: the device build needs `[unstable] build-std = ["alloc", "core"]` in `.cargo/config.toml` to apply `panic=immediate-abort` to `core`; running tests through the esp toolchain reuses that and trips `E0152 duplicate lang item: sized` (build-std's rebuilt `core` collides with the rustup-provided one that `std` links). Workaround documented in `tests/README.md`: run `cargo +stable test --features mock-hardware --target aarch64-apple-darwin --lib` — stable ignores the unstable table and the lib is `no_std`-by-default with `#[cfg(test)] extern crate std;` only inside the test harness. **Coverage**: 10/10 passing, including the auth.md worked example (`POST /v1/device/register` + `{"serial":"ABC"}` → canonical string byte-for-byte), RFC 4231 vector 1 for the HMAC primitive, empty-body SHA-256 constant, method-uppercasing, LF-only / no-trailing-newline invariants, `PathTooLong` overflow, signature determinism and Authorization-header rendering. **Xtensa release image: 492 KiB (unchanged vs. WiFi STA baseline)** — the crypto add-on is mostly inlined into `Mac::update`.
- [x] **Wire types mirroring `hush-protocol/hush-api.yaml` (host-tested)**: `src/proto/api.rs` now defines the full Phase 2 contract surface — `DeviceRegisterRequest`/`DeviceRegisterResponse`, `Device`/`DeviceState`, `DeviceSyncResponse`, `DeviceConfig`, `CardBinding`, `AudioSyncEntry`, `ApiError`, `DeviceEventsRequest` and the nine-variant `DeviceEvent` union — all `no_std`/no-alloc (`heapless::String`/`Vec` with documented capacities, `serde-json-core`). `DeviceEvent` carries a hand-written `Serialize` (json-core supports neither `#[serde(flatten)]` nor adjacently-tagged enums) that emits the `{eventId, ts, type, payload}` shape per the spec's discriminator. Tests pin the serialized/deserialized JSON byte-for-byte against the spec's worked examples (camelCase, optional-field omission, empty arrays, negative RSSI, claim code).
- [x] **HTTPS client request layer (host-tested) + `reqwless`/`embedded-tls` transport (behind `phase2-io`)**: `src/api/client.rs` splits into (a) pure, host-tested helpers — `sign_request` (body SHA-256 → canonical request → HMAC → `Authorization` header, agrees with the auth.md worked example end-to-end), `build_sync_path` (RFC 3986 percent-encoding of the `since` query value — the first device endpoint with a query param), `format_uuid` (16 bytes → hyphenated lowercase `keyId`), and `HttpOutcome::from_status` (200/202/304/401/404/422/429 → typed outcome + retriability); and (b) the `reqwless` + `embedded-tls` transport (`DeviceClient`) gated behind `target_arch = "xtensa"` + the `phase2-io` feature. **Transport bench-pending**: exact `reqwless` 0.13 builder calls and `TlsVerify::Cert` against the embedded ISRG Root X1 anchor need confirming on the bench (see below).
- [x] **`POST /v1/device/register` on first boot + `deviceId` persistence** — `tasks::sync::ensure_registered` (behind `phase2-io`): registers when NVS has no `deviceId`, parses the returned UUID and persists it. Pure request build + UUID parse are exercised through the host-tested helpers; the end-to-end call is bench-pending.
- [x] **`GET /v1/device/sync` loop with `since` + `304`** — `tasks::sync::run_sync` (behind `phase2-io`): polls every `syncIntervalSec` (floored at 60 s), sends the `since` of the last server time, applies + persists the new `DeviceConfig` on `200`, keeps the cache on `304`. Bench-pending end to end.
- [x] **`POST /v1/device/events` flush (idempotent)** — `tasks::sync::flush_events` + `src/storage/outbox.rs`: the outbox is a host-tested bounded, drop-oldest ring buffer; a batch is copied with `batch()` and removed with `ack()` only after `202`, so a failed flush re-sends the same `eventId`s (backend dedups). Bench-pending end to end.
- [x] **NVS persistence (host-tested codec) via `sequential-storage`** — `src/storage/nvs.rs`: the on-flash byte codec for `device_secret` (32 B), `device_id` (16 B), WiFi SSID/pass and `last_config` (JSON) is pure and round-trip host-tested (including length/UTF-8/overflow rejection). The `esp-storage`-backed `NvsStore` (open `nvs` / `storage` partitions, fetch/store) is gated behind `phase2-io` and bench-pending.
- [x] **TLS trust anchor** — `src/certs.rs` embeds **ISRG Root X1** in DER (`certs/isrg_root_x1.der`, 1391 B, SHA-256 verified against the published fingerprint). Decision recorded: bundle ISRG Root X1 only.

**Remaining for Phase 2 done (bench session, `--features phase2-io`):**

- Pin the `phase2-io` crate versions for the `esp-hal 1.0.0-beta.1` generation (`embassy-net 0.6`, `reqwless 0.13`, `esp-storage 0.5`, `sequential-storage 4`, `embedded-storage 0.3`) and get `cargo build --features phase2-io` green; resolve the documented finicky-version risk the same way the esp-wifi pin was.
- Confirm `reqwless`'s TLS verification API supports a CA trust anchor (`TlsVerify::Cert`) against ISRG Root X1; if `embedded-tls` cannot verify the chain, escalate as an open decision (it changes the "certs embebidos" acceptance criterion).
- `main.rs` wiring: build the `embassy_net::Stack` from the STA `WifiDevice` (currently dropped in Phase 1), spawn the net task + DHCPv4 + DNS, allocate the TLS/RX buffers in PSRAM, and spawn `run_sync`.
- NTP / SNTP clock: HMAC `ts` must be real unix-seconds within ±300 s; wire the `UnixClock` the sync task takes to an SNTP-synced source and refuse to sign before first sync.
- Bench e2e: register against the real backend, observe `claimCode`, confirm periodic sync + `304`, and a card-scan event landing in the dashboard. Then bump the checkboxes above from "host-tested" to "bench-verified".

## Phase 3 — Audio cache on SD (~2 weeks)

Acceptance: tapping a known card always plays within < 500 ms when cached; cache miss triggers download.

- [ ] `embedded-sdmmc` mount and basic file I/O.
- [ ] Streaming download from presigned URL into SD with atomic write + fsync.
- [ ] SHA-256 verification on every read; mismatch → evict.
- [ ] LRU eviction when free space < 10 %.
- [ ] Cache index in `/cache/index.bin` updated on every change.

## Phase 4 — Input & power (~1-2 weeks)

Acceptance: 24 hours of mixed use on a single charge without crash; idle drain matches spec.

- [ ] `BTN_MAIN` (GPIO 1) handler: short = play/pause, long = pairing, extra-long = factory reset. (v1 has **no rotary encoder** — volume moves to the app / long-press; adding the KY-040 is a board-change, see Decisions.)
- [ ] State machine: `ACTIVE → LIGHT_SLEEP → DEEP_SLEEP` with `BTN_MAIN` as the GPIO wake source (RTC-capable). Drive `I2S_SD` low before DEEP_SLEEP to kill the amp's ~3 mA idle draw.
- [ ] Validate idle power on bench (< 1 mA LIGHT, < 20 µA DEEP).

## Phase 5 — BLE Improv WiFi (~2 weeks)

Acceptance: user sets up the device from the mobile app with no cables and no edited files.

Improv protocol core + provisioning state machine are done and host-tested
(`src/proto/improv.rs`, `cargo test --features mock-hardware`). The BLE radio
bring-up (`src/hw/ble.rs::ble_controller`) and pairing orchestration
(`src/tasks/ble.rs::run_pairing`) compile for the target behind the
`ble-improv` feature. The concrete GATT host-stack implementation is the only
bench-pending piece — see `docs/adr/0001-ble-host-stack.md`.

- [x] Improv RPC framing + checksums + state machine (pure, host-tested).
- [x] Improv service / characteristic UUIDs (match the published standard).
- [x] BLE HCI controller bring-up (esp-wifi `BleConnector` → `bt-hci` `ExternalController`).
- [x] Pairing task orchestration driving the state machine + LED indications.
- [ ] Concrete `ImprovGatt` GATT server (pick host stack — ADR 0001). **(bench)**
- [ ] Concrete `WifiProvisioner` (join + persist creds + register). **(bench)**
- [ ] Spawn pairing task from `main.rs` on first boot. **(bench)**
- [ ] On-hardware validation: advertise → provision → join → register → teardown. **(bench)**

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

## Decisions taken (hardware, 2026-07-07)

- **Input + LED (v1)**: RESOLVED. Drop the KY-040 rotary encoder; single **WS2812/NeoPixel** LED on GPIO 2 (its controller does the PWM); **one multifunction button** on GPIO 1 (RTC-capable, DEEP_SLEEP wake); keep amp mute on GPIO 3 for standby current; drop MFRC522 hardware RST (soft-reset over SPI) and microSD card-detect. This fits the 11 pads with no expander. Adding the encoder / a 2nd button later is a **board-change** trigger, not a quick add (an I2C expander needs 2 free pins we lack, can't PWM the LED, and clocks a fast encoder unreliably).

## Decisions open

- **WS2812 driver crate**: `smart-leds` + `ws2812-spi`, or drive the WS2812 directly over the ESP32-S3 **RMT** peripheral (`esp-hal-smartled` / `esp-hal` RMT)? RMT is the idiomatic esp-hal path and frees SPI. Pin the crate before rewriting `hw/led.rs`.
- **Amp supply**: the XIAO `5V` pad is only live over USB, not on battery. Power the MAX98357A from `3V3` (~0.5–1 W into the 8 Ω speaker, no extra parts) or add a 5 V boost (more headroom, more complexity)? Default `3V3` unless bench volume is insufficient.
- **Battery low-voltage cutoff**: cut at 3.4 V or 3.3 V? Sparkfun PCM cuts at 2.5 V (too low). We probably want to refuse to start below 3.4 V to protect the cells. Confirm on bench.
- **SD card spec**: 8 GB or 16 GB high-endurance as recommended default? Document the part number families known to work.
- **TLS root certs**: bundle ISRG Root X1 only (Let's Encrypt), or DigiCert + Amazon Root too? Smaller = faster boot. Default ISRG only unless we hit issues.
- **BLE GATT host stack** (Phase 5): `trouble-host` vs `bleps` on top of the `bt-hci` controller. Recommendation + rationale in `docs/adr/0001-ble-host-stack.md`; confirm on bench.

---

## Cross-repo touch points

- `hush-protocol/hush-api.yaml` — wire types in `src/proto/api.rs` and `src/proto/events.rs` must match. Drift is caught by CI (TODO: add `oapi-codegen` style check).
- `hush-backend` — `POST /v1/device/register` must return `claimCode` for the user dashboard.
- `hush-app` — BLE Improv WiFi pairing flow (phase 5) must agree on service UUIDs and characteristic shapes. UUIDs + RPC byte layout are now pinned in `src/proto/improv.rs` (the published Improv standard); `hush-app/lib/ble/improv.ts` consumes the same.

---

## Out of scope (forever)

- DRM. The device can play any locally cached MP3 the user provides.
- Voice recognition / wake words.
- Streaming from arbitrary HTTP servers — only via the Hush backend's presigned URLs (security boundary).
- Multi-room audio sync.
