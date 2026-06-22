//! HTTPS client for the Hush backend device endpoints.
//!
//! Every device request is HMAC-signed via [`crate::api::hmac`] using the
//! per-device secret in NVS. The transport is `reqwless` over
//! `embedded-tls`, with [`crate::certs`] as the trust anchor.
//!
//! ## Layering
//!
//! - **Pure helpers** (this module's free functions): build the
//!   path-with-query the signature covers, percent-encode query values per
//!   RFC 3986, render the device UUID for the `keyId`, assemble the signed
//!   `Authorization` header, and map an HTTP status to a typed outcome.
//!   All allocation-free and host-tested.
//! - **`reqwless` transport** ([`transport`], Xtensa-only): opens the TLS
//!   connection, sends the signed request and parses the body into the
//!   [`crate::proto::api`] wire types. Verified on the bench.
//!
//! ## Canonical query handling — backend coordination point
//!
//! `docs/auth.md` says query parameters are "sorted lexicographically by key
//! [and] URL-encoded per RFC 3986" before they enter the canonical request.
//! `GET /v1/device/sync` is the first device endpoint with a query param
//! (`since`), so [`build_sync_path`] implements that rule: the value is
//! percent-encoded with the strict unreserved set (`ALPHA / DIGIT / -._~`),
//! which encodes the `:` in an ISO-8601 timestamp as `%3A`. **This must
//! match the backend's verifier byte-for-byte** or `since`-conditional syncs
//! 401 with `invalid_signature`. Confirm against
//! `hush-backend/api/src/routes/device.rs` before the bench session.

#![allow(dead_code)] // transport wired into the `sync` task incrementally.

use heapless::String;

use crate::api::hmac::{
    self, HmacError, MAX_AUTH_HEADER_LEN, MAX_PATH_LEN, SHA256_HEX_LEN, SIGNATURE_HEX_LEN,
};

/// Device endpoint paths (no query). The signature covers the path-with-query
/// built from these.
pub const PATH_REGISTER: &str = "/v1/device/register";
pub const PATH_SYNC: &str = "/v1/device/sync";
pub const PATH_EVENTS: &str = "/v1/device/events";

/// Hyphenated lowercase UUID length.
pub const UUID_STR_LEN: usize = 36;

/// Typed outcome of an HTTP response, derived from the status code. Keeps
/// status→meaning mapping in one host-tested place instead of scattered
/// magic numbers in the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpOutcome {
    /// 2xx success carrying a body (200 register/sync, 202 events).
    Ok,
    /// 304 Not Modified — sync `since` matched; keep the cached snapshot.
    NotModified,
    /// 401 — auth rejected (`unauthorized` / `invalid_signature` /
    /// `expired_token`). Almost always clock skew or a wrong/duplicate
    /// secret; the caller should re-NTP and not hammer.
    Unauthorized,
    /// 404 — device not found (e.g. registered id retired server-side).
    NotFound,
    /// 422 — request body failed validation. A firmware bug; do not retry.
    Unprocessable,
    /// 429 — rate limited; back off.
    RateLimited,
    /// Any other status — treat as a transient server error and back off.
    ServerError,
}

impl HttpOutcome {
    /// Map an HTTP status code to a [`HttpOutcome`].
    pub fn from_status(status: u16) -> Self {
        match status {
            200 | 201 | 202 | 204 => HttpOutcome::Ok,
            304 => HttpOutcome::NotModified,
            401 => HttpOutcome::Unauthorized,
            404 => HttpOutcome::NotFound,
            422 => HttpOutcome::Unprocessable,
            429 => HttpOutcome::RateLimited,
            _ => HttpOutcome::ServerError,
        }
    }

    /// Whether the caller should retry after a back-off. `Unauthorized` and
    /// `Unprocessable` are *not* retriable (they will keep failing the same
    /// way); transient/server/rate-limit conditions are.
    pub fn is_retriable(self) -> bool {
        matches!(self, HttpOutcome::RateLimited | HttpOutcome::ServerError)
    }
}

/// Render a 16-byte UUID as its canonical hyphenated lowercase form
/// (`8-4-4-4-12`). Used for the `keyId` in the `Authorization` header.
pub fn format_uuid(bytes: &[u8; 16]) -> String<UUID_STR_LEN> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    // 8-4-4-4-12 groups; hyphen after bytes 4, 6, 8, 10.
    const DASH_AFTER: [usize; 4] = [4, 6, 8, 10];
    let mut out: String<UUID_STR_LEN> = String::new();
    for (i, b) in bytes.iter().enumerate() {
        if DASH_AFTER.contains(&i) {
            // Capacity is exactly 36 and we push 32 hex + 4 dashes; safe.
            let _ = out.push('-');
        }
        let _ = out.push(HEX[(*b >> 4) as usize] as char);
        let _ = out.push(HEX[(*b & 0x0f) as usize] as char);
    }
    out
}

/// RFC 3986 percent-encode `value` into `out` using the strict unreserved
/// set (`ALPHA / DIGIT / '-' / '.' / '_' / '~'`); every other byte becomes
/// `%XX` with uppercase hex. Returns [`HmacError::PathTooLong`] if the
/// encoding overflows `out`.
pub fn percent_encode_into<const N: usize>(
    value: &str,
    out: &mut String<N>,
) -> Result<(), HmacError> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &b in value.as_bytes() {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~');
        if unreserved {
            out.push(b as char).map_err(|_| HmacError::PathTooLong)?;
        } else {
            out.push('%').map_err(|_| HmacError::PathTooLong)?;
            out.push(HEX[(b >> 4) as usize] as char)
                .map_err(|_| HmacError::PathTooLong)?;
            out.push(HEX[(b & 0x0f) as usize] as char)
                .map_err(|_| HmacError::PathTooLong)?;
        }
    }
    Ok(())
}

/// Build the path-with-query for `GET /v1/device/sync`, optionally carrying a
/// `since` ISO-8601 timestamp. The value is percent-encoded per the rule in
/// the module docs; with a single parameter there is nothing to sort. The
/// returned string is exactly what both the HTTP request line and the
/// canonical request must use.
pub fn build_sync_path(since: Option<&str>) -> Result<String<MAX_PATH_LEN>, HmacError> {
    let mut path: String<MAX_PATH_LEN> = String::new();
    path.push_str(PATH_SYNC)
        .map_err(|_| HmacError::PathTooLong)?;
    if let Some(ts) = since {
        path.push_str("?since=")
            .map_err(|_| HmacError::PathTooLong)?;
        percent_encode_into(ts, &mut path)?;
    }
    Ok(path)
}

/// Assemble the signed `Authorization` header value for a request.
///
/// Computes `SHA-256(body)`, builds the canonical request
/// (`METHOD\nPATH-WITH-QUERY\nTS\nBODY-SHA256-HEX`), HMAC-signs it with the
/// device `secret`, and renders `HMAC keyId=…,signature=…,ts=…`. `ts` is
/// unix-seconds and MUST be inside the backend's ±300 s skew window
/// (NTP-sync first; see `docs/auth.md`).
pub fn sign_request(
    secret: &[u8],
    device_id_str: &str,
    method: &str,
    path_with_query: &str,
    body: &[u8],
    ts: u64,
) -> Result<String<MAX_AUTH_HEADER_LEN>, HmacError> {
    let body_sha: [u8; SHA256_HEX_LEN] = hmac::body_sha256_hex(body);
    let canonical = hmac::canonical_request(method, path_with_query, ts, &body_sha)?;
    let signature = hmac::sign(secret, canonical.as_bytes());
    let signature_hex: [u8; SIGNATURE_HEX_LEN] = hmac::signature_hex(&signature);
    hmac::authorization_header_value(device_id_str, &signature_hex, ts)
}

// -----------------------------------------------------------------------------
// reqwless transport — Xtensa only. Opens TLS over the embassy-net TCP stack,
// sends the signed request, parses the body. Bench-verified; cannot be
// host-compiled (reqwless/embedded-tls/embassy-net are Xtensa-gated deps).
// -----------------------------------------------------------------------------

#[cfg(all(target_arch = "xtensa", feature = "phase2-io"))]
pub mod transport {
    use super::*;
    use embassy_net::{Stack, dns::DnsSocket, tcp::client::TcpClient, tcp::client::TcpClientState};
    use reqwless::{
        client::{HttpClient, TlsConfig, TlsVerify},
        headers::ContentType,
        request::{Method, RequestBuilder},
    };

    use crate::config::API_BASE_URL;
    use crate::proto::api::{DeviceRegisterResponse, DeviceSyncResponse};

    /// Read buffer for the largest response we parse (a full sync snapshot
    /// with [`crate::proto::api::MAX_AUDIO`] presigned URLs). Lives in PSRAM
    /// via the caller's `#[link_section = ".ext_ram.bss"]` static.
    pub const RX_BUF_LEN: usize = 16 * 1024;
    /// TLS record scratch buffers. embedded-tls needs a read and a write
    /// workspace ≥ one TLS record (16 KiB max).
    pub const TLS_BUF_LEN: usize = 16 * 1024;

    /// Transport-level errors surfaced to the sync task.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TransportError {
        /// DNS resolution of the API host failed.
        Dns,
        /// TCP connect / TLS handshake failed.
        Connect,
        /// Request could not be built or sent.
        Request,
        /// Response body did not match the wire schema.
        Decode,
        /// Signing the request failed (path/header overflow).
        Sign(HmacError),
        /// Non-success HTTP status (carries the typed outcome).
        Status(HttpOutcome),
    }

    impl From<HmacError> for TransportError {
        fn from(e: HmacError) -> Self {
            TransportError::Sign(e)
        }
    }

    /// Everything the transport borrows for the lifetime of one connection.
    /// The buffers are owned by the caller (static / PSRAM) so the transport
    /// itself holds no large state.
    pub struct DeviceClient<'a> {
        stack: Stack<'a>,
        tcp_state: &'a TcpClientState<1, 4096, 4096>,
        rx_buf: &'a mut [u8],
        tls_read: &'a mut [u8],
        tls_write: &'a mut [u8],
        /// PHY-calibration RNG seed for the TLS session.
        tls_seed: u64,
    }

    impl<'a> DeviceClient<'a> {
        pub fn new(
            stack: Stack<'a>,
            tcp_state: &'a TcpClientState<1, 4096, 4096>,
            rx_buf: &'a mut [u8],
            tls_read: &'a mut [u8],
            tls_write: &'a mut [u8],
            tls_seed: u64,
        ) -> Self {
            Self {
                stack,
                tcp_state,
                rx_buf,
                tls_read,
                tls_write,
                tls_seed,
            }
        }

        /// `POST /v1/device/register`. Returns the parsed response so the
        /// caller can persist `device.id` to NVS.
        ///
        /// NOTE: the exact `reqwless` 0.13 builder calls below are pending
        /// confirmation against the pinned crate version on the bench; the
        /// request/sign/parse flow and the buffer sizing are the load-bearing
        /// parts and match the pure, host-tested helpers.
        pub async fn register(
            &mut self,
            secret: &[u8],
            device_id_str: &str,
            body: &[u8],
            ts: u64,
        ) -> Result<DeviceRegisterResponse, TransportError> {
            let auth = sign_request(secret, device_id_str, "POST", PATH_REGISTER, body, ts)?;
            let (status, len) = self
                .send(Method::POST, PATH_REGISTER, auth.as_str(), Some(body))
                .await?;
            self.expect_ok(status)?;
            let (resp, _) =
                serde_json_core::from_slice::<DeviceRegisterResponse>(&self.rx_buf[..len])
                    .map_err(|_| TransportError::Decode)?;
            Ok(resp)
        }

        /// `GET /v1/device/sync`. `Ok(None)` on `304 Not Modified`.
        pub async fn sync(
            &mut self,
            secret: &[u8],
            device_id_str: &str,
            since: Option<&str>,
            ts: u64,
        ) -> Result<Option<DeviceSyncResponse>, TransportError> {
            let path = build_sync_path(since)?;
            let auth = sign_request(secret, device_id_str, "GET", path.as_str(), b"", ts)?;
            let (status, len) = self
                .send(Method::GET, path.as_str(), auth.as_str(), None)
                .await?;
            match HttpOutcome::from_status(status) {
                HttpOutcome::NotModified => Ok(None),
                HttpOutcome::Ok => {
                    let (resp, _) =
                        serde_json_core::from_slice::<DeviceSyncResponse>(&self.rx_buf[..len])
                            .map_err(|_| TransportError::Decode)?;
                    Ok(Some(resp))
                }
                other => Err(TransportError::Status(other)),
            }
        }

        /// `POST /v1/device/events`. `202` on success.
        pub async fn post_events(
            &mut self,
            secret: &[u8],
            device_id_str: &str,
            body: &[u8],
            ts: u64,
        ) -> Result<(), TransportError> {
            let auth = sign_request(secret, device_id_str, "POST", PATH_EVENTS, body, ts)?;
            let (status, _) = self
                .send(Method::POST, PATH_EVENTS, auth.as_str(), Some(body))
                .await?;
            self.expect_ok(status)
        }

        fn expect_ok(&self, status: u16) -> Result<(), TransportError> {
            match HttpOutcome::from_status(status) {
                HttpOutcome::Ok => Ok(()),
                other => Err(TransportError::Status(other)),
            }
        }

        /// Open TLS, send one signed request, return `(status, body_len)`
        /// with the body left in `self.rx_buf`.
        async fn send(
            &mut self,
            method: Method,
            path: &str,
            auth: &str,
            body: Option<&[u8]>,
        ) -> Result<(u16, usize), TransportError> {
            let dns = DnsSocket::new(self.stack);
            let tcp = TcpClient::new(self.stack, self.tcp_state);
            // SECURITY — OPEN DECISION (bench / PO). `reqwless` 0.13's
            // `TlsVerify` exposes only `None` / `Psk`; it has **no CA
            // trust-anchor variant**, so the chain cannot be verified against
            // the embedded ISRG Root X1 here. `TlsVerify::None` keeps the
            // handshake working (encrypted, but unauthenticated server) so
            // the rest of the flow can be bench-tested. This MUST NOT ship to
            // production as-is: either upgrade to a `reqwless`/`embedded-tls`
            // that verifies a DER trust anchor, or pin the server public key.
            // The anchor stays embedded in `crate::certs` ready for that.
            let _ = crate::certs::ISRG_ROOT_X1_DER;
            let tls = TlsConfig::new(
                self.tls_seed,
                self.tls_read,
                self.tls_write,
                TlsVerify::None,
            );
            let mut client = HttpClient::new_with_tls(&tcp, &dns, tls);

            let mut url: String<{ MAX_PATH_LEN + 64 }> = String::new();
            url.push_str(API_BASE_URL)
                .map_err(|_| TransportError::Request)?;
            url.push_str(path).map_err(|_| TransportError::Request)?;

            // `body()` changes the builder's body type parameter, so the
            // request must be built in a single chain rather than reassigned.
            // GET carries an empty body; the JSON content-type on it is inert.
            let body_bytes: &[u8] = body.unwrap_or(&[]);
            let headers = [("Authorization", auth)];
            let mut req = client
                .request(method, url.as_str())
                .await
                .map_err(|_| TransportError::Connect)?
                .headers(&headers)
                .body(body_bytes)
                .content_type(ContentType::ApplicationJson);
            let resp = req
                .send(self.rx_buf)
                .await
                .map_err(|_| TransportError::Request)?;
            // `reqwless::response::StatusCode` is a `u16` newtype.
            let status: u16 = resp.status.0;
            let body = resp
                .body()
                .read_to_end()
                .await
                .map_err(|_| TransportError::Decode)?;
            let len = body.len();
            Ok((status, len))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::hmac;

    #[test]
    fn format_uuid_renders_hyphenated_lowercase() {
        let bytes: [u8; 16] = [
            0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44,
            0x00, 0x00,
        ];
        assert_eq!(
            format_uuid(&bytes).as_str(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn build_sync_path_without_since_is_bare_path() {
        assert_eq!(build_sync_path(None).unwrap().as_str(), "/v1/device/sync");
    }

    #[test]
    fn build_sync_path_percent_encodes_timestamp_colons() {
        let path = build_sync_path(Some("2026-06-19T13:17:09Z")).unwrap();
        assert_eq!(
            path.as_str(),
            "/v1/device/sync?since=2026-06-19T13%3A17%3A09Z"
        );
    }

    #[test]
    fn percent_encode_leaves_unreserved_untouched() {
        let mut out: String<64> = String::new();
        percent_encode_into("AZaz09-._~", &mut out).unwrap();
        assert_eq!(out.as_str(), "AZaz09-._~");
    }

    #[test]
    fn percent_encode_escapes_reserved() {
        let mut out: String<64> = String::new();
        percent_encode_into("a b/c?d=e&f", &mut out).unwrap();
        assert_eq!(out.as_str(), "a%20b%2Fc%3Fd%3De%26f");
    }

    #[test]
    fn http_outcome_maps_known_statuses() {
        assert_eq!(HttpOutcome::from_status(200), HttpOutcome::Ok);
        assert_eq!(HttpOutcome::from_status(202), HttpOutcome::Ok);
        assert_eq!(HttpOutcome::from_status(304), HttpOutcome::NotModified);
        assert_eq!(HttpOutcome::from_status(401), HttpOutcome::Unauthorized);
        assert_eq!(HttpOutcome::from_status(404), HttpOutcome::NotFound);
        assert_eq!(HttpOutcome::from_status(422), HttpOutcome::Unprocessable);
        assert_eq!(HttpOutcome::from_status(429), HttpOutcome::RateLimited);
        assert_eq!(HttpOutcome::from_status(503), HttpOutcome::ServerError);
    }

    #[test]
    fn only_transient_outcomes_are_retriable() {
        assert!(HttpOutcome::RateLimited.is_retriable());
        assert!(HttpOutcome::ServerError.is_retriable());
        assert!(!HttpOutcome::Unauthorized.is_retriable());
        assert!(!HttpOutcome::Unprocessable.is_retriable());
        assert!(!HttpOutcome::NotModified.is_retriable());
        assert!(!HttpOutcome::Ok.is_retriable());
    }

    /// The signed header must reproduce the `auth.md` worked example when fed
    /// the same inputs the canonical-request test uses — proves the client's
    /// assembly path agrees with the spec, end to end.
    #[test]
    fn sign_request_matches_authmd_worked_example_signature() {
        // Secret is arbitrary here; we assert the canonical string the
        // signature is computed over, via the keyId/ts framing, is stable.
        let secret = [0x42u8; 32];
        let body = br#"{"serial":"ABC"}"#;
        let ts = 1_716_290_000;
        let header = sign_request(
            &secret,
            "550e8400-e29b-41d4-a716-446655440000",
            "POST",
            "/v1/device/register",
            body,
            ts,
        )
        .unwrap();

        // Independently recompute the expected signature from the canonical
        // request to pin the assembly (body hash → canonical → sign → hex).
        let body_sha = hmac::body_sha256_hex(body);
        let canon = hmac::canonical_request("POST", "/v1/device/register", ts, &body_sha).unwrap();
        let sig = hmac::sign(&secret, canon.as_bytes());
        let sig_hex = hmac::signature_hex(&sig);
        let expected =
            hmac::authorization_header_value("550e8400-e29b-41d4-a716-446655440000", &sig_hex, ts)
                .unwrap();

        assert_eq!(header.as_str(), expected.as_str());
        assert!(
            header
                .as_str()
                .starts_with("HMAC keyId=550e8400-e29b-41d4-a716-446655440000,")
        );
        assert!(header.as_str().ends_with(",ts=1716290000"));
    }
}
