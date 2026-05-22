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
/// `hush-protocol/docs/auth.md`. Re-exported from `src/api/hmac.rs`
/// so the firmware bin's `mod api { mod hmac; }` keeps owning the file
/// and there is exactly one source of truth.
#[path = "api/hmac.rs"]
pub mod hmac;
