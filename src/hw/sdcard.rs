//! microSD bring-up over SPI3 using `embedded-sdmmc`.
//!
//! Filesystem is FAT32 (compatibility with off-the-shelf cards). On a
//! card-detect transition we close the volume and reopen it on
//! reinsertion.
//!
//! TODO(phase-1): wire SPI3 with the pins from [`crate::hw::pins`] and
//! expose a `Volume<...>` the `cache` task can use.
