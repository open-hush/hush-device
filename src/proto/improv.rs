//! Improv Wi-Fi BLE protocol — pure, `no_std`, allocation-free core.
//!
//! This module implements the wire format and state machine of the
//! [Improv Wi-Fi BLE standard](https://www.improv-wifi.com/ble/) — the
//! open provisioning protocol the mobile app (`hush-app`) drives to hand
//! the device its Wi-Fi credentials without a cable. It is deliberately
//! split from the BLE radio / GATT bring-up (`crate::hw::ble`,
//! `crate::tasks::ble`, both Xtensa-only) so that the parts that MUST be
//! byte-for-byte correct against the app — RPC framing, checksums, state
//! transitions — are pure logic exercised on the host with
//! `cargo test --features mock-hardware`.
//!
//! ## Why this is the load-bearing file
//!
//! The GATT plumbing can be re-derived; a wrong checksum byte or an
//! off-by-one in the SSID/password sub-framing cannot — it silently
//! bricks onboarding for every unit in the field. Everything in here is
//! covered by host tests at the bottom of the file.
//!
//! ## Coordination point (issue OPE-54)
//!
//! The service / characteristic UUIDs and the RPC byte layout below are
//! the contract `hush-app/lib/ble/improv.ts` consumes. They follow the
//! published Improv standard verbatim (no Hush-specific divergence), so
//! the app side can use any off-the-shelf Improv client. If the spec and
//! this file ever disagree, the spec wins and this is a bug.

use heapless::{String, Vec};

// -----------------------------------------------------------------------------
// UUIDs
// -----------------------------------------------------------------------------

/// Canonical (big-endian, string-order) 128-bit UUID — the 16 bytes in the
/// same order they appear in the hyphenated string form. The BLE layer is
/// responsible for any little-endian reversal its stack expects on the
/// wire; keeping the canonical order here makes the constants trivially
/// checkable against the published spec and the app's `improv.ts`.
pub type Uuid128 = [u8; 16];

/// Improv Wi-Fi service UUID `00467768-6228-2272-4663-277478268000`.
pub const SERVICE_UUID: Uuid128 = [
    0x00, 0x46, 0x77, 0x68, 0x62, 0x28, 0x22, 0x72, 0x46, 0x63, 0x27, 0x74, 0x78, 0x26, 0x80, 0x00,
];

/// `Current State` characteristic — read + notify, 1 byte ([`State`]).
/// `00467768-6228-2272-4663-277478268001`.
pub const CHAR_CURRENT_STATE_UUID: Uuid128 = with_suffix(0x01);
/// `Error State` characteristic — read + notify, 1 byte ([`ErrorState`]).
/// `00467768-6228-2272-4663-277478268002`.
pub const CHAR_ERROR_STATE_UUID: Uuid128 = with_suffix(0x02);
/// `RPC Command` characteristic — write, framed RPC packet.
/// `00467768-6228-2272-4663-277478268003`.
pub const CHAR_RPC_COMMAND_UUID: Uuid128 = with_suffix(0x03);
/// `RPC Result` characteristic — read + notify, framed RPC result.
/// `00467768-6228-2272-4663-277478268004`.
pub const CHAR_RPC_RESULT_UUID: Uuid128 = with_suffix(0x04);
/// `Capabilities` characteristic — read, 1 byte bitfield ([`CAPABILITIES`]).
/// `00467768-6228-2272-4663-277478268005`.
pub const CHAR_CAPABILITIES_UUID: Uuid128 = with_suffix(0x05);

/// All Improv characteristic UUIDs differ from the service UUID only in
/// their last byte; this keeps the table above honest and DRY.
const fn with_suffix(last: u8) -> Uuid128 {
    let mut u = SERVICE_UUID;
    u[15] = last;
    u
}

/// Capabilities bitfield advertised on the `Capabilities` characteristic.
/// Bit 0 = "device supports the `Identify` RPC". We leave it clear: Phase
/// 5 onboarding needs no physical identify step, and not advertising it
/// keeps the app from offering a button that does nothing. The `Identify`
/// RPC is still handled gracefully if a client sends it anyway.
pub const CAPABILITIES: u8 = 0x00;

// -----------------------------------------------------------------------------
// State / error enums (the single-byte characteristic values)
// -----------------------------------------------------------------------------

/// `Current State` characteristic values (Improv spec §"Current State").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    /// Device needs the user to authorize provisioning (e.g. press a
    /// button) before it will accept credentials. Unused in Phase 5 — we
    /// boot pairing mode straight into [`State::Authorized`] — but kept
    /// for completeness and a future physical-authorization gate.
    AuthorizationRequired = 0x01,
    /// Ready to receive credentials.
    Authorized = 0x02,
    /// Credentials received; attempting to join the AP.
    Provisioning = 0x03,
    /// Successfully joined the AP (and, for Hush, registered with the
    /// backend). Terminal: the BLE stack is torn down after this.
    Provisioned = 0x04,
}

impl State {
    pub const fn as_byte(self) -> u8 {
        self as u8
    }
}

/// `Error State` characteristic values (Improv spec §"Error State").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorState {
    None = 0x00,
    InvalidRpcPacket = 0x01,
    UnknownRpcCommand = 0x02,
    UnableToConnect = 0x03,
    NotAuthorized = 0x04,
    Unknown = 0xFF,
}

impl ErrorState {
    pub const fn as_byte(self) -> u8 {
        self as u8
    }
}

// -----------------------------------------------------------------------------
// RPC command parsing (app → device, written to CHAR_RPC_COMMAND)
// -----------------------------------------------------------------------------

/// SSID/password capacities. Match `crate::hw::wifi::WIFI_SSID_MAX_LEN` /
/// `WIFI_PSK_MAX_LEN` (32 / 64, the 802.11 + WPA2 maxima). Duplicated as
/// locals because this module is also compiled into the host-test lib,
/// which does not pull in the Xtensa-only `crate::hw` tree.
pub const SSID_MAX_LEN: usize = 32;
pub const PSK_MAX_LEN: usize = 64;

/// Improv RPC command identifiers.
const CMD_SEND_WIFI_SETTINGS: u8 = 0x01;
const CMD_IDENTIFY: u8 = 0x02;

/// Largest possible incoming RPC command packet: command(1) + length(1) +
/// up to 255 data bytes (the length field is a `u8`) + checksum(1). The
/// BLE task sizes its receive buffer to this so a maximal write never
/// truncates before [`parse_command`] sees it.
pub const RPC_COMMAND_MAX: usize = 1 + 1 + 255 + 1;

/// Wi-Fi credentials decoded from a `SendWifiSettings` RPC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiCredentials {
    pub ssid: String<SSID_MAX_LEN>,
    pub password: String<PSK_MAX_LEN>,
}

/// A successfully parsed RPC command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rpc {
    /// `SendWifiSettings` (0x01): join this network.
    SendWifiSettings(WifiCredentials),
    /// `Identify` (0x02): blink so the user can spot the device.
    Identify,
}

/// Why an RPC packet was rejected. Maps 1:1 onto the [`ErrorState`] the
/// device must publish back to the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcError {
    /// Truncated, over-long, bad checksum, bad sub-lengths, or non-UTF-8 /
    /// over-capacity SSID or password → `InvalidRpcPacket`.
    InvalidPacket,
    /// Well-formed framing but a command byte we don't implement →
    /// `UnknownRpcCommand`.
    UnknownCommand,
}

impl RpcError {
    pub const fn to_error_state(self) -> ErrorState {
        match self {
            RpcError::InvalidPacket => ErrorState::InvalidRpcPacket,
            RpcError::UnknownCommand => ErrorState::UnknownRpcCommand,
        }
    }
}

/// Improv checksum: the unsigned-8 sum of every byte that precedes the
/// trailing checksum byte. Used by both the command and result framings.
fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b))
}

/// Parse a framed Improv RPC packet:
///
/// ```text
/// byte 0      : command
/// byte 1      : data length (N)
/// byte 2..2+N : data
/// byte 2+N    : checksum  = (sum of bytes 0..2+N) mod 256
/// ```
///
/// The total packet length is therefore always `N + 3`. Anything else is
/// an [`RpcError::InvalidPacket`].
pub fn parse_command(packet: &[u8]) -> Result<Rpc, RpcError> {
    // Smallest valid packet is [cmd, 0, checksum] (a zero-data command).
    if packet.len() < 3 {
        return Err(RpcError::InvalidPacket);
    }
    let command = packet[0];
    let data_len = packet[1] as usize;

    // Length must match the framing exactly — reject both truncated and
    // trailing-garbage packets rather than guessing.
    let expected = data_len + 3;
    if packet.len() != expected {
        return Err(RpcError::InvalidPacket);
    }

    let body = &packet[..expected - 1]; // command + len + data
    let got = packet[expected - 1];
    if checksum(body) != got {
        return Err(RpcError::InvalidPacket);
    }

    let data = &packet[2..2 + data_len];
    match command {
        CMD_SEND_WIFI_SETTINGS => parse_wifi_settings(data).map(Rpc::SendWifiSettings),
        CMD_IDENTIFY => {
            // Identify carries no data; a non-empty body is malformed.
            if data.is_empty() {
                Ok(Rpc::Identify)
            } else {
                Err(RpcError::InvalidPacket)
            }
        }
        _ => Err(RpcError::UnknownCommand),
    }
}

/// Decode the `SendWifiSettings` payload:
///
/// ```text
/// byte 0          : ssid length (S)
/// byte 1..1+S     : ssid
/// byte 1+S        : password length (P)
/// byte 2+S..2+S+P : password
/// ```
fn parse_wifi_settings(data: &[u8]) -> Result<WifiCredentials, RpcError> {
    let ssid_len = *data.first().ok_or(RpcError::InvalidPacket)? as usize;
    let ssid_end = 1 + ssid_len;
    // Need at least one more byte after the SSID for the password length.
    if data.len() < ssid_end + 1 {
        return Err(RpcError::InvalidPacket);
    }
    let ssid_bytes = &data[1..ssid_end];

    let pass_len = data[ssid_end] as usize;
    let pass_start = ssid_end + 1;
    let pass_end = pass_start + pass_len;
    if data.len() != pass_end {
        return Err(RpcError::InvalidPacket);
    }
    let pass_bytes = &data[pass_start..pass_end];

    let ssid = str_from_utf8(ssid_bytes)?;
    let password = str_from_utf8(pass_bytes)?;
    Ok(WifiCredentials { ssid, password })
}

/// UTF-8 + capacity guarded conversion into a fixed-capacity heapless
/// `String`. Both failures collapse to `InvalidPacket`: an SSID that is
/// not valid UTF-8 or longer than the radio accepts is unusable either
/// way.
fn str_from_utf8<const N: usize>(bytes: &[u8]) -> Result<String<N>, RpcError> {
    let s = core::str::from_utf8(bytes).map_err(|_| RpcError::InvalidPacket)?;
    String::try_from(s).map_err(|_| RpcError::InvalidPacket)
}

// -----------------------------------------------------------------------------
// RPC result building (device → app, notified on CHAR_RPC_RESULT)
// -----------------------------------------------------------------------------

/// Maximum size of a built RPC result packet. The only result we send is
/// the `SendWifiSettings` success response carrying at most one redirect
/// URL string: command(1) + len(1) + [url_len(1) + url] + checksum(1).
/// A 128-byte ceiling leaves room for a generous URL well past anything
/// the dashboard emits.
pub const RPC_RESULT_MAX: usize = 128;

/// A built RPC result packet, ready to write to the `RPC Result`
/// characteristic.
pub type RpcResult = Vec<u8, RPC_RESULT_MAX>;

/// Errors building a result packet (only over-capacity is possible).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultTooLong;

/// Build the `SendWifiSettings` success result. The Improv spec lets the
/// device return a list of strings; the app treats the first as a URL to
/// open once provisioning succeeds. We send either zero strings (empty
/// list) or a single redirect URL.
///
/// ```text
/// byte 0  : command (echo of 0x01)
/// byte 1  : data length
/// data    : repeated [str_len(1), str_bytes...]
/// last    : checksum
/// ```
pub fn build_wifi_success(redirect_url: Option<&str>) -> Result<RpcResult, ResultTooLong> {
    let mut data: Vec<u8, RPC_RESULT_MAX> = Vec::new();
    if let Some(url) = redirect_url {
        let bytes = url.as_bytes();
        let len: u8 = bytes.len().try_into().map_err(|_| ResultTooLong)?;
        data.push(len).map_err(|_| ResultTooLong)?;
        data.extend_from_slice(bytes).map_err(|_| ResultTooLong)?;
    }
    let data_len: u8 = data.len().try_into().map_err(|_| ResultTooLong)?;

    let mut out: RpcResult = Vec::new();
    out.push(CMD_SEND_WIFI_SETTINGS)
        .map_err(|_| ResultTooLong)?;
    out.push(data_len).map_err(|_| ResultTooLong)?;
    out.extend_from_slice(&data).map_err(|_| ResultTooLong)?;
    let cs = checksum(&out);
    out.push(cs).map_err(|_| ResultTooLong)?;
    Ok(out)
}

// -----------------------------------------------------------------------------
// State machine
// -----------------------------------------------------------------------------

/// What the protocol layer asks the BLE task to do in response to an
/// input. The task is the only place that touches the radio, the Wi-Fi
/// controller, NVS and the LED; the state machine stays pure so it can be
/// fully host-tested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing for the task to do beyond re-notifying the (already
    /// updated) state / error characteristics.
    None,
    /// Credentials accepted — try to join this AP, then register. The
    /// task reports back via [`Improv::on_provision_result`].
    Provision(WifiCredentials),
    /// `Identify` RPC received — blink the LED so the user can spot us.
    Identify,
    /// Provisioning finished successfully. The task should notify the
    /// (already built) result packet, then tear the BLE stack down.
    Provisioned(RpcResult),
}

/// The Improv provisioning state machine. Holds the two single-byte
/// characteristic values and advances them as RPCs and provisioning
/// outcomes arrive. No I/O, no allocation, no `await`.
#[derive(Debug, Clone)]
pub struct Improv {
    state: State,
    error: ErrorState,
}

impl Default for Improv {
    fn default() -> Self {
        Self::new()
    }
}

impl Improv {
    /// Enter pairing mode. Phase 5 needs no physical authorization step,
    /// so we boot straight into [`State::Authorized`] (ready for
    /// credentials) with no error.
    pub const fn new() -> Self {
        Self {
            state: State::Authorized,
            error: ErrorState::None,
        }
    }

    pub const fn state(&self) -> State {
        self.state
    }

    pub const fn error(&self) -> ErrorState {
        self.error
    }

    /// Handle a raw write to the `RPC Command` characteristic.
    ///
    /// Clears the error on entry (each command starts a fresh attempt),
    /// then advances state and returns the [`Action`] the task must
    /// perform. A parse failure sets the matching [`ErrorState`] and
    /// leaves the device in [`State::Authorized`] so the app can retry.
    pub fn on_rpc(&mut self, packet: &[u8]) -> Action {
        // Ignore writes mid-provisioning: a connect attempt is already in
        // flight and the result will drive the next transition. Without
        // this guard a impatient double-write could race the controller.
        if self.state == State::Provisioning {
            return Action::None;
        }

        self.error = ErrorState::None;
        match parse_command(packet) {
            Ok(Rpc::SendWifiSettings(creds)) => {
                self.state = State::Provisioning;
                Action::Provision(creds)
            }
            Ok(Rpc::Identify) => Action::Identify,
            Err(err) => {
                self.error = err.to_error_state();
                self.state = State::Authorized;
                Action::None
            }
        }
    }

    /// Report the outcome of the provisioning attempt the task ran for an
    /// earlier [`Action::Provision`].
    ///
    /// On success: publish [`State::Provisioned`] and return the built
    /// result packet for the task to notify before tearing BLE down. On
    /// failure: publish `UnableToConnect` and fall back to
    /// [`State::Authorized`] so the user can re-enter credentials.
    pub fn on_provision_result(&mut self, outcome: ProvisionOutcome) -> Action {
        match outcome {
            ProvisionOutcome::Success { redirect_url } => {
                self.state = State::Provisioned;
                self.error = ErrorState::None;
                // A too-long URL is a programming error on our side, not a
                // protocol failure: degrade to an empty (but valid) result
                // rather than stranding the app waiting for a notify.
                let result = build_wifi_success(redirect_url)
                    .unwrap_or_else(|_| build_wifi_success(None).unwrap_or_default());
                Action::Provisioned(result)
            }
            ProvisionOutcome::Failed => {
                self.state = State::Authorized;
                self.error = ErrorState::UnableToConnect;
                Action::None
            }
        }
    }
}

/// Result of the task's "join AP + register with backend" attempt, fed
/// back into [`Improv::on_provision_result`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionOutcome<'a> {
    /// Joined the AP and registered. `redirect_url`, if present, is sent
    /// to the app as the post-provisioning URL to open.
    Success { redirect_url: Option<&'a str> },
    /// Could not join the AP, or registration failed.
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    /// Helper: frame an Improv RPC command with a correct checksum.
    fn frame(command: u8, data: &[u8]) -> std::vec::Vec<u8> {
        let mut p = std::vec::Vec::new();
        p.push(command);
        p.push(data.len() as u8);
        p.extend_from_slice(data);
        let cs = data
            .iter()
            .fold(command.wrapping_add(data.len() as u8), |a, b| {
                a.wrapping_add(*b)
            });
        p.push(cs);
        p
    }

    /// Helper: build the `SendWifiSettings` data payload.
    fn wifi_data(ssid: &str, pass: &str) -> std::vec::Vec<u8> {
        let mut d = std::vec::Vec::new();
        d.push(ssid.len() as u8);
        d.extend_from_slice(ssid.as_bytes());
        d.push(pass.len() as u8);
        d.extend_from_slice(pass.as_bytes());
        d
    }

    // ---- UUIDs ----

    #[test]
    fn service_uuid_matches_spec_string() {
        // 00467768-6228-2272-4663-277478268000
        assert_eq!(
            SERVICE_UUID,
            [
                0x00, 0x46, 0x77, 0x68, 0x62, 0x28, 0x22, 0x72, 0x46, 0x63, 0x27, 0x74, 0x78, 0x26,
                0x80, 0x00
            ]
        );
    }

    #[test]
    fn characteristics_differ_only_in_last_byte() {
        for (uuid, last) in [
            (CHAR_CURRENT_STATE_UUID, 0x01),
            (CHAR_ERROR_STATE_UUID, 0x02),
            (CHAR_RPC_COMMAND_UUID, 0x03),
            (CHAR_RPC_RESULT_UUID, 0x04),
            (CHAR_CAPABILITIES_UUID, 0x05),
        ] {
            assert_eq!(&uuid[..15], &SERVICE_UUID[..15]);
            assert_eq!(uuid[15], last);
        }
    }

    // ---- checksum ----

    #[test]
    fn checksum_wraps_at_256() {
        assert_eq!(checksum(&[0xFF, 0x02]), 0x01);
        assert_eq!(checksum(&[]), 0x00);
    }

    // ---- parse: wifi settings ----

    #[test]
    fn parses_valid_wifi_settings() {
        let packet = frame(
            CMD_SEND_WIFI_SETTINGS,
            &wifi_data("home-net", "s3cr3t-pass"),
        );
        let rpc = parse_command(&packet).unwrap();
        match rpc {
            Rpc::SendWifiSettings(c) => {
                assert_eq!(c.ssid.as_str(), "home-net");
                assert_eq!(c.password.as_str(), "s3cr3t-pass");
            }
            _ => panic!("expected SendWifiSettings"),
        }
    }

    #[test]
    fn parses_empty_password_open_network() {
        let packet = frame(CMD_SEND_WIFI_SETTINGS, &wifi_data("open-ap", ""));
        match parse_command(&packet).unwrap() {
            Rpc::SendWifiSettings(c) => {
                assert_eq!(c.ssid.as_str(), "open-ap");
                assert!(c.password.is_empty());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_max_length_ssid_and_password() {
        let ssid = "x".repeat(SSID_MAX_LEN);
        let pass = "y".repeat(PSK_MAX_LEN);
        let packet = frame(CMD_SEND_WIFI_SETTINGS, &wifi_data(&ssid, &pass));
        match parse_command(&packet).unwrap() {
            Rpc::SendWifiSettings(c) => {
                assert_eq!(c.ssid.len(), SSID_MAX_LEN);
                assert_eq!(c.password.len(), PSK_MAX_LEN);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn rejects_bad_checksum() {
        let mut packet = frame(CMD_SEND_WIFI_SETTINGS, &wifi_data("net", "pw"));
        let last = packet.len() - 1;
        packet[last] ^= 0xFF;
        assert_eq!(parse_command(&packet), Err(RpcError::InvalidPacket));
    }

    #[test]
    fn rejects_truncated_packet() {
        let packet = frame(CMD_SEND_WIFI_SETTINGS, &wifi_data("net", "pw"));
        assert_eq!(
            parse_command(&packet[..packet.len() - 1]),
            Err(RpcError::InvalidPacket)
        );
        assert_eq!(parse_command(&[]), Err(RpcError::InvalidPacket));
        assert_eq!(parse_command(&[0x01, 0x00]), Err(RpcError::InvalidPacket));
    }

    #[test]
    fn rejects_trailing_garbage() {
        let mut packet = frame(CMD_SEND_WIFI_SETTINGS, &wifi_data("net", "pw"));
        packet.push(0x00); // length field no longer matches packet size
        assert_eq!(parse_command(&packet), Err(RpcError::InvalidPacket));
    }

    #[test]
    fn rejects_ssid_length_overrunning_buffer() {
        // ssid_len claims 50 bytes but only 3 follow.
        let data = vec![50u8, b'a', b'b', b'c'];
        let packet = frame(CMD_SEND_WIFI_SETTINGS, &data);
        assert_eq!(parse_command(&packet), Err(RpcError::InvalidPacket));
    }

    #[test]
    fn rejects_password_length_mismatch() {
        // ssid ok, password length claims more than provided.
        let data = vec![3u8, b'n', b'e', b't', 5u8, b'p', b'w'];
        let packet = frame(CMD_SEND_WIFI_SETTINGS, &data);
        assert_eq!(parse_command(&packet), Err(RpcError::InvalidPacket));
    }

    #[test]
    fn rejects_ssid_over_capacity() {
        let ssid = "x".repeat(SSID_MAX_LEN + 1);
        let packet = frame(CMD_SEND_WIFI_SETTINGS, &wifi_data(&ssid, "pw"));
        assert_eq!(parse_command(&packet), Err(RpcError::InvalidPacket));
    }

    #[test]
    fn rejects_non_utf8_ssid() {
        let data = vec![2u8, 0xFF, 0xFE, 0u8];
        let packet = frame(CMD_SEND_WIFI_SETTINGS, &data);
        assert_eq!(parse_command(&packet), Err(RpcError::InvalidPacket));
    }

    // ---- parse: identify / unknown ----

    #[test]
    fn parses_identify() {
        let packet = frame(CMD_IDENTIFY, &[]);
        assert_eq!(parse_command(&packet), Ok(Rpc::Identify));
    }

    #[test]
    fn rejects_identify_with_data() {
        let packet = frame(CMD_IDENTIFY, &[0x01]);
        assert_eq!(parse_command(&packet), Err(RpcError::InvalidPacket));
    }

    #[test]
    fn unknown_command_is_distinct_from_invalid() {
        let packet = frame(0x7F, &[]);
        assert_eq!(parse_command(&packet), Err(RpcError::UnknownCommand));
    }

    #[test]
    fn rpc_error_maps_to_error_state() {
        assert_eq!(
            RpcError::InvalidPacket.to_error_state(),
            ErrorState::InvalidRpcPacket
        );
        assert_eq!(
            RpcError::UnknownCommand.to_error_state(),
            ErrorState::UnknownRpcCommand
        );
    }

    // ---- result building ----

    #[test]
    fn builds_success_result_with_url() {
        let out = build_wifi_success(Some("https://app.open-hush.com/claim")).unwrap();
        // command, data_len, [url_len, url...], checksum
        assert_eq!(out[0], CMD_SEND_WIFI_SETTINGS);
        let url = "https://app.open-hush.com/claim";
        assert_eq!(out[1] as usize, url.len() + 1);
        assert_eq!(out[2] as usize, url.len());
        assert_eq!(&out[3..3 + url.len()], url.as_bytes());
        // checksum verifies against the body.
        let cs = out[out.len() - 1];
        assert_eq!(super::checksum(&out[..out.len() - 1]), cs);
    }

    #[test]
    fn builds_empty_success_result() {
        let out = build_wifi_success(None).unwrap();
        assert_eq!(out[0], CMD_SEND_WIFI_SETTINGS);
        assert_eq!(out[1], 0);
        // [cmd, 0, checksum]; checksum = cmd + 0.
        assert_eq!(out.len(), 3);
        assert_eq!(out[2], CMD_SEND_WIFI_SETTINGS);
    }

    #[test]
    fn rejects_oversized_redirect_url() {
        let url = "x".repeat(RPC_RESULT_MAX);
        assert_eq!(build_wifi_success(Some(&url)), Err(ResultTooLong));
    }

    // ---- state machine ----

    #[test]
    fn boots_authorized_no_error() {
        let sm = Improv::new();
        assert_eq!(sm.state(), State::Authorized);
        assert_eq!(sm.error(), ErrorState::None);
    }

    #[test]
    fn happy_path_authorized_to_provisioned() {
        let mut sm = Improv::new();
        let packet = frame(CMD_SEND_WIFI_SETTINGS, &wifi_data("net", "pw"));

        let action = sm.on_rpc(&packet);
        assert_eq!(sm.state(), State::Provisioning);
        assert_eq!(sm.error(), ErrorState::None);
        match action {
            Action::Provision(c) => {
                assert_eq!(c.ssid.as_str(), "net");
                assert_eq!(c.password.as_str(), "pw");
            }
            _ => panic!("expected Provision"),
        }

        let action = sm.on_provision_result(ProvisionOutcome::Success {
            redirect_url: Some("https://app.open-hush.com"),
        });
        assert_eq!(sm.state(), State::Provisioned);
        assert_eq!(sm.error(), ErrorState::None);
        assert!(matches!(action, Action::Provisioned(_)));
    }

    #[test]
    fn connect_failure_returns_to_authorized_with_error() {
        let mut sm = Improv::new();
        let packet = frame(CMD_SEND_WIFI_SETTINGS, &wifi_data("net", "wrong"));
        sm.on_rpc(&packet);
        let action = sm.on_provision_result(ProvisionOutcome::Failed);
        assert_eq!(sm.state(), State::Authorized);
        assert_eq!(sm.error(), ErrorState::UnableToConnect);
        assert_eq!(action, Action::None);
    }

    #[test]
    fn invalid_rpc_sets_error_and_stays_authorized() {
        let mut sm = Improv::new();
        let action = sm.on_rpc(&[0x01, 0x00]); // truncated
        assert_eq!(action, Action::None);
        assert_eq!(sm.state(), State::Authorized);
        assert_eq!(sm.error(), ErrorState::InvalidRpcPacket);
    }

    #[test]
    fn unknown_command_sets_unknown_error() {
        let mut sm = Improv::new();
        let packet = frame(0x55, &[]);
        sm.on_rpc(&packet);
        assert_eq!(sm.error(), ErrorState::UnknownRpcCommand);
        assert_eq!(sm.state(), State::Authorized);
    }

    #[test]
    fn identify_does_not_change_state() {
        let mut sm = Improv::new();
        let action = sm.on_rpc(&frame(CMD_IDENTIFY, &[]));
        assert_eq!(action, Action::Identify);
        assert_eq!(sm.state(), State::Authorized);
        assert_eq!(sm.error(), ErrorState::None);
    }

    #[test]
    fn writes_are_ignored_while_provisioning() {
        let mut sm = Improv::new();
        sm.on_rpc(&frame(CMD_SEND_WIFI_SETTINGS, &wifi_data("net", "pw")));
        assert_eq!(sm.state(), State::Provisioning);
        // A second write while the first attempt is in flight is a no-op.
        let action = sm.on_rpc(&frame(CMD_SEND_WIFI_SETTINGS, &wifi_data("other", "x")));
        assert_eq!(action, Action::None);
        assert_eq!(sm.state(), State::Provisioning);
    }

    #[test]
    fn error_clears_on_next_valid_attempt() {
        let mut sm = Improv::new();
        sm.on_rpc(&frame(0x55, &[])); // unknown → error set
        assert_eq!(sm.error(), ErrorState::UnknownRpcCommand);
        sm.on_rpc(&frame(CMD_SEND_WIFI_SETTINGS, &wifi_data("net", "pw")));
        assert_eq!(sm.error(), ErrorState::None);
        assert_eq!(sm.state(), State::Provisioning);
    }

    #[test]
    fn state_and_error_byte_values_match_spec() {
        assert_eq!(State::AuthorizationRequired.as_byte(), 0x01);
        assert_eq!(State::Authorized.as_byte(), 0x02);
        assert_eq!(State::Provisioning.as_byte(), 0x03);
        assert_eq!(State::Provisioned.as_byte(), 0x04);
        assert_eq!(ErrorState::None.as_byte(), 0x00);
        assert_eq!(ErrorState::InvalidRpcPacket.as_byte(), 0x01);
        assert_eq!(ErrorState::UnknownRpcCommand.as_byte(), 0x02);
        assert_eq!(ErrorState::UnableToConnect.as_byte(), 0x03);
        assert_eq!(ErrorState::NotAuthorized.as_byte(), 0x04);
        assert_eq!(ErrorState::Unknown.as_byte(), 0xFF);
    }
}
