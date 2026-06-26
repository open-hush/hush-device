# ADR 0001 — BLE GATT host stack for Improv Wi-Fi (Phase 5)

- **Status**: Proposed — pending bench validation.
- **Date**: 2026-06-26
- **Context**: hush-device Phase 5 (OPE-54), BLE Improv Wi-Fi GATT server.

## Context

Phase 5 needs a BLE GATT server exposing the Improv Wi-Fi service so the
mobile app can hand the device its Wi-Fi credentials without a cable.

The radio bring-up is settled and verified to compile for the
`xtensa-esp32s3-none-elf` target:

- `esp-wifi 0.14`'s `BleConnector` (behind the `ble` feature) already
  implements `bt_hci::transport::Transport`.
- `bt-hci 0.3.2`'s `ExternalController` lifts that transport into the
  `Controller` trait a GATT host stack consumes.

This is wired in `src/hw/ble.rs::ble_controller`. What is **not** yet
decided is which GATT **host stack** runs on top of that controller to
serve the attribute table (service + 5 Improv characteristics), handle ATT
reads/writes, and push notifications.

## The decision that needs making

Pick the GATT host stack that backs `crate::hw::ble::ImprovGatt`:

| Option | Pros | Cons |
|---|---|---|
| **`trouble-host`** | Actively maintained, `no_std`/async-first, designed around `bt-hci` `Controller`, the direction esp-rs examples are moving. | API still pre-1.0 and churning; `gatt_server!` macro ergonomics; version must track `bt-hci 0.3`. |
| **`bleps`** | Used in older esp-rs examples for this exact `esp-wifi` generation. | Historically git-only (no stable crates.io line), effectively unmaintained vs trouble; would be a dead end. |

## Recommendation

**`trouble-host`**, pinned to the release that depends on `bt-hci 0.3`
(same line `esp-wifi 0.14.0` pulls), implementing the `ImprovGatt` trait.

Rationale: it is the maintained, `bt-hci`-native option and avoids adopting
an unmaintained git dependency (`bleps`) we would have to migrate off later
— exactly the kind of accidental debt this project avoids.

## Why this is deferred, not decided in this PR

1. **It cannot be runtime-validated without hardware.** No ESP32-S3 on the
   bench in this environment, and the host toolchain has no Xtensa linker,
   so the GATT server's behaviour (advertising, ATT MTU, notification
   delivery) is unobservable here. Committing ~300 lines of stack-specific
   GATT code that compiles but is unproven would be false confidence.
2. **The trait seam makes the choice cheap to land later.** Everything that
   depends on the GATT server — the Improv protocol core
   (`src/proto/improv.rs`, fully host-tested) and the pairing orchestration
   (`src/tasks/ble.rs::run_pairing`, type-checked for the target) — talks to
   the `ImprovGatt` trait, not a concrete stack. The host stack is a single
   swappable implementation.

## Consequences / follow-up (bench task)

1. Add `trouble-host` (matching `bt-hci 0.3`) under the `ble-improv` feature.
2. Implement `ImprovGatt` for a `trouble-host`-backed server: advertise the
   Improv service UUID, expose the 5 characteristics from
   `crate::proto::improv`, map writes → `next_rpc`, and notifications →
   `notify_state` / `notify_error` / `notify_result`.
3. Implement `WifiProvisioner` (`src/tasks/ble.rs`): join via the existing
   `WifiController`, persist creds with `NvsStore::set_wifi_credentials`,
   trigger `POST /v1/device/register`, free the BLE stack on success.
4. Spawn the pairing task from `main.rs` on first boot (no creds in NVS).
5. Validate end to end on hardware: advertise, provision from `hush-app`,
   confirm join + register + teardown (~30 KiB SRAM reclaimed), and the
   four LED indications.
