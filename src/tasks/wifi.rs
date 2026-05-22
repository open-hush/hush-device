//! WiFi STA task — Phase 1 smoke test.
//!
//! Brings the radio up, joins the configured AP, and runs an indefinite
//! reconnect loop. There is **no IP stack here**: the embassy-net /
//! DHCP / DNS / TCP bring-up belongs in Phase 2 where it powers the
//! HTTPS sync client. The point of Phase 1 is to prove that
//!
//! - the `esp-wifi` init handshake completes (timer + RNG + radio
//!   clocks plumbed correctly),
//! - the `WifiController` accepts a [`ClientConfiguration`] and
//!   transitions through `StaStart → StaConnected`,
//! - we recover from a flaky AP (mid-test power cycle, weak RSSI,
//!   etc.) instead of hanging.
//!
//! The boot lines this task emits are part of the canonical Phase 1
//! log set documented in `PLAN.md`:
//!
//! - `wifi: radio started, joining "<ssid>"`
//! - `wifi: associated with "<ssid>"`
//! - `wifi: disconnected, reconnecting in 2 s`
//! - `wifi: connect failed: <err>, retrying in 5 s`
//!
//! Stack: ~4 KiB. The userland surface of `esp-wifi` is shallow
//! (it owns its preempt-scheduler threads internally); this task just
//! awaits two futures at a time, so 4 KiB is comfortable.

use embassy_time::{Duration, Timer};
use esp_wifi::wifi::{
    AuthMethod, ClientConfiguration, Configuration, WifiController, WifiEvent,
};
use log::{error, info};

use crate::hw::wifi::WifiCredentials;

const RECONNECT_BACKOFF: Duration = Duration::from_secs(2);
const CONNECT_ERROR_BACKOFF: Duration = Duration::from_secs(5);

#[embassy_executor::task]
pub async fn wifi_task(mut controller: WifiController<'static>, creds: WifiCredentials) {
    let config = Configuration::Client(ClientConfiguration {
        ssid: creds.ssid.clone(),
        password: creds.password.clone(),
        // WPA2-Personal is the only auth method Phase 1 supports.
        // Open networks and WPA3 are deliberately out of scope until
        // BLE Improv (Phase 5) lets the user pick. WPA-only legacy
        // APs are unlikely on a 2026 deployment; we'll add WPA3 when
        // we have a way to configure it from the dashboard.
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    });

    if let Err(err) = controller.set_configuration(&config) {
        error!("wifi: set_configuration failed: {err:?}");
        return;
    }

    if let Err(err) = controller.start_async().await {
        error!("wifi: start_async failed: {err:?}");
        return;
    }
    info!("wifi: radio started, joining \"{}\"", creds.ssid.as_str());

    loop {
        match controller.connect_async().await {
            Ok(()) => {
                info!("wifi: associated with \"{}\"", creds.ssid.as_str());
                // Park here until the AP drops us. `wait_for_event`
                // resolves on the next matching event without polling.
                controller.wait_for_event(WifiEvent::StaDisconnected).await;
                info!("wifi: disconnected, reconnecting in 2 s");
                Timer::after(RECONNECT_BACKOFF).await;
            }
            Err(err) => {
                error!("wifi: connect failed: {err:?}, retrying in 5 s");
                Timer::after(CONNECT_ERROR_BACKOFF).await;
            }
        }
    }
}
