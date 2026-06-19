//! Wire types matching the OpenAPI spec in
//! `hush-protocol/hush-api.yaml`.
//!
//! Drift between this file and the spec is a bug. A future CI job will
//! diff the generated types against the spec; until then, treat any
//! mismatch as a release blocker. The host tests at the bottom pin the
//! serialized/deserialized shapes to the spec's worked examples so a
//! rename or field-shape change surfaces here, not at runtime against a
//! live backend.
//!
//! ## no_std / no-alloc constraints
//!
//! Every string field is a [`heapless::String`] with a fixed capacity and
//! every array is a [`heapless::Vec`] with a fixed element cap, so
//! `serde-json-core` can (de)serialize without an allocator. The capacities
//! are sized for the real payloads (presigned URLs are the long pole) with
//! margin; an over-long field deserializes to an error rather than
//! truncating silently.
//!
//! ## Direction of travel
//!
//! - The device **deserializes** [`DeviceRegisterResponse`] and
//!   [`DeviceSyncResponse`] (and [`ApiError`] on the 4xx path).
//! - The device **serializes** [`DeviceRegisterRequest`],
//!   [`DeviceEventsRequest`] and the [`DeviceEvent`] union.
//!
//! `DeviceEvent` carries a manual [`Serialize`] impl because the wire shape
//! is a discriminated union with a nested `payload` object
//! (`{eventId, ts, type, payload}`). `serde-json-core` does not support
//! `#[serde(flatten)]` or adjacently-tagged enums, so we drive the four
//! top-level fields explicitly and let each payload struct derive
//! `Serialize`. The device never deserializes events, so no `Deserialize`
//! is needed for the union.

use heapless::{String, Vec};
use serde::{Deserialize, Serialize};

// -----------------------------------------------------------------------------
// Field capacities. Kept as named consts so the rationale lives next to the
// number and call sites can reference the same bound.
// -----------------------------------------------------------------------------

/// Canonical lowercase UUID rendering is always 36 bytes.
pub const UUID_LEN: usize = 36;
/// ISO-8601 UTC with the mandatory `Z` suffix: `2026-06-19T13:17:09.123Z`
/// is 24 bytes; round up for slack.
pub const TIMESTAMP_LEN: usize = 32;
/// RFID UID hex, `^[0-9a-f]{8,20}$` per the spec — 20 bytes max.
pub const UID_HEX_LEN: usize = 20;
/// Device serial printed on the unit. Generous; serials are short.
pub const SERIAL_LEN: usize = 64;
/// SemVer firmware string, e.g. `0.2.0`.
pub const FW_VERSION_LEN: usize = 32;
/// Claim code shown during pairing — short by design.
pub const CLAIM_CODE_LEN: usize = 32;
/// Hex SHA-256 of the transcoded MP3.
pub const SHA256_HEX_LEN: usize = 64;
/// User-chosen device name (spec maxLength 120).
pub const DEVICE_NAME_LEN: usize = 120;
/// Presigned GET URLs are the long pole. S3/GCS presigned URLs routinely
/// run several hundred bytes once the signature, headers and expiry are
/// query-encoded. 1 KiB covers them with margin; an over-long URL fails
/// deserialization rather than truncating to a URL that 403s.
pub const URL_LEN: usize = 1024;
/// Machine-readable error code (snake_case).
pub const ERROR_CODE_LEN: usize = 48;
/// Human-readable error message.
pub const ERROR_MESSAGE_LEN: usize = 256;
/// Short snake_case error tag inside an `error` event payload.
pub const ERROR_REASON_LEN: usize = 48;
/// SSID inside a `wifi_signal` event (IEEE 802.11 caps SSID at 32 bytes).
pub const SSID_LEN: usize = 33;

/// Maximum card bindings carried in one sync snapshot. A single Hush box is
/// not expected to bind anywhere near this many cards; the cap bounds the
/// stack/PSRAM footprint of a [`DeviceSyncResponse`].
pub const MAX_CARDS: usize = 64;
/// Maximum audio entries carried in one sync snapshot.
pub const MAX_AUDIO: usize = 64;
/// Maximum events flushed in a single `POST /v1/device/events` batch. The
/// spec allows up to 200; we cap lower to bound the request buffer.
pub const MAX_EVENTS_PER_BATCH: usize = 32;

// Convenience aliases so field types read like the spec.
pub type Uuid = String<UUID_LEN>;
pub type Timestamp = String<TIMESTAMP_LEN>;
pub type UidHex = String<UID_HEX_LEN>;

// -----------------------------------------------------------------------------
// register
// -----------------------------------------------------------------------------

/// `POST /v1/device/register` request body.
///
/// `macAddress` is optional (`pattern ^([0-9a-f]{2}:){5}[0-9a-f]{2}$`); when
/// `None` it is omitted from the JSON entirely so the backend's optional
/// field stays absent rather than `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRegisterRequest {
    pub serial: String<SERIAL_LEN>,
    pub firmware_version: String<FW_VERSION_LEN>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mac_address: Option<String<17>>,
}

/// `POST /v1/device/register` 200 response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRegisterResponse {
    pub device: Device,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub claim_code: Option<String<CLAIM_CODE_LEN>>,
}

/// Lifecycle state of a device. Mirrors `Device.state` enum in the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    Unclaimed,
    Claimed,
    Retired,
}

/// `Device` resource. Only the fields the firmware consumes are typed with
/// intent; the rest are still deserialized so a full snapshot round-trips.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: Uuid,
    pub serial: String<SERIAL_LEN>,
    pub state: DeviceState,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub owner_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String<DEVICE_NAME_LEN>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub firmware_version: Option<String<FW_VERSION_LEN>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_seen_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

// -----------------------------------------------------------------------------
// sync
// -----------------------------------------------------------------------------

/// `GET /v1/device/sync` 200 response body. A `304 Not Modified` carries no
/// body and is handled at the HTTP layer, not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSyncResponse {
    pub server_time: Timestamp,
    pub config: DeviceConfig,
    pub cards: Vec<CardBinding, MAX_CARDS>,
    pub audio: Vec<AudioSyncEntry, MAX_AUDIO>,
}

/// Power-saving thresholds and limits pushed from the dashboard.
///
/// `lightSleepAfterSec`, `deepSleepAfterSec` and `volumeMax` are required;
/// `ledBrightness` is optional.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConfig {
    pub light_sleep_after_sec: u32,
    pub deep_sleep_after_sec: u32,
    pub volume_max: u8,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub led_brightness: Option<u8>,
}

/// One RFID-card → audio binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardBinding {
    pub uid: UidHex,
    pub audio_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bound_at: Option<Timestamp>,
}

/// One audio item in the sync manifest, with a presigned download URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSyncEntry {
    pub id: Uuid,
    pub sha256: String<SHA256_HEX_LEN>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub size_bytes: Option<u64>,
    pub download_url: String<URL_LEN>,
    pub expires_at: Timestamp,
}

// -----------------------------------------------------------------------------
// error envelope
// -----------------------------------------------------------------------------

/// Canonical `Error` schema returned on the 4xx path. Named `ApiError` to
/// avoid clashing with [`crate::error::Error`]. `details` is intentionally
/// dropped — it is `additionalProperties: true` (an arbitrary map), which
/// has no fixed no-alloc shape and the firmware never reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String<ERROR_CODE_LEN>,
    pub message: String<ERROR_MESSAGE_LEN>,
}

// -----------------------------------------------------------------------------
// events
// -----------------------------------------------------------------------------

/// `POST /v1/device/events` request body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceEventsRequest {
    pub events: Vec<DeviceEvent, MAX_EVENTS_PER_BATCH>,
}

/// Reason a `playback_finished` event fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackFinishedReason {
    Completed,
    Interrupted,
    Error,
}

/// Which physical control fired a `button_pressed` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Reset,
    Pairing,
    Encoder,
}

/// The `DeviceEvent` discriminated union (spec `DeviceEvent`).
///
/// `eventId` is a client-generated UUID; the backend deduplicates on
/// `(deviceId, eventId)`, so the firmware MUST reuse the same `eventId`
/// when it retries a flush. `ts` is the UTC ISO-8601 capture time.
///
/// The [`Serialize`] impl is hand-written (see module docs) to emit the
/// `{eventId, ts, type, payload}` wire shape `serde-json-core`'s derive
/// cannot express.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    CardScanned {
        event_id: Uuid,
        ts: Timestamp,
        uid: UidHex,
    },
    CardUnknown {
        event_id: Uuid,
        ts: Timestamp,
        uid: UidHex,
    },
    PlaybackStarted {
        event_id: Uuid,
        ts: Timestamp,
        audio_id: Uuid,
    },
    PlaybackFinished {
        event_id: Uuid,
        ts: Timestamp,
        audio_id: Uuid,
        reason: PlaybackFinishedReason,
        position_ms: Option<u32>,
    },
    ButtonPressed {
        event_id: Uuid,
        ts: Timestamp,
        button: Button,
        duration_ms: Option<u32>,
    },
    VolumeChanged {
        event_id: Uuid,
        ts: Timestamp,
        volume: u8,
    },
    LowBattery {
        event_id: Uuid,
        ts: Timestamp,
        battery_percent: u8,
        voltage_mv: Option<u16>,
    },
    WifiSignal {
        event_id: Uuid,
        ts: Timestamp,
        rssi: i16,
        ssid: Option<String<SSID_LEN>>,
    },
    Error {
        event_id: Uuid,
        ts: Timestamp,
        reason: String<ERROR_REASON_LEN>,
    },
}

impl DeviceEvent {
    /// The `type` discriminator string sent on the wire.
    pub fn type_str(&self) -> &'static str {
        match self {
            DeviceEvent::CardScanned { .. } => "card_scanned",
            DeviceEvent::CardUnknown { .. } => "card_unknown",
            DeviceEvent::PlaybackStarted { .. } => "playback_started",
            DeviceEvent::PlaybackFinished { .. } => "playback_finished",
            DeviceEvent::ButtonPressed { .. } => "button_pressed",
            DeviceEvent::VolumeChanged { .. } => "volume_changed",
            DeviceEvent::LowBattery { .. } => "low_battery",
            DeviceEvent::WifiSignal { .. } => "wifi_signal",
            DeviceEvent::Error { .. } => "error",
        }
    }

    /// The client-generated idempotency key.
    pub fn event_id(&self) -> &str {
        match self {
            DeviceEvent::CardScanned { event_id, .. }
            | DeviceEvent::CardUnknown { event_id, .. }
            | DeviceEvent::PlaybackStarted { event_id, .. }
            | DeviceEvent::PlaybackFinished { event_id, .. }
            | DeviceEvent::ButtonPressed { event_id, .. }
            | DeviceEvent::VolumeChanged { event_id, .. }
            | DeviceEvent::LowBattery { event_id, .. }
            | DeviceEvent::WifiSignal { event_id, .. }
            | DeviceEvent::Error { event_id, .. } => event_id.as_str(),
        }
    }
}

// Payload structs — these derive `Serialize` and produce exactly the
// `payload` object the spec defines for each event type.
#[derive(Serialize)]
struct UidPayload<'a> {
    uid: &'a str,
}

#[derive(Serialize)]
struct PlaybackStartedPayload<'a> {
    #[serde(rename = "audioId")]
    audio_id: &'a str,
}

#[derive(Serialize)]
struct PlaybackFinishedPayload<'a> {
    #[serde(rename = "audioId")]
    audio_id: &'a str,
    reason: PlaybackFinishedReason,
    #[serde(rename = "positionMs", skip_serializing_if = "Option::is_none")]
    position_ms: Option<u32>,
}

#[derive(Serialize)]
struct ButtonPressedPayload {
    button: Button,
    #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u32>,
}

#[derive(Serialize)]
struct VolumePayload {
    volume: u8,
}

#[derive(Serialize)]
struct LowBatteryPayload {
    #[serde(rename = "batteryPercent")]
    battery_percent: u8,
    #[serde(rename = "voltageMv", skip_serializing_if = "Option::is_none")]
    voltage_mv: Option<u16>,
}

#[derive(Serialize)]
struct WifiSignalPayload<'a> {
    rssi: i16,
    #[serde(skip_serializing_if = "Option::is_none")]
    ssid: Option<&'a str>,
}

#[derive(Serialize)]
struct ErrorPayload<'a> {
    reason: &'a str,
}

impl Serialize for DeviceEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;

        // Helper: emit the three common fields plus a typed payload.
        fn emit<S, P>(
            serializer: S,
            event_id: &str,
            ts: &str,
            type_str: &'static str,
            payload: &P,
        ) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
            P: Serialize,
        {
            let mut s = serializer.serialize_struct("DeviceEvent", 4)?;
            s.serialize_field("eventId", event_id)?;
            s.serialize_field("ts", ts)?;
            s.serialize_field("type", type_str)?;
            s.serialize_field("payload", payload)?;
            s.end()
        }

        match self {
            DeviceEvent::CardScanned { event_id, ts, uid } => emit(
                serializer,
                event_id,
                ts,
                "card_scanned",
                &UidPayload { uid: uid.as_str() },
            ),
            DeviceEvent::CardUnknown { event_id, ts, uid } => emit(
                serializer,
                event_id,
                ts,
                "card_unknown",
                &UidPayload { uid: uid.as_str() },
            ),
            DeviceEvent::PlaybackStarted {
                event_id,
                ts,
                audio_id,
            } => emit(
                serializer,
                event_id,
                ts,
                "playback_started",
                &PlaybackStartedPayload {
                    audio_id: audio_id.as_str(),
                },
            ),
            DeviceEvent::PlaybackFinished {
                event_id,
                ts,
                audio_id,
                reason,
                position_ms,
            } => emit(
                serializer,
                event_id,
                ts,
                "playback_finished",
                &PlaybackFinishedPayload {
                    audio_id: audio_id.as_str(),
                    reason: *reason,
                    position_ms: *position_ms,
                },
            ),
            DeviceEvent::ButtonPressed {
                event_id,
                ts,
                button,
                duration_ms,
            } => emit(
                serializer,
                event_id,
                ts,
                "button_pressed",
                &ButtonPressedPayload {
                    button: *button,
                    duration_ms: *duration_ms,
                },
            ),
            DeviceEvent::VolumeChanged {
                event_id,
                ts,
                volume,
            } => emit(
                serializer,
                event_id,
                ts,
                "volume_changed",
                &VolumePayload { volume: *volume },
            ),
            DeviceEvent::LowBattery {
                event_id,
                ts,
                battery_percent,
                voltage_mv,
            } => emit(
                serializer,
                event_id,
                ts,
                "low_battery",
                &LowBatteryPayload {
                    battery_percent: *battery_percent,
                    voltage_mv: *voltage_mv,
                },
            ),
            DeviceEvent::WifiSignal {
                event_id,
                ts,
                rssi,
                ssid,
            } => emit(
                serializer,
                event_id,
                ts,
                "wifi_signal",
                &WifiSignalPayload {
                    rssi: *rssi,
                    ssid: ssid.as_ref().map(|s| s.as_str()),
                },
            ),
            DeviceEvent::Error {
                event_id,
                ts,
                reason,
            } => emit(
                serializer,
                event_id,
                ts,
                "error",
                &ErrorPayload {
                    reason: reason.as_str(),
                },
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use heapless::String as HString;

    /// Helper: build a heapless::String from a &str, panicking on overflow
    /// (test-only, capacities are sized for these literals).
    fn s<const N: usize>(v: &str) -> HString<N> {
        HString::try_from(v).expect("test literal fits capacity")
    }

    #[test]
    fn register_request_serializes_camel_case_and_omits_absent_mac() {
        let req = DeviceRegisterRequest {
            serial: s("HUSH-0001"),
            firmware_version: s("0.2.0"),
            mac_address: None,
        };
        let mut buf = [0u8; 128];
        let len = serde_json_core::to_slice(&req, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert_eq!(json, r#"{"serial":"HUSH-0001","firmwareVersion":"0.2.0"}"#);
    }

    #[test]
    fn register_request_serializes_mac_when_present() {
        let req = DeviceRegisterRequest {
            serial: s("HUSH-0001"),
            firmware_version: s("0.2.0"),
            mac_address: Some(s("aa:bb:cc:dd:ee:ff")),
        };
        let mut buf = [0u8; 128];
        let len = serde_json_core::to_slice(&req, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert_eq!(
            json,
            r#"{"serial":"HUSH-0001","firmwareVersion":"0.2.0","macAddress":"aa:bb:cc:dd:ee:ff"}"#
        );
    }

    #[test]
    fn register_response_deserializes_with_claim_code() {
        let body = r#"{
            "device": {
                "id": "550e8400-e29b-41d4-a716-446655440000",
                "serial": "HUSH-0001",
                "state": "unclaimed",
                "firmwareVersion": "0.2.0",
                "createdAt": "2026-06-15T09:34:54Z"
            },
            "claimCode": "ABCD-1234"
        }"#;
        let (resp, _): (DeviceRegisterResponse, usize) = serde_json_core::from_str(body).unwrap();
        assert_eq!(
            resp.device.id.as_str(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(resp.device.state, DeviceState::Unclaimed);
        assert_eq!(resp.claim_code.as_deref(), Some("ABCD-1234"));
    }

    #[test]
    fn sync_response_deserializes_full_snapshot() {
        let body = r#"{
            "serverTime": "2026-06-19T13:17:09Z",
            "config": {
                "lightSleepAfterSec": 30,
                "deepSleepAfterSec": 300,
                "volumeMax": 80,
                "ledBrightness": 50
            },
            "cards": [
                { "uid": "04a1b2c3d4e5", "audioId": "11111111-1111-1111-1111-111111111111", "boundAt": "2026-06-18T10:00:00Z" }
            ],
            "audio": [
                {
                    "id": "11111111-1111-1111-1111-111111111111",
                    "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                    "sizeBytes": 1048576,
                    "downloadUrl": "https://cdn.open-hush.com/audio/x.mp3?sig=abc",
                    "expiresAt": "2026-06-19T14:17:09Z"
                }
            ]
        }"#;
        let (resp, _): (DeviceSyncResponse, usize) = serde_json_core::from_str(body).unwrap();
        assert_eq!(resp.config.light_sleep_after_sec, 30);
        assert_eq!(resp.config.deep_sleep_after_sec, 300);
        assert_eq!(resp.config.volume_max, 80);
        assert_eq!(resp.config.led_brightness, Some(50));
        assert_eq!(resp.cards.len(), 1);
        assert_eq!(resp.cards[0].uid.as_str(), "04a1b2c3d4e5");
        assert_eq!(resp.audio.len(), 1);
        assert_eq!(resp.audio[0].size_bytes, Some(1_048_576));
        assert!(resp.audio[0].download_url.as_str().ends_with("sig=abc"));
    }

    #[test]
    fn sync_response_deserializes_with_empty_arrays_and_optional_config() {
        let body = r#"{
            "serverTime": "2026-06-19T13:17:09Z",
            "config": { "lightSleepAfterSec": 30, "deepSleepAfterSec": 300, "volumeMax": 100 },
            "cards": [],
            "audio": []
        }"#;
        let (resp, _): (DeviceSyncResponse, usize) = serde_json_core::from_str(body).unwrap();
        assert_eq!(resp.config.led_brightness, None);
        assert!(resp.cards.is_empty());
        assert!(resp.audio.is_empty());
    }

    #[test]
    fn api_error_deserializes_ignoring_details() {
        let body = r#"{"code":"invalid_signature","message":"HMAC verification failed","details":{"k":"v"}}"#;
        let (err, _): (ApiError, usize) = serde_json_core::from_str(body).unwrap();
        assert_eq!(err.code.as_str(), "invalid_signature");
        assert_eq!(err.message.as_str(), "HMAC verification failed");
    }

    #[test]
    fn card_scanned_event_serializes_to_spec_shape() {
        let ev = DeviceEvent::CardScanned {
            event_id: s("7c0d976d-e8cc-42c5-ad04-bf1502007fff"),
            ts: s("2026-06-19T13:17:09Z"),
            uid: s("04a1b2c3d4e5"),
        };
        let mut buf = [0u8; 256];
        let len = serde_json_core::to_slice(&ev, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert_eq!(
            json,
            r#"{"eventId":"7c0d976d-e8cc-42c5-ad04-bf1502007fff","ts":"2026-06-19T13:17:09Z","type":"card_scanned","payload":{"uid":"04a1b2c3d4e5"}}"#
        );
    }

    #[test]
    fn playback_finished_event_omits_absent_position() {
        let ev = DeviceEvent::PlaybackFinished {
            event_id: s("11111111-2222-3333-4444-555555555555"),
            ts: s("2026-06-19T13:17:09Z"),
            audio_id: s("99999999-8888-7777-6666-555555555555"),
            reason: PlaybackFinishedReason::Completed,
            position_ms: None,
        };
        let mut buf = [0u8; 256];
        let len = serde_json_core::to_slice(&ev, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert_eq!(
            json,
            r#"{"eventId":"11111111-2222-3333-4444-555555555555","ts":"2026-06-19T13:17:09Z","type":"playback_finished","payload":{"audioId":"99999999-8888-7777-6666-555555555555","reason":"completed"}}"#
        );
    }

    #[test]
    fn wifi_signal_event_includes_negative_rssi_and_ssid() {
        let ev = DeviceEvent::WifiSignal {
            event_id: s("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
            ts: s("2026-06-19T13:17:09Z"),
            rssi: -67,
            ssid: Some(s("home-wifi")),
        };
        let mut buf = [0u8; 256];
        let len = serde_json_core::to_slice(&ev, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert_eq!(
            json,
            r#"{"eventId":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","ts":"2026-06-19T13:17:09Z","type":"wifi_signal","payload":{"rssi":-67,"ssid":"home-wifi"}}"#
        );
    }

    #[test]
    fn events_request_serializes_batch() {
        let mut events: Vec<DeviceEvent, MAX_EVENTS_PER_BATCH> = Vec::new();
        events
            .push(DeviceEvent::CardScanned {
                event_id: s("00000000-0000-0000-0000-000000000001"),
                ts: s("2026-06-19T13:17:09Z"),
                uid: s("04a1b2c3d4e5"),
            })
            .unwrap();
        events
            .push(DeviceEvent::Error {
                event_id: s("00000000-0000-0000-0000-000000000002"),
                ts: s("2026-06-19T13:17:10Z"),
                reason: s("decoder_failed"),
            })
            .unwrap();
        let req = DeviceEventsRequest { events };
        let mut buf = [0u8; 512];
        let len = serde_json_core::to_slice(&req, &mut buf).unwrap();
        let json = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(
            json.starts_with(r#"{"events":[{"eventId":"00000000-0000-0000-0000-000000000001""#)
        );
        assert!(json.contains(r#""type":"error","payload":{"reason":"decoder_failed"}"#));
    }
}
