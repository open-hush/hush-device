//! Embassy tasks — one per concern.
//!
//! Tasks are spawned from `main.rs`. They never share mutable state via
//! globals; dependencies are passed in as arguments and inter-task
//! communication uses channels declared in [`crate::proto::events`].

pub mod rfid;
pub mod audio;
pub mod cache;
pub mod sync;
pub mod input;
pub mod power;
pub mod led;
