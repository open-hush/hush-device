//! BLE Improv Wi-Fi pairing task (Phase 5).
//!
//! Drives the [`Improv`] provisioning state machine over a GATT server
//! ([`ImprovGatt`]) and a Wi-Fi provisioner ([`WifiProvisioner`]). The flow:
//!
//! ```text
//! advertise (LED: amber slow-blink)
//!   └─ client writes SendWifiSettings
//!        └─ join AP + register backend (LED: blue slow-blink)
//!             ├─ ok   → Provisioned, notify result, LED solid green, tear BLE down
//!             └─ fail → UnableToConnect, LED red, back to advertising
//! ```
//!
//! Compiled only on the Xtensa target under the `ble-improv` feature. The
//! function is generic over its two collaborators so its control flow is
//! type-checked (and could be host-tested with mocks) without the radio;
//! the concrete GATT host stack is the only bench-pending piece.

use embassy_time::{Duration, Timer};
use log::{info, warn};

use crate::hw::ble::ImprovGatt;
use crate::proto::improv::{Action, Improv, ProvisionOutcome, RPC_COMMAND_MAX};
use crate::proto::led::{Colour, LED_CHAN, LedState, Pattern};

/// Stack size for the concrete pairing task. ~8 KiB: the GATT host-stack
/// event loop plus the Wi-Fi connect path are the deepest call chains here,
/// and both allocate their large buffers elsewhere (PSRAM / static). Refine
/// with `cargo size` once the concrete task is wired on the bench.
pub const BLE_TASK_STACK: usize = 8192;

/// Joins an AP and brings the device onto the backend. Abstracted so the
/// pairing task does not hard-wire the Wi-Fi controller, NVS and the
/// register HTTP path; the concrete implementation:
///
/// 1. configures the [`esp_wifi::wifi::WifiController`] with the creds and
///    waits for association (bounded by a timeout),
/// 2. on success persists them via
///    [`crate::storage::nvs::NvsStore::set_wifi_credentials`] so the unit
///    survives a reboot without re-pairing,
/// 3. triggers `POST /v1/device/register` (the Phase 2 client), and
/// 4. returns whether *both* the join and the registration succeeded.
#[allow(async_fn_in_trait)] // internal trait; future need not be Send.
pub trait WifiProvisioner {
    /// `true` only when the AP joined **and** registration succeeded.
    async fn provision(&mut self, ssid: &str, password: &str) -> bool;
}

/// How long the device dwells on a status colour before returning to the
/// advertising indication, so the user actually sees the transition.
const STATUS_DWELL: Duration = Duration::from_secs(2);

async fn set_led(state: LedState) {
    // The LED task drains this channel promptly; `send` only awaits if the
    // tiny channel is momentarily full, which back-pressures correctly.
    LED_CHAN.send(state).await;
}

/// Run the Improv pairing flow to completion. Returns once the device is
/// provisioned and the BLE stack has been torn down; loops on the GATT
/// server otherwise (each failed attempt leaves the device advertising for
/// a retry).
pub async fn run_pairing<G, P>(mut gatt: G, mut provisioner: P)
where
    G: ImprovGatt,
    P: WifiProvisioner,
{
    let mut sm = Improv::new();
    info!("ble: entering Improv pairing mode");
    set_led(LedState::new(Colour::Amber, Pattern::SlowBlink)).await;
    let _ = gatt.notify_state(sm.state()).await;
    let _ = gatt.notify_error(sm.error()).await;

    let mut buf = [0u8; RPC_COMMAND_MAX];
    loop {
        let packet = match gatt.next_rpc(&mut buf).await {
            Ok(p) => p,
            Err(e) => {
                warn!("ble: GATT read failed: {e:?}");
                continue;
            }
        };

        match sm.on_rpc(packet) {
            // Parse error, or a write that arrived mid-provisioning. The
            // state machine already updated the error byte; re-publish so
            // the app sees why and can retry.
            Action::None => {
                let _ = gatt.notify_error(sm.error()).await;
                let _ = gatt.notify_state(sm.state()).await;
            }

            // Identify: blink fast for a couple of seconds so the user can
            // pick this unit out, then resume the advertising indication.
            Action::Identify => {
                info!("ble: identify request");
                set_led(LedState::new(Colour::Blue, Pattern::FastBlink)).await;
                Timer::after(STATUS_DWELL).await;
                set_led(LedState::new(Colour::Amber, Pattern::SlowBlink)).await;
            }

            // Credentials accepted — try to join + register.
            Action::Provision(creds) => {
                let _ = gatt.notify_state(sm.state()).await; // Provisioning
                set_led(LedState::new(Colour::Blue, Pattern::SlowBlink)).await;
                info!("ble: provisioning, joining \"{}\"", creds.ssid.as_str());

                let ok = provisioner
                    .provision(creds.ssid.as_str(), creds.password.as_str())
                    .await;

                let outcome = if ok {
                    // A post-provisioning redirect URL is optional in the
                    // Improv spec; the app claims the device via the
                    // `claimCode` from register, so we send an empty result.
                    ProvisionOutcome::Success { redirect_url: None }
                } else {
                    ProvisionOutcome::Failed
                };

                let action = sm.on_provision_result(outcome);
                let _ = gatt.notify_error(sm.error()).await;
                let _ = gatt.notify_state(sm.state()).await;

                match action {
                    Action::Provisioned(result) => {
                        let _ = gatt.notify_result(&result).await;
                        set_led(LedState::solid(Colour::Green)).await;
                        info!("ble: provisioned; tearing down BLE stack (frees ~30 KiB)");
                        if let Err(e) = gatt.teardown().await {
                            warn!("ble: teardown failed: {e:?}");
                        }
                        return;
                    }
                    _ => {
                        warn!("ble: provisioning failed; resuming advertising");
                        set_led(LedState::new(Colour::Red, Pattern::FastBlink)).await;
                        Timer::after(STATUS_DWELL).await;
                        set_led(LedState::new(Colour::Amber, Pattern::SlowBlink)).await;
                    }
                }
            }

            // `on_rpc` never yields `Provisioned` (only `on_provision_result`
            // does). Handle it without panicking — a pairing task crash
            // would strand the user mid-onboarding.
            Action::Provisioned(_) => {
                warn!("ble: unexpected Provisioned action from on_rpc; ignoring");
            }
        }
    }
}
