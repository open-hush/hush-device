//! Crate-wide error type.
//!
//! No `unwrap` in hot paths; everything that can fail returns
//! [`Result<T>`]. Errors are logged at the boundary task and either degrade
//! gracefully (LED red blink) or trigger a controlled reboot.
//!
//! TODO(phase-2): flesh out variants as real failures appear during bring-up.

#[derive(Debug)]
pub enum Error {
    /// Hardware peripheral refused to initialise.
    Hardware(&'static str),
    /// I/O on SD card or NVS.
    Storage(&'static str),
    /// Network I/O (WiFi, TLS, HTTP).
    Network(&'static str),
    /// Wire format does not match `hush-protocol`.
    Protocol(&'static str),
    /// HMAC signing or verification failed.
    Auth(&'static str),
    /// Audio decoder error.
    Audio(&'static str),
}

pub type Result<T> = core::result::Result<T, Error>;
