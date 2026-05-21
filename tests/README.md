# Tests

Two flavours of tests:

## Host-side unit tests

Pure-logic modules (HMAC canonicalization, JSON shapes, state machines, byte parsing) live behind `cfg(feature = "mock-hardware")` and can be tested on the host:

```bash
cargo test --features mock-hardware --target x86_64-apple-darwin
```

Place these as inline `#[cfg(test)] mod tests` blocks inside the module being tested, **not** here. This directory is reserved for integration tests that need a wider surface.

## On-device integration tests

Embassy/esp-hal don't currently have a great "test on hardware" story. For phase 1 we treat the bring-up bench session as the test suite:

- Plug in a fresh XIAO ESP32-S3.
- Flash the firmware.
- Verify by hand: LED comes up, WiFi connects, RFID reads a card, microSD mounts, I2S plays a tone.

Phase 4+ will add a small `xtask` runner that drives the device via UART for repeatable bring-up tests.

> TODO(phase-4): set up `xtask` runner.
