//! Cache task — resolves `audioId → file path on SD`, downloading from the
//! backend on cache miss.
//!
//! Stack: ~6 KB target (network buffers live in PSRAM).
//!
//! TODO(phase-3): implement the cache lookup + LRU eviction + streaming
//! download to SD with SHA-256 verification on every read.
