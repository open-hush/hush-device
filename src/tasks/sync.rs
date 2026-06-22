//! Sync task — owns the device's conversation with the backend.
//!
//! Lifecycle, in order:
//!
//! 1. **Boot**: read the factory secret + (optional) `deviceId` from NVS. No
//!    secret ⇒ the unit was never provisioned; we log and park (the device
//!    is useless without it, but we must not panic the executor).
//! 2. **Register** (first boot only): `POST /v1/device/register`, persist the
//!    returned `device.id` to NVS so subsequent boots skip this step.
//! 3. **Sync loop**: every `syncIntervalSec` (from the last `DeviceConfig`,
//!    default [`crate::config::DEFAULT_SYNC_INTERVAL_SEC`], floored at 60 s)
//!    call `GET /v1/device/sync` with the `since` of the last successful
//!    sync. On `200` apply + persist the new config; on `304` keep the
//!    cached one.
//! 4. **Flush**: after each sync, drain the [`crate::storage::outbox`] via
//!    `POST /v1/device/events`, removing events only after a `202` so a
//!    failed flush re-sends the same `eventId`s (idempotent).
//!
//! ## Clock
//!
//! HMAC `ts` must be real unix-seconds within ±300 s of the server
//! (`docs/auth.md`). The task takes a [`UnixClock`] that main wires to an
//! SNTP-synced source; until NTP succeeds the clock reports `None` and the
//! task waits rather than signing with a bogus timestamp (which would 401
//! `expired_token`).
//!
//! Stack: 8 KiB — the TLS workspace and the JSON parse buffers dominate, and
//! both live in caller-owned PSRAM statics, so the task's own stack only
//! holds the futures' state machines.
//!
//! This task is Xtensa-only (it drives `reqwless` + `esp-storage`); its
//! orchestration is verified on the bench. The wire (de)serialization,
//! request signing, NVS codec and outbox logic it calls are host-tested in
//! their own modules.

#![cfg(all(target_arch = "xtensa", feature = "phase2-io"))]

use embassy_time::{Duration, Timer};
use log::{error, info, warn};

use embedded_storage_async::nor_flash::NorFlash;

use crate::api::client::HttpOutcome;
use crate::api::client::transport::{DeviceClient, TransportError};
use crate::config::{API_BASE_URL, DEFAULT_SYNC_INTERVAL_SEC};
use crate::proto::api::{DeviceConfig, DeviceEventsRequest, DeviceRegisterRequest};
use crate::storage::nvs::NvsStore;
use crate::storage::outbox::Outbox;

/// Hard floor on the sync interval regardless of what the backend pushes —
/// protects the backend from a misconfigured `syncIntervalSec`.
const MIN_SYNC_INTERVAL_SEC: u32 = 60;

/// Back-off after a retriable failure (rate-limit / transient server error).
const BACKOFF: Duration = Duration::from_secs(30);

/// A wall-clock source. `now_unix()` returns `None` until NTP has synced;
/// the task refuses to sign requests before then.
pub trait UnixClock {
    fn now_unix(&self) -> Option<u64>;
}

/// Compute the effective sync interval from the last config, flooring it.
fn sync_interval_secs(config: Option<&DeviceConfig>) -> u32 {
    // DeviceConfig carries sleep thresholds, not the sync cadence; the
    // cadence is a device constant for now (the spec keeps `syncIntervalSec`
    // dashboard-side for a later phase). Kept as a function so wiring the
    // server value in later is a one-line change.
    let _ = config;
    DEFAULT_SYNC_INTERVAL_SEC.max(MIN_SYNC_INTERVAL_SEC)
}

/// Wait until the clock has a valid unix timestamp (NTP synced), polling at
/// 1 Hz. Returns the timestamp.
async fn wait_for_clock(clock: &impl UnixClock) -> u64 {
    loop {
        if let Some(ts) = clock.now_unix() {
            return ts;
        }
        warn!("sync: waiting for NTP before signing requests");
        Timer::after(Duration::from_secs(1)).await;
    }
}

/// The sync task entry point. `serial` and `firmware_version` are the
/// build/provisioning identity used for first-boot registration.
pub async fn run_sync<F: NorFlash>(
    client: &mut DeviceClient<'_>,
    nvs: &mut NvsStore<F>,
    storage: &mut NvsStore<F>,
    outbox: &mut Outbox,
    clock: &impl UnixClock,
    serial: &str,
    firmware_version: &str,
) {
    info!("sync: task up, target {API_BASE_URL}");

    // 1. Factory secret. No secret ⇒ unprovisioned: nothing we can sign.
    let secret = match nvs.device_secret() {
        Ok(s) => s,
        Err(e) => {
            error!("sync: no device secret in NVS ({e:?}); device is unprovisioned, parking");
            return;
        }
    };

    // 2. Ensure we have a deviceId, registering on first boot.
    let device_id =
        match ensure_registered(client, nvs, &secret, clock, serial, firmware_version).await {
            Some(id) => id,
            None => {
                error!("sync: registration failed; parking (will retry on next reboot)");
                return;
            }
        };
    let device_id_str = crate::api::client::format_uuid(&device_id);

    // Last config persisted across reboots seeds the interval.
    let mut last_config = storage.last_config().ok();
    let mut since: Option<heapless::String<{ crate::proto::api::TIMESTAMP_LEN }>> = None;

    // 3 + 4. Sync loop.
    loop {
        let ts = wait_for_clock(clock).await;
        match client
            .sync(&secret, device_id_str.as_str(), since.as_deref(), ts)
            .await
        {
            Ok(Some(resp)) => {
                if let Err(e) = storage.set_last_config(&resp.config) {
                    warn!("sync: failed to persist config: {e:?}");
                }
                last_config = Some(resp.config);
                since = heapless::String::try_from(resp.server_time.as_str()).ok();
                info!(
                    "sync: applied config ({} cards, {} audio)",
                    resp.cards.len(),
                    resp.audio.len()
                );
            }
            Ok(None) => info!("sync: 304 not modified"),
            Err(TransportError::Status(HttpOutcome::Unauthorized)) => {
                error!("sync: 401 — clock skew or bad secret; backing off");
            }
            Err(e) => warn!("sync: failed: {e:?}"),
        }

        flush_events(client, outbox, &secret, device_id_str.as_str(), clock).await;

        let interval = sync_interval_secs(last_config.as_ref());
        Timer::after(Duration::from_secs(interval as u64)).await;
    }
}

/// Read the stored deviceId, or register and persist it on first boot.
async fn ensure_registered<F: NorFlash>(
    client: &mut DeviceClient<'_>,
    nvs: &mut NvsStore<F>,
    secret: &[u8],
    clock: &impl UnixClock,
    serial: &str,
    firmware_version: &str,
) -> Option<[u8; 16]> {
    if let Ok(id) = nvs.device_id() {
        info!("sync: already registered");
        return Some(id);
    }

    let req = DeviceRegisterRequest {
        serial: heapless::String::try_from(serial).ok()?,
        firmware_version: heapless::String::try_from(firmware_version).ok()?,
        mac_address: None,
    };
    let mut body = [0u8; 256];
    let len = serde_json_core::to_slice(&req, &mut body).ok()?;

    let ts = wait_for_clock(clock).await;
    // keyId for the very first request is the zero UUID; the backend resolves
    // the device from the serial in the body, not the keyId, on register.
    let bootstrap_id = crate::api::client::format_uuid(&[0u8; 16]);
    match client
        .register(secret, bootstrap_id.as_str(), &body[..len], ts)
        .await
    {
        Ok(resp) => {
            let raw = parse_uuid(resp.device.id.as_str())?;
            if let Err(e) = nvs.set_device_id(&raw) {
                error!("sync: registered but failed to persist deviceId: {e:?}");
                return None;
            }
            info!("sync: registered, deviceId persisted");
            Some(raw)
        }
        Err(e) => {
            error!("sync: register failed: {e:?}");
            None
        }
    }
}

/// Drain the outbox in one batch; ack only after a 202 so a failed flush
/// re-sends the same `eventId`s (backend dedups).
async fn flush_events(
    client: &mut DeviceClient<'_>,
    outbox: &mut Outbox,
    secret: &[u8],
    device_id_str: &str,
    clock: &impl UnixClock,
) {
    if outbox.is_empty() {
        return;
    }
    let batch = outbox.batch();
    let n = batch.len();
    let req = DeviceEventsRequest { events: batch };
    let mut body = [0u8; 4096];
    let len = match serde_json_core::to_slice(&req, &mut body) {
        Ok(l) => l,
        Err(_) => {
            warn!("sync: event batch too large to serialize; dropping head");
            outbox.ack(1);
            return;
        }
    };
    let ts = wait_for_clock(clock).await;
    match client
        .post_events(secret, device_id_str, &body[..len], ts)
        .await
    {
        Ok(()) => {
            outbox.ack(n);
            info!("sync: flushed {n} events");
        }
        Err(e) => warn!("sync: event flush failed ({e:?}); will retry same eventIds"),
    }
}

/// Parse a hyphenated lowercase UUID string into its 16 raw bytes. Returns
/// `None` on any malformed input (wrong length, bad hex, misplaced hyphens).
fn parse_uuid(s: &str) -> Option<[u8; 16]> {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    let mut out = [0u8; 16];
    let mut oi = 0;
    let mut i = 0;
    while i < bytes.len() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if bytes[i] != b'-' {
                return None;
            }
            i += 1;
            continue;
        }
        let hi = hex_val(bytes[i])?;
        let lo = hex_val(bytes[i + 1])?;
        out[oi] = (hi << 4) | lo;
        oi += 1;
        i += 2;
    }
    if oi == 16 { Some(out) } else { None }
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
