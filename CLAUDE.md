# `hush-device` — Claude Code operating context

You are working on the **Hush firmware**: Rust `no_std` on a Seeed Studio XIAO ESP32-S3, with [embassy](https://embassy.dev/) on top of `esp-hal`.

## Hardware target — fixed

- MCU: **XIAO ESP32-S3**, 8 MB PSRAM, USB-C native.
- Target triple: `xtensa-esp32s3-none-elf` (Tensilica Xtensa LX7, not RISC-V).
- The pin map lives in `src/hw/pins.rs`. **That file is the truth.** If you find a magic GPIO number anywhere else, that's a bug.

## Toolchain — mandatory before touching code

Stable `rustc` does **not** support the Xtensa target. You need the Espressif fork:

```bash
cargo install espup espflash
espup install
source $HOME/export-esp.sh   # every shell — or add to ~/.zshrc
```

If `espup` is unavailable (CI runner, restricted machine), build and test on the host with `--features mock-hardware` only. Never silently switch to a different target.

## Common commands

```bash
# Build (default target picked from .cargo/config.toml)
cargo build --release

# Flash + monitor (requires the device plugged in over USB-C)
cargo run --release

# Just lint
cargo clippy --target xtensa-esp32s3-none-elf -- -D warnings

# Host-side unit tests
cargo test --features mock-hardware --target x86_64-apple-darwin

# Erase flash (factory reset, brutal)
espflash erase-flash
```

The `cargo run` runner is configured in `.cargo/config.toml` (uses `espflash flash --monitor`).

## Conventions — non-negotiable

1. **`#![no_std]` strict.** Anything pulling `std` is a regression.
2. **`alloc` is justified, never implicit.** When you allocate, allocate from PSRAM (via `esp-alloc`). Document why. Audio buffers, JSON parsing into a heapless target, and TLS workspace are the legitimate cases. Almost nothing else is.
3. **No `unwrap`, `expect`, or `panic!` in hot paths.** "Hot paths" = audio task, RFID task, sync task. Errors propagate with `?` or degrade gracefully (LED red blink + log line).
4. **Embassy tasks declare their stack size.** Inline as a comment why that number was chosen. Default is wrong; measure with `cargo size` once the task is real.
5. **Critical sections are short and contain no `.await`.** Use `critical_section::with` only for genuinely atomic register touches.
6. **Large buffers go to PSRAM**, never to internal SRAM (which is ~512 KB total and we share it with WiFi + BLE):
   ```rust
   #[link_section = ".ext_ram.bss"]
   static mut BUF: [u8; 64 * 1024] = [0; 64 * 1024];
   ```
7. **Hardware traits live in `src/hw/`.** Tasks depend on traits, not on concrete HAL types, so the `mock-hardware` feature can swap implementations for host tests.

## Architecture in one paragraph

`main.rs` initialises the HAL, sets up `esp-alloc` for PSRAM, brings up the embassy executor, then spawns one task per concern: `rfid`, `audio`, `cache`, `sync`, `input`, `power`, `led`. Tasks communicate via `embassy_sync::channel::Channel` (point-to-point commands) and `embassy_sync::pubsub::PubSubChannel` (broadcast events — `Event` enum in `src/proto/events.rs`). No globals; every task receives its dependencies as arguments.

## Source of truth for state

| State | Where |
|---|---|
| Per-device HMAC secret | NVS, partition `nvs` |
| WiFi credentials | NVS, partition `nvs` |
| Last `DeviceConfig` (sleep timers, volume cap, …) | NVS, partition `storage` |
| Event outbox (unflushed scans, button presses, errors) | NVS, partition `storage` |
| Audio cache | microSD `/cache/<audioId>.mp3` + `/cache/index.bin` |
| Logs | microSD `/logs/YYYY-MM-DD.log` (rotated) |

**The microSD is only a cache.** Anything that must survive a card swap or a factory-SD-reformat lives in NVS.

## Phasing — do not jump ahead

The roadmap in `PLAN.md` has six phases. Finish phase N before opening phase N+1. If you notice something belonging to a later phase, write it as a `// TODO(phase-X):` and add it to that phase's checklist in `PLAN.md`. Do not implement it now.

## Open decisions — ask, do not invent

- MP3 decoder crate (`PLAN.md`).
- Battery low-voltage cutoff threshold.
- Bundled TLS root certificates.

If you encounter one of these during work, surface it to the user. Do not pick silently.

## Where things live

| Subject | File |
|---|---|
| Entry point + task spawn | `src/main.rs` |
| Pin map | `src/hw/pins.rs` |
| HAL bring-up helpers | `src/hw/{i2s,mfrc522,sdcard,led}.rs` |
| Task implementations | `src/tasks/*.rs` |
| HTTP client + HMAC | `src/api/{client,hmac}.rs` |
| Wire types (matches OpenAPI) | `src/proto/api.rs` |
| Inter-task events | `src/proto/events.rs` |
| NVS / outbox / cache index | `src/storage/*.rs` |
| MP3 decode + I2S playback | `src/audio/*.rs` |
| Architecture notes | `docs/ARCHITECTURE.md` |
| Toolchain setup | `docs/DEVELOPMENT.md` |
| Pin map (tabular) | `docs/PIN_MAP.md` |
| OTA scheme | `docs/OTA.md` |
| Debugging | `docs/DEBUGGING.md` |
