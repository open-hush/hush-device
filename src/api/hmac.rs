//! HMAC-SHA256 signing for device requests.
//!
//! The canonical request format, header layout and clock-skew window
//! are defined in `hush-protocol/docs/auth.md` and are the source of
//! truth. Any drift between this module and the backend verifier means
//! requests fail at 401 `invalid_signature` — the test below pins us to
//! the worked example in the spec so regressions surface here, not in
//! production.
//!
//! Canonical request (single LF separators, no trailing newline):
//!
//! ```text
//! <METHOD>\n<PATH-WITH-QUERY>\n<TS>\n<BODY-SHA256-HEX>
//! ```
//!
//! - `METHOD` — uppercased HTTP method.
//! - `PATH-WITH-QUERY` — path with leading `/`, plus the query string
//!   if any. **Caller responsibility**: sort query keys lexicographically
//!   and URL-encode per RFC 3986. The device endpoints in Phase 2
//!   (`/v1/device/register`, `/v1/device/sync`, `/v1/device/events`)
//!   carry no query parameters today; revisit this helper when one
//!   does.
//! - `TS` — `u64` unix-seconds, decimal, same value as the `ts=` field
//!   in the `Authorization` header.
//! - `BODY-SHA256-HEX` — lowercase hex SHA-256 of the raw body bytes;
//!   empty body uses the SHA-256 of the empty string.

#![allow(dead_code)] // wired into the client in a follow-up Phase 2 commit.

use core::fmt::Write as _;

use heapless::String;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// Raw HMAC-SHA256 output length in bytes.
pub const SIGNATURE_BYTES: usize = 32;
/// Hex-encoded HMAC signature length.
pub const SIGNATURE_HEX_LEN: usize = SIGNATURE_BYTES * 2;
/// Hex-encoded SHA-256 digest length.
pub const SHA256_HEX_LEN: usize = 64;

/// Upper bound for `path-with-query`. Device endpoints today are well
/// under this (the longest is `/v1/device/register` at 19 bytes); the
/// margin covers a future cursor/since query string before a v2 bump.
pub const MAX_PATH_LEN: usize = 192;

/// Canonical-request capacity: method (≤6) + LF + path (≤192) + LF +
/// ts (≤20) + LF + sha256 hex (64) = 285. Round up to 320 for slack
/// against a longer method-name or alloc-free `write!` overshoot.
pub const MAX_CANONICAL_LEN: usize = 320;

/// `HMAC keyId=<uuid-36>,signature=<hex-64>,ts=<u64-decimal>` =
/// 11 + 36 + 11 + 64 + 4 + 20 = 146 bytes worst-case. 160 covers it.
pub const MAX_AUTH_HEADER_LEN: usize = 160;

/// Errors that can only happen because a caller-supplied input or a
/// fixed-capacity buffer wasn't large enough. Crypto operations
/// themselves cannot fail for the fixed shapes used here.
///
/// The shared `TooLong` suffix is intentional (clippy's
/// `enum_variant_names` lint is fine here): every variant *is* a
/// length overflow, and naming them after the buffer they bound
/// keeps call-site error handling readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum HmacError {
    /// `path_with_query` exceeded [`MAX_PATH_LEN`].
    PathTooLong,
    /// Canonical-request buffer overflowed [`MAX_CANONICAL_LEN`].
    CanonicalTooLong,
    /// Authorization-header buffer overflowed [`MAX_AUTH_HEADER_LEN`].
    HeaderTooLong,
}

type HmacSha256 = Hmac<Sha256>;

/// Lowercase hex SHA-256 of `body`. For the empty body this is the
/// well-known constant `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`.
pub fn body_sha256_hex(body: &[u8]) -> [u8; SHA256_HEX_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(body);
    let digest = hasher.finalize();
    let mut out = [0u8; SHA256_HEX_LEN];
    write_hex(&digest, &mut out);
    out
}

/// Build the canonical request string per `docs/auth.md`. The output
/// is a [`heapless::String`] so callers can hand the byte slice to
/// [`sign`] without copying.
///
/// - `method` is uppercased into the output (HTTP methods are ASCII).
/// - `path_with_query` is written verbatim — query sorting / RFC 3986
///   encoding is the caller's job.
/// - `ts` is rendered as a decimal `u64`.
/// - `body_sha_hex` is the precomputed lowercase hex digest of the
///   request body (see [`body_sha256_hex`]).
pub fn canonical_request(
    method: &str,
    path_with_query: &str,
    ts: u64,
    body_sha_hex: &[u8; SHA256_HEX_LEN],
) -> Result<String<MAX_CANONICAL_LEN>, HmacError> {
    if path_with_query.len() > MAX_PATH_LEN {
        return Err(HmacError::PathTooLong);
    }

    let mut out: String<MAX_CANONICAL_LEN> = String::new();
    for b in method.bytes() {
        out.push(b.to_ascii_uppercase() as char)
            .map_err(|_| HmacError::CanonicalTooLong)?;
    }
    out.push('\n').map_err(|_| HmacError::CanonicalTooLong)?;
    out.push_str(path_with_query)
        .map_err(|_| HmacError::CanonicalTooLong)?;
    out.push('\n').map_err(|_| HmacError::CanonicalTooLong)?;
    write!(&mut out, "{ts}").map_err(|_| HmacError::CanonicalTooLong)?;
    out.push('\n').map_err(|_| HmacError::CanonicalTooLong)?;
    // SAFETY: `body_sha_hex` is always lowercase hex (produced by
    // `write_hex` above), so it is valid UTF-8 by construction.
    let body_sha_str = core::str::from_utf8(body_sha_hex)
        .expect("body_sha_hex is ASCII lowercase hex by construction");
    out.push_str(body_sha_str)
        .map_err(|_| HmacError::CanonicalTooLong)?;
    Ok(out)
}

/// HMAC-SHA256 over `canonical` keyed by `secret`. RFC 2104 accepts any
/// key length; production secrets are exactly 32 bytes per
/// `docs/auth.md`, but the slice signature keeps host tests free to
/// reuse RFC 4231 vectors with shorter keys.
pub fn sign(secret: &[u8], canonical: &[u8]) -> [u8; SIGNATURE_BYTES] {
    let mut mac = HmacSha256::new_from_slice(secret)
        .expect("Hmac<Sha256>::new_from_slice accepts any key length per RFC 2104");
    mac.update(canonical);
    let result = mac.finalize().into_bytes();
    let mut out = [0u8; SIGNATURE_BYTES];
    out.copy_from_slice(&result);
    out
}

/// Lowercase hex of an HMAC-SHA256 output. Pair with [`sign`] to get
/// the `signature=` value for the `Authorization` header.
pub fn signature_hex(signature: &[u8; SIGNATURE_BYTES]) -> [u8; SIGNATURE_HEX_LEN] {
    let mut out = [0u8; SIGNATURE_HEX_LEN];
    write_hex(signature, &mut out);
    out
}

/// Render the `Authorization` **header value** (the byte string that
/// follows `Authorization:` on the wire — the HTTP client adds the
/// `Authorization:` name itself).
///
/// `device_id_str` is taken verbatim: pass the canonical lowercase
/// 36-char UUID rendering, no surrounding whitespace, no quotes.
pub fn authorization_header_value(
    device_id_str: &str,
    signature_hex: &[u8; SIGNATURE_HEX_LEN],
    ts: u64,
) -> Result<String<MAX_AUTH_HEADER_LEN>, HmacError> {
    let mut out: String<MAX_AUTH_HEADER_LEN> = String::new();
    out.push_str("HMAC keyId=")
        .map_err(|_| HmacError::HeaderTooLong)?;
    out.push_str(device_id_str)
        .map_err(|_| HmacError::HeaderTooLong)?;
    out.push_str(",signature=")
        .map_err(|_| HmacError::HeaderTooLong)?;
    let sig_str = core::str::from_utf8(signature_hex)
        .expect("signature_hex is ASCII lowercase hex by construction");
    out.push_str(sig_str)
        .map_err(|_| HmacError::HeaderTooLong)?;
    out.push_str(",ts=").map_err(|_| HmacError::HeaderTooLong)?;
    write!(&mut out, "{ts}").map_err(|_| HmacError::HeaderTooLong)?;
    Ok(out)
}

/// Lowercase hex encode of `bytes` into the front of `out`. `out` must
/// be at least `2 * bytes.len()` long; this is enforced by every public
/// caller using fixed-size arrays.
fn write_hex(bytes: &[u8], out: &mut [u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, b) in bytes.iter().enumerate() {
        out[2 * i] = HEX[(*b >> 4) as usize];
        out[2 * i + 1] = HEX[(*b & 0x0f) as usize];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIPS 180-4 §B.1 / RFC 6234 §A — SHA-256("") is universal.
    #[test]
    fn body_sha256_hex_empty_body_matches_known_constant() {
        let expected = b"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(&body_sha256_hex(b""), expected);
    }

    /// `docs/auth.md` claims SHA-256 of `{"serial":"ABC"}` equals
    /// `4ab5335f…83e8`. If this test fails the spec's worked example
    /// is wrong **or** we are; either way: stop and reconcile.
    #[test]
    fn body_sha256_hex_matches_authmd_worked_example() {
        let expected = b"4ab5335fc428dd5acb18e99a9c531408017a472feb6fc5ce1e382723678083e8";
        assert_eq!(&body_sha256_hex(br#"{"serial":"ABC"}"#), expected);
    }

    /// The worked example in `docs/auth.md` — entire canonical string
    /// reproduced byte-for-byte. This is the single most important
    /// assertion in the firmware's auth surface.
    #[test]
    fn canonical_request_matches_authmd_worked_example() {
        let body_sha = body_sha256_hex(br#"{"serial":"ABC"}"#);
        let canon =
            canonical_request("POST", "/v1/device/register", 1_716_290_000, &body_sha).unwrap();
        let expected = "POST\n/v1/device/register\n1716290000\n\
                        4ab5335fc428dd5acb18e99a9c531408017a472feb6fc5ce1e382723678083e8";
        assert_eq!(canon.as_str(), expected);
    }

    #[test]
    fn canonical_request_uppercases_method() {
        let body_sha = body_sha256_hex(b"");
        let canon = canonical_request("get", "/v1/health", 42, &body_sha).unwrap();
        assert!(
            canon.as_str().starts_with("GET\n"),
            "method must be uppercased, got: {:?}",
            canon.as_str()
        );
    }

    #[test]
    fn canonical_request_no_trailing_newline_and_lf_only() {
        let body_sha = body_sha256_hex(b"");
        let canon = canonical_request("GET", "/v1/health", 1, &body_sha).unwrap();
        let bytes = canon.as_bytes();
        assert!(
            !bytes.ends_with(b"\n"),
            "canonical request must not end with LF"
        );
        assert!(
            !bytes.contains(&b'\r'),
            "canonical request must use bare LF, not CRLF"
        );
        assert_eq!(
            bytes.iter().filter(|b| **b == b'\n').count(),
            3,
            "exactly 3 LF separators between the 4 fields"
        );
    }

    #[test]
    fn canonical_request_rejects_over_long_path() {
        let body_sha = body_sha256_hex(b"");
        let mut path: String<512> = String::new();
        path.push('/').unwrap();
        for _ in 0..MAX_PATH_LEN {
            path.push('a').unwrap();
        }
        assert_eq!(path.len(), MAX_PATH_LEN + 1);
        assert_eq!(
            canonical_request("GET", path.as_str(), 1, &body_sha).unwrap_err(),
            HmacError::PathTooLong
        );
    }

    /// RFC 4231 §4.2 — HMAC-SHA256 test vector 1. Verifies the
    /// underlying primitive integrates with our `sign(...)` shape.
    #[test]
    fn sign_matches_rfc4231_vector_1() {
        let key = [0x0bu8; 20];
        let data = b"Hi There";
        let sig = sign(&key, data);
        let expected: [u8; 32] = [
            0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53, 0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b,
            0xf1, 0x2b, 0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7, 0x26, 0xe9, 0x37, 0x6c,
            0x2e, 0x32, 0xcf, 0xf7,
        ];
        assert_eq!(sig, expected);
    }

    #[test]
    fn sign_is_deterministic_and_input_sensitive() {
        let secret = [0x42u8; 32];
        let s1 = sign(&secret, b"abc");
        let s2 = sign(&secret, b"abc");
        let s3 = sign(&secret, b"abd");
        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
    }

    #[test]
    fn signature_hex_renders_lowercase() {
        let sig: [u8; SIGNATURE_BYTES] = [
            0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53, 0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b,
            0xf1, 0x2b, 0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7, 0x26, 0xe9, 0x37, 0x6c,
            0x2e, 0x32, 0xcf, 0xf7,
        ];
        let hex = signature_hex(&sig);
        assert_eq!(
            &hex[..],
            b"b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn authorization_header_value_well_formed() {
        let sig = [0u8; SIGNATURE_BYTES];
        let sig_hex = signature_hex(&sig);
        let hdr = authorization_header_value(
            "550e8400-e29b-41d4-a716-446655440000",
            &sig_hex,
            1_716_290_000,
        )
        .unwrap();
        let expected = "HMAC keyId=550e8400-e29b-41d4-a716-446655440000,\
                        signature=0000000000000000000000000000000000000000000000000000000000000000,\
                        ts=1716290000";
        assert_eq!(hdr.as_str(), expected);
    }
}
