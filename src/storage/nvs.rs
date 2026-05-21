//! Typed NVS access over `sequential-storage` + `esp-storage`.
//!
//! Keys:
//!
//! | Key | Type | Set at |
//! |---|---|---|
//! | `device_secret` | `[u8; 32]` | Factory provisioning, never rotated |
//! | `device_id`     | `[u8; 16]` (UUID) | After `POST /v1/device/register` |
//! | `wifi_ssid`     | heapless::String<33> | BLE pairing |
//! | `wifi_pass`     | heapless::String<64> | BLE pairing |
//! | `last_config`   | serialised `DeviceConfig` | After every sync |
//!
//! TODO(phase-2): implement typed get/set helpers and a one-time-init that
//! detects an empty NVS and refuses to boot (the device has not been
//! provisioned).
