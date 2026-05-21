//! Index of cached audio files on the microSD.
//!
//! Stored in NVS (small, ~64 entries) so it can be consulted without
//! mounting the SD card — useful when the card is missing or corrupted.
//! The on-SD `/cache/index.bin` is the authoritative copy; the NVS copy
//! is a hot cache rebuilt on boot.
//!
//! TODO(phase-3): entry shape `{ audio_id: Uuid, sha256: [u8; 32], size:
//! u32, last_played: u32 }`. LRU eviction handled by the `cache` task.
