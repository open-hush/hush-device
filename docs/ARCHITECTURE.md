# Architecture

A high-level view of how the firmware is organised. For pin-level detail see [`PIN_MAP.md`](./PIN_MAP.md); for phase-by-phase implementation status see [`../PLAN.md`](../PLAN.md).

## Table of contents

- [One paragraph](#one-paragraph)
- [Tasks and channels](#tasks-and-channels)
- [Memory budget](#memory-budget)
- [State machine](#state-machine)
- [Hardware abstraction](#hardware-abstraction)
- [Boot sequence](#boot-sequence)

---

## One paragraph

`main.rs` initialises `esp-hal`, sets up `esp-alloc` over the 8 MB PSRAM, brings up the embassy executor, and spawns seven tasks (`rfid`, `audio`, `cache`, `sync`, `input`, `power`, `led`). Tasks never share mutable state through globals; dependencies are arguments, and inter-task signalling uses two channel families: a broadcast `PubSubChannel<Event>` for high-level events (card scanned, button pressed, low battery, etc.) and per-pair `Channel<Command>` instances for direct commands (`audio` accepts `PlayAudio { audio_id }`, `cache` accepts `EnsureCached { audio_id }`, …).

## Tasks and channels

```
+--------+   CardScanned   +-------+   PlayAudio    +-------+
| rfid   | --------------> | audio | <------------- | input |
+--------+                 +-------+                +-------+
                              |                         |
                              | EnsureCached            | VolumeDelta
                              v                         |
                          +-------+                     v
                          | cache | <----------------- (LED + Power)
                          +-------+
                              |
                              | DownloadAudio
                              v
+--------+   SyncCompleted    +------+    HMACSigned    +-----+
| power  | <----------------- | sync | -------------->  | api |
+--------+                    +------+                  +-----+
   ^                             |
   | PowerTransition             | DeviceConfig
   |                             v
+--------+                  +---------+
| input  | ---------------> | storage |
+--------+   debounced       (NVS + outbox + cache index)
```

The above is shape, not a strict dependency graph. Detailed contracts live next to the channel definitions in [`src/proto/events.rs`](../src/proto/events.rs).

## Memory budget

The ESP32-S3 internal SRAM is ~512 KB and is *shared* with WiFi, BLE, and the embassy executor. Everything chunky goes to PSRAM.

| Region | Used by | Approx |
|---|---|---|
| Internal SRAM | embassy task stacks (× 7), WiFi/BT control structures, log buffers | ~250 KB |
| PSRAM | audio decode workspace, TLS workspace, sync JSON parse buffer, cache index in RAM | ~200 KB |
| Internal flash (NVS partitions) | secrets, WiFi creds, outbox, cache index | < 64 KB working set |
| microSD | audio cache, daily logs | grows |

Task stacks are dimensioned per-task and documented in [`src/tasks/*.rs`](../src/tasks/). The default of 8 KB is **wrong** for almost every task; measure with `cargo size`.

## State machine

```
                          card_scanned (known)
       ┌─────────────────────────────────────────────────┐
       │                                                 │
       v                                                 │
  IDLE ─── card_scanned (unknown) ──> NOTIFY_UNKNOWN ────┘
   │                                                     │
   │ encoder_press                                       │
   v                                                     │
  PLAYING ─── card_scanned (different) ──> FADE_SWAP ────┘
   │
   │ encoder_press OR card_removed
   v
  IDLE
```

This is the user-visible playback state. Each transition publishes an `Event` so the LED and power tasks can react.

## Hardware abstraction

Everything in [`src/hw/`](../src/hw/) is intended to compile under both the real HAL (`xtensa-esp32s3-none-elf`) and a host target with the `mock-hardware` feature. Tasks consume traits (not concrete types) so the host-side substitutes can stand in for unit tests.

> TODO(phase-1): finalise the trait surface. Candidates: `Led`, `RfidReader`, `SdCard`, `I2sOut`, `Encoder`.

## Boot sequence

1. Reset vector → `esp-hal` HAL init.
2. PSRAM allocator init via `esp_alloc::psram_allocator!`.
3. UART logger init (`esp-println`).
4. Read NVS:
   - If `device_secret` missing → panic with a clear message ("device not provisioned").
   - If `wifi_creds` missing → start BLE pairing task and skip WiFi bring-up.
   - Else load `last_config` into runtime state.
5. WiFi STA bring-up (if creds present).
6. Spawn all tasks.
7. Run executor.

> TODO(phase-1): document the timing budget. Target boot-to-LED-green: < 1.5 s.
