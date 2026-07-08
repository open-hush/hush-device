//! BLE radio bring-up + the GATT-server abstraction the Improv pairing
//! task drives.
//!
//! Compiled only on the Xtensa target under the `ble-improv` feature (see
//! `Cargo.toml`). The pure Improv protocol — RPC framing, checksums, the
//! provisioning state machine — lives in [`crate::proto::improv`] and is
//! host-tested independently of anything in this file.
//!
//! ## Two layers
//!
//! 1. **HCI controller bring-up** ([`ble_controller`]). `esp-wifi`'s
//!    [`BleConnector`] already implements `bt_hci::transport::Transport`;
//!    `bt-hci`'s `ExternalController` lifts it into the `Controller` a GATT
//!    host stack consumes. This is the verified, decision-free part.
//! 2. **GATT server seam** ([`ImprovGatt`]). The pairing task
//!    ([`crate::tasks::ble::run_pairing`]) depends on this trait, not on a
//!    concrete BLE host stack, so (a) the orchestration is reviewable and
//!    type-checked without the radio, and (b) the host-stack choice
//!    (`trouble-host` vs `bleps`) stays a single, swappable implementation.
//!    That choice is an open ADR — see `docs/adr/0001-ble-host-stack.md`.

use crate::proto::improv::{ErrorState, State};

use bt_hci::controller::ExternalController;
use esp_hal::peripherals::BT;
use esp_wifi::{EspWifiController, ble::controller::BleConnector};

/// HCI command slots the external controller buffers. 20 matches the
/// esp-hal BLE examples for this `esp-wifi 0.14` generation; revisit if the
/// GATT host stack reports command-queue exhaustion on the bench.
pub const HCI_SLOTS: usize = 20;

/// The concrete HCI controller the GATT host stack runs on: esp-wifi's HCI
/// transport wrapped in bt-hci's `ExternalController`.
pub type HushBleController<'d> = ExternalController<BleConnector<'d>, HCI_SLOTS>;

/// Bring the BLE radio up and hand back a `Controller` ready for a GATT
/// host stack. `init` is the same [`EspWifiController`] that powers the
/// Wi-Fi STA path; the radio coexists with WiFi on the shared 2.4 GHz front
/// end (esp-wifi arbitrates internally).
pub fn ble_controller<'d>(init: &'d EspWifiController<'d>, bt: BT<'d>) -> HushBleController<'d> {
    ExternalController::new(BleConnector::new(init, bt))
}

/// The Improv GATT server, abstracted so the pairing task never touches a
/// concrete BLE host stack.
///
/// A concrete implementation owns the advertised service + its five
/// characteristics (UUIDs in [`crate::proto::improv`]) and pumps the host
/// stack's event loop internally. The contract:
///
/// - `next_rpc` blocks until a client writes the `RPC Command`
///   characteristic, returning the raw bytes for [`parse_command`] to
///   decode. The implementation keeps the stack serviced while it waits.
/// - `notify_state` / `notify_error` / `notify_result` update the matching
///   read+notify characteristic and push a notification to the subscriber.
/// - `teardown` stops advertising and frees the BLE host stack + radio
///   (~30 KiB of internal SRAM the device wants back for audio once
///   provisioned).
///
/// [`parse_command`]: crate::proto::improv::parse_command
#[allow(async_fn_in_trait)] // internal trait; we never need the future to be Send.
pub trait ImprovGatt {
    /// Implementation-specific GATT/host-stack error. `Debug` so the task
    /// can log it without imposing a concrete type.
    type Error: core::fmt::Debug;

    /// Publish the `Current State` characteristic (read + notify).
    async fn notify_state(&mut self, state: State) -> Result<(), Self::Error>;

    /// Publish the `Error State` characteristic (read + notify).
    async fn notify_error(&mut self, error: ErrorState) -> Result<(), Self::Error>;

    /// Publish the `RPC Result` characteristic (read + notify).
    async fn notify_result(&mut self, result: &[u8]) -> Result<(), Self::Error>;

    /// Await the next write to the `RPC Command` characteristic, copying it
    /// into `buf` and returning the written prefix.
    async fn next_rpc<'a>(&mut self, buf: &'a mut [u8]) -> Result<&'a [u8], Self::Error>;

    /// Stop advertising, drop the GATT server, and free the BLE stack.
    async fn teardown(self) -> Result<(), Self::Error>;
}
