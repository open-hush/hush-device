//! Typed NVS access over `sequential-storage` + `esp-storage`.
//!
//! NVS is the **source of truth** for everything that must survive a reboot
//! or a microSD swap. Keys:
//!
//! | Key const | Type | Partition | Set at |
//! |---|---|---|---|
//! | [`KEY_DEVICE_SECRET`] | `[u8; 32]` | `nvs` | Factory provisioning, never rotated |
//! | [`KEY_DEVICE_ID`] | `[u8; 16]` (UUID) | `nvs` | After `POST /v1/device/register` |
//! | [`KEY_WIFI_SSID`] | `String<33>` | `nvs` | BLE pairing (Phase 5) |
//! | [`KEY_WIFI_PASS`] | `String<64>` | `nvs` | BLE pairing (Phase 5) |
//! | [`KEY_LAST_CONFIG`] | `DeviceConfig` (JSON) | `storage` | After every sync |
//!
//! ## Layering
//!
//! The **codec** ([`encode_*`] / `decode_*`, the `*Value` newtypes) is pure,
//! allocation-free and host-tested — it is the part that determines whether
//! a value written before a reboot reads back byte-for-byte. The
//! `sequential-storage` + `esp-storage` flash backend ([`NvsStore`]) is
//! Xtensa-only and delegates every (de)serialization to the codec, so the
//! on-flash byte layout is exactly what the host tests pin.
//!
//! `sequential-storage` hands [`crate::proto::api::DeviceConfig`] back the
//! exact slice it stored, so variable-length values (strings, JSON) need no
//! length prefix: the slice length *is* the length.

#![allow(dead_code)] // the Xtensa store is wired into `main`/`sync` incrementally.

use heapless::String;

use crate::proto::api::DeviceConfig;

/// Per-device HMAC secret, baked at provisioning. Never rotated on-device.
pub const KEY_DEVICE_SECRET: u8 = 0x01;
/// Device UUID assigned by the backend at `register`, stored as 16 raw bytes.
pub const KEY_DEVICE_ID: u8 = 0x02;
/// WiFi SSID (≤ 32 bytes per IEEE 802.11, +1 slack).
pub const KEY_WIFI_SSID: u8 = 0x03;
/// WiFi passphrase (≤ 63 bytes for WPA2-Personal, +1 slack).
pub const KEY_WIFI_PASS: u8 = 0x04;
/// Last `DeviceConfig` received from the backend, JSON-encoded.
pub const KEY_LAST_CONFIG: u8 = 0x05;

/// Raw length of the device secret.
pub const DEVICE_SECRET_LEN: usize = 32;
/// Raw length of a UUID in bytes.
pub const DEVICE_ID_LEN: usize = 16;
/// SSID capacity (33 = 32 + 1 NUL-free slack, matches `hw::wifi`).
pub const WIFI_SSID_CAP: usize = 33;
/// WPA2 passphrase capacity.
pub const WIFI_PASS_CAP: usize = 64;
/// Upper bound on the JSON encoding of a [`DeviceConfig`]. The largest shape
/// (`{"lightSleepAfterSec":4294967295,"deepSleepAfterSec":4294967295,"volumeMax":100,"ledBrightness":100}`)
/// is ~96 bytes; 128 covers it with margin.
pub const CONFIG_JSON_CAP: usize = 128;

/// Codec / store errors. Distinct from [`crate::error::Error`] so the pure
/// codec stays free of firmware-wide types and is host-testable in isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvsError {
    /// Stored slice had an unexpected length for a fixed-size value.
    BadLength,
    /// Stored bytes were not valid UTF-8 (string values).
    NotUtf8,
    /// Output buffer too small to hold the encoded value.
    BufferTooSmall,
    /// JSON (de)serialization of a [`DeviceConfig`] failed.
    BadJson,
    /// Requested key was not present in flash.
    NotFound,
    /// Underlying flash I/O failed (Xtensa backend only).
    Flash,
}

// -----------------------------------------------------------------------------
// Pure codec — host-tested. Each function is the single source of truth for
// the on-flash byte layout of one value type.
// -----------------------------------------------------------------------------

/// Encode the 32-byte device secret. Returns the number of bytes written.
pub fn encode_secret(secret: &[u8; DEVICE_SECRET_LEN], out: &mut [u8]) -> Result<usize, NvsError> {
    let dst = out
        .get_mut(..DEVICE_SECRET_LEN)
        .ok_or(NvsError::BufferTooSmall)?;
    dst.copy_from_slice(secret);
    Ok(DEVICE_SECRET_LEN)
}

/// Decode the 32-byte device secret from its exact stored slice.
pub fn decode_secret(bytes: &[u8]) -> Result<[u8; DEVICE_SECRET_LEN], NvsError> {
    bytes.try_into().map_err(|_| NvsError::BadLength)
}

/// Encode the 16-byte device UUID.
pub fn encode_device_id(id: &[u8; DEVICE_ID_LEN], out: &mut [u8]) -> Result<usize, NvsError> {
    let dst = out
        .get_mut(..DEVICE_ID_LEN)
        .ok_or(NvsError::BufferTooSmall)?;
    dst.copy_from_slice(id);
    Ok(DEVICE_ID_LEN)
}

/// Decode the 16-byte device UUID from its exact stored slice.
pub fn decode_device_id(bytes: &[u8]) -> Result<[u8; DEVICE_ID_LEN], NvsError> {
    bytes.try_into().map_err(|_| NvsError::BadLength)
}

/// Encode a UTF-8 string value (SSID / passphrase) as raw bytes.
pub fn encode_str(value: &str, out: &mut [u8]) -> Result<usize, NvsError> {
    let src = value.as_bytes();
    let dst = out.get_mut(..src.len()).ok_or(NvsError::BufferTooSmall)?;
    dst.copy_from_slice(src);
    Ok(src.len())
}

/// Decode a fixed-capacity string value from its exact stored slice.
pub fn decode_str<const N: usize>(bytes: &[u8]) -> Result<String<N>, NvsError> {
    let s = core::str::from_utf8(bytes).map_err(|_| NvsError::NotUtf8)?;
    String::try_from(s).map_err(|_| NvsError::BadLength)
}

/// Encode a [`DeviceConfig`] as compact JSON.
pub fn encode_config(config: &DeviceConfig, out: &mut [u8]) -> Result<usize, NvsError> {
    serde_json_core::to_slice(config, out).map_err(|_| NvsError::BufferTooSmall)
}

/// Decode a [`DeviceConfig`] from its stored JSON slice.
pub fn decode_config(bytes: &[u8]) -> Result<DeviceConfig, NvsError> {
    serde_json_core::from_slice::<DeviceConfig>(bytes)
        .map(|(config, _)| config)
        .map_err(|_| NvsError::BadJson)
}

// -----------------------------------------------------------------------------
// Xtensa flash backend. Thin wrapper over `sequential-storage::map`; every
// value goes through the pure codec above. Cannot be host-compiled
// (esp-storage's flash driver is Xtensa-only) and is verified on the bench.
// -----------------------------------------------------------------------------

/// Flash range of the `nvs` partition (secrets + WiFi creds). Mirrors
/// `partitions.csv`: offset `0x9000`, size `0x6000`.
pub const NVS_PARTITION: core::ops::Range<u32> = 0x9000..(0x9000 + 0x6000);
/// Flash range of the `storage` partition (last config + outbox + cache
/// index). Mirrors `partitions.csv`: offset `0x620000`, size `0x100000`.
pub const STORAGE_PARTITION: core::ops::Range<u32> = 0x62_0000..(0x62_0000 + 0x10_0000);

#[cfg(all(target_arch = "xtensa", feature = "phase2-io"))]
mod flash_backend {
    use super::*;
    use embedded_storage_async::nor_flash::NorFlash;
    use sequential_storage::{
        cache::NoCache,
        map::{SerializationError, Value, fetch_item, store_item},
    };

    /// Scratch buffer for `sequential-storage`'s read-modify-write. Must be
    /// ≥ the largest value plus the page overhead; the config JSON is the
    /// largest value at ≤ 128 B, NVS flash pages are 4 KiB. 256 B is ample
    /// for the headers `sequential-storage` prepends.
    const SCRATCH_LEN: usize = 256;

    /// A typed handle over one flash partition.
    ///
    /// Generic over an **async** [`NorFlash`] `F` instead of hard-coding
    /// `esp_storage::FlashStorage`: `sequential-storage` v4 is async, and
    /// `FlashStorage` is a *blocking* `NorFlash`, so `main` lifts it with
    /// `embassy_embedded_hal::adapter::BlockingAsync<FlashStorage>` and hands
    /// the adapter in here. Decoupling from the concrete flash type also lets
    /// a host mock-flash exercise the store path. All ops are `block_on`'d —
    /// safe because the only intended `F` is a blocking-backed adapter whose
    /// futures resolve without yielding.
    pub struct NvsStore<F: NorFlash> {
        flash: F,
        range: core::ops::Range<u32>,
    }

    /// Newtype so we can `impl Value` for the config JSON path without the
    /// orphan rule biting (`DeviceConfig` lives in `proto`).
    struct ConfigValue(DeviceConfig);

    impl<'a> Value<'a> for ConfigValue {
        fn serialize_into(&self, buffer: &mut [u8]) -> Result<usize, SerializationError> {
            encode_config(&self.0, buffer).map_err(|_| SerializationError::BufferTooSmall)
        }

        fn deserialize_from(buffer: &'a [u8]) -> Result<Self, SerializationError>
        where
            Self: Sized,
        {
            decode_config(buffer)
                .map(ConfigValue)
                .map_err(|_| SerializationError::InvalidFormat)
        }
    }

    impl<F: NorFlash> NvsStore<F> {
        /// Wrap a flash device and the partition range it owns. Use
        /// [`NVS_PARTITION`] for secrets + WiFi creds, [`STORAGE_PARTITION`]
        /// for the last config.
        pub fn new(flash: F, range: core::ops::Range<u32>) -> Self {
            Self { flash, range }
        }

        fn fetch_raw<'b>(&mut self, key: u8, buf: &'b mut [u8]) -> Result<&'b [u8], NvsError> {
            // `&[u8]` implements `Value` as an identity view over the slice.
            let found: Option<&[u8]> = embassy_futures::block_on(fetch_item::<u8, &[u8], _>(
                &mut self.flash,
                self.range.clone(),
                &mut NoCache::new(),
                buf,
                &key,
            ))
            .map_err(|_| NvsError::Flash)?;
            found.ok_or(NvsError::NotFound)
        }

        fn store_raw(&mut self, key: u8, value: &[u8]) -> Result<(), NvsError> {
            let mut scratch = [0u8; SCRATCH_LEN];
            embassy_futures::block_on(store_item::<u8, &[u8], _>(
                &mut self.flash,
                self.range.clone(),
                &mut NoCache::new(),
                &mut scratch,
                &key,
                &value,
            ))
            .map_err(|_| NvsError::Flash)
        }

        /// Read the 32-byte device secret (factory-provisioned).
        pub fn device_secret(&mut self) -> Result<[u8; DEVICE_SECRET_LEN], NvsError> {
            let mut buf = [0u8; SCRATCH_LEN];
            decode_secret(self.fetch_raw(KEY_DEVICE_SECRET, &mut buf)?)
        }

        /// Read the 16-byte device UUID (set after `register`).
        pub fn device_id(&mut self) -> Result<[u8; DEVICE_ID_LEN], NvsError> {
            let mut buf = [0u8; SCRATCH_LEN];
            decode_device_id(self.fetch_raw(KEY_DEVICE_ID, &mut buf)?)
        }

        /// Persist the device UUID returned by `register`.
        pub fn set_device_id(&mut self, id: &[u8; DEVICE_ID_LEN]) -> Result<(), NvsError> {
            self.store_raw(KEY_DEVICE_ID, id)
        }

        /// Read the persisted WiFi SSID, if any.
        pub fn wifi_ssid(&mut self) -> Result<String<WIFI_SSID_CAP>, NvsError> {
            let mut buf = [0u8; SCRATCH_LEN];
            decode_str(self.fetch_raw(KEY_WIFI_SSID, &mut buf)?)
        }

        /// Read the persisted WiFi passphrase, if any.
        pub fn wifi_pass(&mut self) -> Result<String<WIFI_PASS_CAP>, NvsError> {
            let mut buf = [0u8; SCRATCH_LEN];
            decode_str(self.fetch_raw(KEY_WIFI_PASS, &mut buf)?)
        }

        /// Persist WiFi credentials (called by the BLE pairing flow, Phase 5).
        pub fn set_wifi_credentials(&mut self, ssid: &str, pass: &str) -> Result<(), NvsError> {
            self.store_raw(KEY_WIFI_SSID, ssid.as_bytes())?;
            self.store_raw(KEY_WIFI_PASS, pass.as_bytes())
        }

        /// Read the last `DeviceConfig` persisted after a sync.
        pub fn last_config(&mut self) -> Result<DeviceConfig, NvsError> {
            let mut buf = [0u8; SCRATCH_LEN];
            let found: Option<ConfigValue> =
                embassy_futures::block_on(fetch_item::<u8, ConfigValue, _>(
                    &mut self.flash,
                    self.range.clone(),
                    &mut NoCache::new(),
                    &mut buf,
                    &KEY_LAST_CONFIG,
                ))
                .map_err(|_| NvsError::Flash)?;
            found.map(|c| c.0).ok_or(NvsError::NotFound)
        }

        /// Persist the latest `DeviceConfig` from the backend.
        pub fn set_last_config(&mut self, config: &DeviceConfig) -> Result<(), NvsError> {
            let mut buf = [0u8; CONFIG_JSON_CAP];
            let n = encode_config(config, &mut buf)?;
            self.store_raw(KEY_LAST_CONFIG, &buf[..n])
        }
    }
}

#[cfg(all(target_arch = "xtensa", feature = "phase2-io"))]
pub use flash_backend::NvsStore;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_round_trips_byte_for_byte() {
        let secret: [u8; DEVICE_SECRET_LEN] = core::array::from_fn(|i| (i * 7 + 3) as u8);
        let mut buf = [0u8; 64];
        let n = encode_secret(&secret, &mut buf).unwrap();
        assert_eq!(n, DEVICE_SECRET_LEN);
        assert_eq!(decode_secret(&buf[..n]).unwrap(), secret);
    }

    #[test]
    fn secret_decode_rejects_wrong_length() {
        assert_eq!(decode_secret(&[0u8; 31]), Err(NvsError::BadLength));
        assert_eq!(decode_secret(&[0u8; 33]), Err(NvsError::BadLength));
    }

    #[test]
    fn device_id_round_trips() {
        let id: [u8; DEVICE_ID_LEN] = core::array::from_fn(|i| i as u8);
        let mut buf = [0u8; 16];
        let n = encode_device_id(&id, &mut buf).unwrap();
        assert_eq!(decode_device_id(&buf[..n]).unwrap(), id);
    }

    #[test]
    fn str_round_trips_and_enforces_capacity() {
        let mut buf = [0u8; 64];
        let n = encode_str("home-wifi", &mut buf).unwrap();
        let back: String<WIFI_SSID_CAP> = decode_str(&buf[..n]).unwrap();
        assert_eq!(back.as_str(), "home-wifi");
    }

    #[test]
    fn str_decode_rejects_overflowing_capacity() {
        // 40 bytes into a 33-byte SSID slot must error, not truncate.
        let long = [b'a'; 40];
        let res: Result<String<WIFI_SSID_CAP>, _> = decode_str(&long);
        assert_eq!(res, Err(NvsError::BadLength));
    }

    #[test]
    fn str_decode_rejects_invalid_utf8() {
        let res: Result<String<WIFI_SSID_CAP>, _> = decode_str(&[0xff, 0xfe]);
        assert_eq!(res, Err(NvsError::NotUtf8));
    }

    #[test]
    fn config_round_trips_through_json() {
        let config = DeviceConfig {
            light_sleep_after_sec: 30,
            deep_sleep_after_sec: 300,
            volume_max: 80,
            led_brightness: Some(50),
        };
        let mut buf = [0u8; CONFIG_JSON_CAP];
        let n = encode_config(&config, &mut buf).unwrap();
        assert_eq!(decode_config(&buf[..n]).unwrap(), config);
    }

    #[test]
    fn config_round_trips_without_optional_brightness() {
        let config = DeviceConfig {
            light_sleep_after_sec: 5,
            deep_sleep_after_sec: 60,
            volume_max: 100,
            led_brightness: None,
        };
        let mut buf = [0u8; CONFIG_JSON_CAP];
        let n = encode_config(&config, &mut buf).unwrap();
        assert_eq!(decode_config(&buf[..n]).unwrap(), config);
    }

    #[test]
    fn config_json_cap_holds_largest_shape() {
        let config = DeviceConfig {
            light_sleep_after_sec: u32::MAX,
            deep_sleep_after_sec: u32::MAX,
            volume_max: 100,
            led_brightness: Some(100),
        };
        let mut buf = [0u8; CONFIG_JSON_CAP];
        assert!(encode_config(&config, &mut buf).is_ok());
    }
}
