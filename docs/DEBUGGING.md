# Debugging

## Logs

Default log level is `info`. Bump with:

```bash
ESP_LOG=debug cargo run --release
# Or: trace, info, warn, error
```

Logs are emitted via `esp-println` over the UART connected to the USB-C port.

## Backtraces

`esp-backtrace` is configured with `panic-handler` + `exception-handler` features. On a panic you should see:

```
!! A panic occured in '<file>:<line>':
   <message>
backtrace:
  0x...
  0x...
```

Decode the addresses with:

```bash
addr2line -e target/xtensa-esp32s3-none-elf/release/hush-device 0x...
```

## Memory leaks (PSRAM)

PSRAM allocations are tracked by `esp-alloc`. To dump current usage at runtime:

```rust
let stats = esp_alloc::HEAP.stats();
log::info!("psram free = {}, used = {}", stats.free, stats.used);
```

> TODO(phase-1): periodic dump from the `power` task every 60 s when in ACTIVE.

## I2S audio glitches

Most common causes:

1. **DMA underrun**: the decode loop didn't refill the I2S buffer in time. Increase the buffer or check if a higher-priority task is hogging the CPU.
2. **Clock mismatch**: BCLK ≠ sample_rate × bits × channels. Recheck I2S config.
3. **SD card too slow**: a card class lower than U1 / V10 can't sustain even 128 kbps reads. Use a U3 or A1 card.

## RFID misreads

- Check the antenna ground plane: the MFRC522 is sensitive to nearby metal.
- Use a card-class card (not a sticker tag) for development — sticker tags have worse range.
- The MFRC522 `RST` line must be high before SPI ops; verify with a scope if reads are intermittent.

## WiFi never connects

1. SSID and password are stored in NVS, written via BLE pairing. Dump them in dev with `espflash read-flash 0x9000 0x6000 nvs.bin` and inspect.
2. The ESP32-S3 supports 2.4 GHz **only**. A 5 GHz network looks broken from this perspective.
3. WPA3-only networks may fail with older `esp-wifi`. Pin to WPA2 + WPA3 mixed mode in your router.

## TLS handshake fails

The bundled root CA store is small. If you're hitting a backend with a cert not chained to ISRG Root X1, the handshake fails silently. Check the backend cert chain with:

```bash
openssl s_client -connect api.open-hush.com:443 -showcerts
```

## Useful tools

- [`espflash`](https://github.com/esp-rs/espflash) — flash + monitor.
- [`probe-rs`](https://probe.rs/) — JTAG debugging (requires a JTAG adapter, not the USB serial).
- [`cargo-binutils`](https://github.com/rust-embedded/cargo-binutils) — `cargo size`, `cargo nm`, `cargo objdump`.
- [`bloaty`](https://github.com/google/bloaty) — figure out what's eating binary size.
