//! HMAC-SHA256 signing for device requests.
//!
//! Canonical request format and clock skew tolerance are defined in
//! `hush-protocol/docs/auth.md`. The exact canonicalization must match
//! the backend verifier byte-for-byte; covered by host tests with the
//! `mock-hardware` feature.
//!
//! TODO(phase-2): `pub fn sign(secret, method, path, body, ts) -> [u8; 32]`
//! and a header builder that returns the full
//! `Authorization: HMAC keyId=...,signature=...,ts=...` string in a
//! heapless buffer.
