//! Host-testable surface of the firmware.
//!
//! The firmware itself ships from `src/main.rs` as a `#![no_main]`
//! Xtensa binary; this library exists **only** so pure-logic modules
//! (HMAC canonicalization, JSON wire shapes, state machines) can be
//! exercised with `cargo test --features mock-hardware --target
//! x86_64-apple-darwin`. It is never linked into the device binary.
//!
//! Modules listed here must compile against host `std` (under
//! `cfg(test)`) and on the target with `#![no_std]` — which means **no
//! references to `crate::error`, `crate::hw`, `crate::config` or any
//! other firmware-only symbol**. Pull them in via `#[path]` from their
//! real location so the bin keeps owning the canonical source, and the
//! lib stays a slim "extracted for tests" view.

#![no_std]

#[cfg(test)]
extern crate std;

/// HMAC-SHA256 device-request signing. Canonical form: see
/// `hush-protocol/docs/auth.md`.
///
/// The modules below mirror the firmware's own `crate::api` / `crate::proto`
/// / `crate::storage` tree via `#[path]` so that any `crate::…` reference
/// inside a re-exported file resolves to the **same** path in both the bin
/// (Xtensa) and this host-test lib. Each file keeps the firmware's
/// Xtensa-only I/O behind `#[cfg(target_arch = "xtensa")]`, so on the host
/// only its pure, allocation-free logic compiles in.
pub mod api {
    #[path = "hmac.rs"]
    pub mod hmac;

    /// Pure request-building helpers (path+query canonicalization, signed
    /// header assembly, HTTP status → outcome mapping). The `reqwless`
    /// transport itself is Xtensa-gated inside the file.
    #[path = "client.rs"]
    pub mod client;
}

pub mod proto {
    /// Wire types mirroring `hush-protocol/hush-api.yaml`.
    #[path = "api.rs"]
    pub mod api;

    /// Improv Wi-Fi BLE protocol core (RPC framing + provisioning state
    /// machine). Pure logic, host-tested; the BLE radio / GATT bring-up
    /// that drives it is Xtensa-only and lives in `crate::tasks::ble`.
    #[path = "improv.rs"]
    pub mod improv;
}

pub mod storage {
    /// Typed NVS records + their pure byte codec (the `esp-storage` backend
    /// is Xtensa-gated inside the file).
    #[path = "nvs.rs"]
    pub mod nvs;

    /// Append-only, drop-oldest event outbox ring buffer.
    #[path = "outbox.rs"]
    pub mod outbox;
}

/// Embedded TLS trust anchors (ISRG Root X1). Pure data.
pub mod certs;

/// Compile-time configuration consts (API base URL, sync interval, …).
/// Mirrored here so `crate::config::…` references inside re-exported
/// modules resolve to the same path in the bin and this lib.
pub mod config;
