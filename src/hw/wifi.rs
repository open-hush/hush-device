//! WiFi STA credentials.
//!
//! Phase 1 only needs a single hardcoded SSID + PSK so the device can
//! prove the radio brings up and associates with an AP on the bench.
//! We pull the values from build-time environment variables
//! ([`HUSH_WIFI_SSID`] and [`HUSH_WIFI_PSK`]) instead of inlining them
//! in source so secrets never land in git. The build fails with a
//! clear message if either is unset.
//!
//! Phase 5 (BLE Improv WiFi) replaces this with credentials negotiated
//! over BLE and persisted in NVS; the [`WifiCredentials`] struct will
//! survive that transition and just gain a `from_nvs` constructor.
//!
//! ## String type
//!
//! `esp_wifi::wifi::ClientConfiguration::{ssid,password}` are
//! `alloc::string::String` from `esp-wifi 0.14` onwards (it used to be
//! `heapless::String<32>` / `<64>` in 0.13; the rename landed alongside
//! the `esp-hal 1.0.0-beta.1` pin). We allocate them from the PSRAM
//! heap configured in `main.rs`. The two `String` instances together
//! cost at most 96 bytes of heap.
//!
//! [`HUSH_WIFI_SSID`]: #
//! [`HUSH_WIFI_PSK`]: #

extern crate alloc;

use alloc::string::{String, ToString};

/// Hard limits taken straight from the ESP-IDF / `esp-wifi` ABI. We
/// surface them here so the build-time check below catches an
/// over-long SSID at compile time rather than at boot.
pub const WIFI_SSID_MAX_LEN: usize = 32;
pub const WIFI_PSK_MAX_LEN: usize = 64;

#[derive(Debug, Clone)]
pub struct WifiCredentials {
    pub ssid: String,
    pub password: String,
}

impl WifiCredentials {
    /// Read the SSID and PSK from the `HUSH_WIFI_SSID` / `HUSH_WIFI_PSK`
    /// build-time env vars. The build refuses to link if either is
    /// missing — that is intentional: a firmware image without
    /// credentials silently fails on the bench in a way that wastes
    /// time, so we surface it at compile time.
    pub fn from_env() -> Self {
        const SSID: &str = env!(
            "HUSH_WIFI_SSID",
            "set HUSH_WIFI_SSID before building (Phase 1 WiFi STA smoke test)"
        );
        const PSK: &str = env!(
            "HUSH_WIFI_PSK",
            "set HUSH_WIFI_PSK before building (Phase 1 WiFi STA smoke test)"
        );

        // Compile-time length checks. `const_assert`-style: if the
        // env-var value is too long, the `[(); ...]` indexing panics
        // during const evaluation and the build fails with a clear
        // location.
        const _: () = {
            assert!(
                SSID.len() <= WIFI_SSID_MAX_LEN,
                "HUSH_WIFI_SSID longer than 32 bytes — WiFi SSIDs are capped at 32 by the spec"
            );
            assert!(
                PSK.len() <= WIFI_PSK_MAX_LEN,
                "HUSH_WIFI_PSK longer than 64 bytes — WPA2 PSKs are capped at 64 ASCII chars"
            );
        };

        Self {
            ssid: SSID.to_string(),
            password: PSK.to_string(),
        }
    }
}
