//! Embedded TLS trust anchors.
//!
//! Phase 2 talks to `https://api.open-hush.com`, served behind a
//! Let's Encrypt certificate. The chain roots in **ISRG Root X1**, so that
//! single self-signed root is all the device needs to validate the server.
//!
//! Decision (`PLAN.md` → "TLS root certs"): bundle **ISRG Root X1 only**.
//! Smaller trust store = faster handshake and less flash. If Let's Encrypt
//! ever cross-signs through a different root, or we move the API behind a CA
//! that does not chain to ISRG Root X1, add that root here and document why.
//!
//! The certificate is embedded in **DER** form (`certs/isrg_root_x1.der`),
//! the shape `embedded-tls` consumes directly — no runtime PEM/base64
//! decode, no allocator. Provenance: fetched from
//! <https://letsencrypt.org/certs/isrgrootx1.pem> and converted with
//! `openssl x509 -outform DER`. SHA-256 of the DER:
//! `96bcec06264976f37460779acf28c5a7cfe8a3c0aae11a8ffcee05c0bddf08c6`
//! (matches the published ISRG Root X1 fingerprint).

/// ISRG Root X1, DER-encoded. 1391 bytes.
pub static ISRG_ROOT_X1_DER: &[u8] = include_bytes!("../certs/isrg_root_x1.der");

/// All trust anchors the firmware ships with, in DER form. The TLS client
/// (Xtensa) feeds these to `embedded-tls`; host tests assert their shape.
pub static TRUST_ANCHORS_DER: &[&[u8]] = &[ISRG_ROOT_X1_DER];

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded blob must be present and the right size; an empty or
    /// truncated `include_bytes!` would silently disable verification.
    #[test]
    fn isrg_root_x1_der_is_embedded_and_sized() {
        assert_eq!(ISRG_ROOT_X1_DER.len(), 1391);
    }

    /// DER certificates are a SEQUENCE: first byte is the tag `0x30`. This
    /// is a cheap guard against accidentally embedding the PEM text (which
    /// would start with `0x2d` = '-') instead of the DER bytes.
    #[test]
    fn isrg_root_x1_is_der_not_pem() {
        assert_eq!(ISRG_ROOT_X1_DER[0], 0x30, "DER SEQUENCE tag expected");
    }

    #[test]
    fn trust_anchors_contains_isrg_root_x1() {
        assert_eq!(TRUST_ANCHORS_DER.len(), 1);
        assert_eq!(TRUST_ANCHORS_DER[0].len(), 1391);
    }
}
