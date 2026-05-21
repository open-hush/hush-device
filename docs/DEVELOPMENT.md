# Development

## Prerequisites

- macOS, Linux (x86_64 or aarch64), or WSL. Bare Windows is not officially supported (`espflash` works but the dev experience suffers).
- A USB-C cable that actually carries data (a surprising number do not).
- A XIAO ESP32-S3 plugged in via USB-C. The first time you connect it, hold the BOOT button while plugging it in to force flash mode.

## Toolchain

The Xtensa target is not in mainline `rustc`. Install Espressif's toolchain:

```bash
cargo install espup espflash
espup install
source $HOME/export-esp.sh        # add this to ~/.zshrc or ~/.bashrc
```

If you skip `source export-esp.sh`, `cargo build` will fail with "linker `xtensa-esp32s3-elf-gcc` not found" or similar.

Verify:

```bash
rustc --version --verbose
# host should be "x86_64-apple-darwin" or similar
# but the `esp` toolchain is what's active in this directory

cargo check --target xtensa-esp32s3-none-elf
```

## Without `espup`

On restricted machines (some CI runners, locked-down corp laptops) you can:

- Skip the Xtensa target entirely and run **host-side tests only** with the `mock-hardware` feature. You can still iterate on pure-logic modules (HMAC canonicalization, NVS encoding, decoder framing) this way.
- Use the [`esp-rs/rust-build`](https://github.com/esp-rs/rust-build) prebuilt binaries if your platform is supported.

## Building

```bash
cargo build --release
# Output: target/xtensa-esp32s3-none-elf/release/hush-device (ELF)
```

## Flashing

```bash
cargo run --release
# Internally: espflash flash --monitor --partition-table partitions.csv
```

If the runner does not pick up your device automatically, list ports:

```bash
espflash board-info
```

and pass `--port /dev/cu.usbmodem<...>` to `espflash` directly.

## Monitoring an already-flashed device

```bash
espflash monitor
# Quit with Ctrl-C
```

## Erasing

```bash
espflash erase-flash
# Wipes everything: secret, WiFi creds, OTA partitions. Device must be
# re-provisioned afterwards.
```

## Host-side tests

```bash
cargo test --features mock-hardware --target x86_64-apple-darwin
# Adjust target to your host. Linux: x86_64-unknown-linux-gnu.
```

## Linting

```bash
cargo clippy --target xtensa-esp32s3-none-elf --release -- -D warnings
cargo fmt
```

## Common failure modes

| Symptom | Cause | Fix |
|---|---|---|
| `linker not found` | `source $HOME/export-esp.sh` not run | Source it |
| `failed to open serial port` | macOS hasn't loaded the USB driver yet | Replug cable, wait 2 s |
| `Permission denied` on `/dev/cu.usbmodem...` (Linux) | User not in `dialout` group | `sudo usermod -aG dialout $USER`, log out and back in |
| Device boots into bootloader and won't run | Bad image | `espflash erase-flash`, reflash |
| Random crashes / brownouts | USB cable can't supply enough current | Use a powered hub or a known-good USB-C cable |
