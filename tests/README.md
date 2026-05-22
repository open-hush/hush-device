# Tests

Two flavours of tests:

## Host-side unit tests

Pure-logic modules (HMAC canonicalization, JSON shapes, state machines, byte parsing) live behind `cfg(feature = "mock-hardware")` and can be tested on the host. **Run them with the stable toolchain** (not the esp one):

```bash
# Apple Silicon
cargo +stable test --features mock-hardware --target aarch64-apple-darwin --lib

# x86_64
cargo +stable test --features mock-hardware --target x86_64-apple-darwin --lib
```

### Why `+stable` and `--lib`

- The Xtensa firmware build needs `[unstable] build-std = ["alloc", "core"]` in `.cargo/config.toml` (to apply `panic=immediate-abort` to `core`). Running tests through the esp toolchain reuses that and trips `E0152 duplicate lang item: sized` — build-std rebuilds `core` from source, and the host test harness *also* pulls in the rustup-provided `core` via `std`, ending up with two copies. The stable toolchain ignores `[unstable]` entirely, so only one `core` ends up in the binary.
- `--lib` targets the host-testable surface declared in `src/lib.rs` (a `#[path]` re-export of `src/api/hmac.rs` etc.). The bin (`src/main.rs`) is `#![no_main]` + Xtensa-only and cannot link a host test harness — we keep it out with `test = false` in `Cargo.toml`.

Place tests as inline `#[cfg(test)] mod tests` blocks inside the module being tested (e.g. `src/api/hmac.rs`), **not** here. This directory is reserved for integration tests that need a wider surface.

## On-device integration tests

Embassy/esp-hal don't currently have a great "test on hardware" story. For phase 1 we treat the bring-up bench session as the test suite:

- Plug in a fresh XIAO ESP32-S3.
- Flash the firmware.
- Verify by hand: LED comes up, WiFi connects, RFID reads a card, microSD mounts, I2S plays a tone.

Phase 4+ will add a small `xtask` runner that drives the device via UART for repeatable bring-up tests.

> TODO(phase-4): set up `xtask` runner.
