//! microSD bring-up over the shared SPI bus using `embedded-sdmmc`.
//!
//! Phase-1 goal: prove the SPI link to the card works, log the card
//! size, and successfully mount the first FAT32 volume from the MBR.
//! The cache task (phase 3) will own the driver from then on; this
//! module keeps the [`SdCardDriver`] alive in a `'static` so phase 3
//! can pick it up without re-claiming the SPI bus from the consumed
//! `Peripherals` struct.
//!
//! ## Wiring
//!
//! - Shared SPI bus: SCK [`crate::hw::pins::SPI_SCK`], MOSI
//!   [`crate::hw::pins::SPI_MOSI`], MISO [`crate::hw::pins::SPI_MISO`] — the
//!   same bus the MFRC522 hangs off (XIAO has no room for two SPI buses).
//! - Chip select [`crate::hw::pins::SD_CS`] (active-low).
//! - Card-detect: **no pin** — no free GPIO in the XIAO's 11-pad budget.
//!   Card presence is inferred at mount (a failed init == "no card") rather
//!   than from a dedicated CD line.
//!
//! TODO(hw-reconcile): the CS wrapper below still uses `ExclusiveDevice`,
//! which owns the whole bus. Once RFID + microSD truly share one bus this
//! must become a shared-bus `SpiDevice`. Tracked in `PLAN.md`.
//!
//! ## Clocking
//!
//! 400 kHz for the first-bring-up. SD spec requires the init handshake
//! to happen between 100 kHz and 400 kHz; once the card is in
//! data-transfer mode it tolerates 12-25 MHz, but bumping the clock
//! requires the `SdCard::spi(|spi| ...)` re-clock closure and a
//! sizing-up bench check that we defer to phase 3 (audio cache, where
//! throughput actually matters).
//!
//! ## TimeSource
//!
//! [`StubTimeSource`] hard-codes Jan 1 2000. FAT32 directory entries
//! get this timestamp until phase 4 wires a real RTC; for the cache
//! use case (whose entries are short-lived and identified by SHA-256,
//! not mtime) this is harmless.

use embedded_hal_bus::spi::CriticalSectionDevice;
use embedded_sdmmc::{
    Error as SdMmcError, SdCard, SdCardError, TimeSource, Timestamp, VolumeIdx, VolumeManager,
};
use esp_hal::{Blocking, delay::Delay, gpio::Output, spi::master::Spi};

/// SPI clock for first bring-up. SD spec mandates the init handshake
/// at ≤ 400 kHz; we sit at the upper end so card detection still
/// completes in well under a second.
pub const SD_INIT_SPI_HZ: u32 = 400_000;

/// Concrete SPI device type: a per-device handle onto the **shared** SPI bus
/// (RFID + microSD share one bus on the XIAO), combining the `'static`
/// critical-section-guarded bus with the microSD's dedicated CS pin.
pub type SdSpiDevice =
    CriticalSectionDevice<'static, Spi<'static, Blocking>, Output<'static>, Delay>;

/// Concrete `SdCard` handle the rest of the firmware can borrow from
/// the `'static` storage in `main`.
pub type SdCardHandle = SdCard<SdSpiDevice, Delay>;

/// Concrete `VolumeManager` parameterised over the default 4 / 4 / 1
/// limits embedded-sdmmc ships with. Bumped only when the cache task
/// shows up and needs more concurrent dirs / files.
pub type SdVolumeManager = VolumeManager<SdCardHandle, StubTimeSource>;

/// TimeSource stub. Always reports 2000-01-01 00:00:00, which is the
/// FAT32 epoch's lower bound expressible without `.unwrap()` on
/// `Timestamp::from_calendar`. We avoid the fallible constructor by
/// building the struct directly with field literals.
#[derive(Default, Clone, Copy)]
pub struct StubTimeSource;

impl TimeSource for StubTimeSource {
    fn get_timestamp(&self) -> Timestamp {
        // 2000-01-01 00:00:00. `year_since_1970 = 30` is the year
        // offset embedded-sdmmc expects.
        Timestamp {
            year_since_1970: 30,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

/// Errors surfaced by [`SdCardDriver::new`] and the bring-up calls in
/// `main`. Configuration faults and runtime SD errors are split so the
/// caller can decide whether "no card present" should pulse the LED
/// amber (operator should insert a card) or red (driver itself is
/// broken).
#[derive(Debug)]
pub enum SdError {
    /// `ExclusiveDevice::new_no_delay` failed because the CS pin
    /// refused its initial high write — indicates a GPIO config bug,
    /// not a card fault.
    SpiConfig,
    /// `SdCard::num_bytes` or another driver call returned an
    /// `SdCardError`. Stringified so the bus-error generic doesn't
    /// bleed into every call site.
    Card(&'static str),
    /// `VolumeManager::open_volume(VolumeIdx(0))` failed — typically
    /// "no MBR signature" (raw flash card) or "Bad partition table"
    /// (exotic format). Stringified for the same reason as `Card`.
    Mount(&'static str),
}

impl SdError {
    fn from_card(err: SdCardError) -> Self {
        Self::Card(card_error_tag(err))
    }
}

const fn card_error_tag(err: SdCardError) -> &'static str {
    use SdCardError::*;
    match err {
        Transport => "transport",
        CantEnableCRC => "cant_enable_crc",
        TimeoutReadBuffer => "timeout_read_buffer",
        TimeoutWaitNotBusy => "timeout_wait_not_busy",
        TimeoutCommand(_) => "timeout_command",
        TimeoutACommand(_) => "timeout_a_command",
        Cmd58Error => "cmd58_error",
        RegisterReadError => "register_read_error",
        CrcError(_, _) => "crc_error",
        ReadError => "read_error",
        WriteError => "write_error",
        BadState => "bad_state",
        CardNotFound => "card_not_found",
        GpioError => "gpio_error",
    }
}

/// Concrete SD driver: owns the SPI device wrapper, the `SdCard`
/// handle, and the `VolumeManager` so a phase-3 cache task can borrow
/// against them without restructuring.
pub struct SdCardDriver {
    pub volume_mgr: SdVolumeManager,
    /// Card size cached from the init handshake. `embedded-sdmmc 0.9`
    /// does not let us re-read it through `VolumeManager` without
    /// hitting an upstream API bug (the `device` closure helper has
    /// a stray `T` return type tied to the time-source generic), so
    /// we capture it once at construction.
    card_size_bytes: u64,
}

impl SdCardDriver {
    /// Build the driver. Eagerly drives the SD init handshake so a
    /// missing or unresponsive card surfaces as an `SdError::Card`
    /// here rather than as a confusing later mount failure.
    pub fn new(device: SdSpiDevice) -> Result<Self, SdError> {
        let sdcard: SdCardHandle = SdCard::new(device, Delay::new());
        let card_size_bytes = sdcard.num_bytes().map_err(SdError::from_card)?;
        let volume_mgr = VolumeManager::new(sdcard, StubTimeSource);
        Ok(Self {
            volume_mgr,
            card_size_bytes,
        })
    }

    /// Whether a usable card was found. There is no card-detect pin on the
    /// XIAO, so presence is inferred from the init handshake: reaching a
    /// constructed driver means `num_bytes` answered, so a non-zero size is
    /// a present, readable card.
    pub fn card_present(&self) -> bool {
        self.card_size_bytes > 0
    }

    /// Usable card size in bytes, cached from the init handshake.
    pub fn card_size_bytes(&self) -> u64 {
        self.card_size_bytes
    }

    /// Attempt to open the first MBR partition as a FAT32 volume.
    /// Drops the resulting `Volume` immediately — the goal of phase 1
    /// is to prove the mount succeeds, not to hold files open. The
    /// `VolumeManager` retains the partition table cache for cheap
    /// re-opens by the phase-3 cache task.
    pub fn probe_first_volume(&self) -> Result<(), SdError> {
        let _ = self
            .volume_mgr
            .open_volume(VolumeIdx(0))
            .map_err(|err| SdError::Mount(volume_error_tag(&err)))?;
        Ok(())
    }
}

/// Hard-coded to `SdCardError` because that is the only `D::Error` we
/// ever instantiate. The generic helper would require an
/// `E: core::fmt::Debug` bound to reach the variants whose payloads
/// embedded-sdmmc declares as `Debug`-required, and propagating that
/// bound through every call site adds noise for no win.
fn volume_error_tag(err: &SdMmcError<SdCardError>) -> &'static str {
    match err {
        SdMmcError::DeviceError(_) => "device_error",
        SdMmcError::FormatError(_) => "format_error",
        SdMmcError::NoSuchVolume => "no_such_volume",
        SdMmcError::FilenameError(_) => "filename_error",
        SdMmcError::TooManyOpenVolumes => "too_many_open_volumes",
        SdMmcError::TooManyOpenDirs => "too_many_open_dirs",
        SdMmcError::TooManyOpenFiles => "too_many_open_files",
        SdMmcError::BadHandle => "bad_handle",
        SdMmcError::NotFound => "not_found",
        SdMmcError::FileAlreadyOpen => "file_already_open",
        SdMmcError::DirAlreadyOpen => "dir_already_open",
        SdMmcError::OpenedDirAsFile => "opened_dir_as_file",
        SdMmcError::OpenedFileAsDir => "opened_file_as_dir",
        SdMmcError::DeleteDirAsFile => "delete_dir_as_file",
        SdMmcError::VolumeStillInUse => "volume_still_in_use",
        SdMmcError::VolumeAlreadyOpen => "volume_already_open",
        SdMmcError::Unsupported => "unsupported",
        SdMmcError::EndOfFile => "end_of_file",
        SdMmcError::BadCluster => "bad_cluster",
        SdMmcError::ConversionError => "conversion_error",
        SdMmcError::NotEnoughSpace => "not_enough_space",
        SdMmcError::AllocationError => "allocation_error",
        SdMmcError::UnterminatedFatChain => "unterminated_fat_chain",
        SdMmcError::ReadOnly => "read_only",
        SdMmcError::FileAlreadyExists => "file_already_exists",
        SdMmcError::BadBlockSize(_) => "bad_block_size",
        SdMmcError::InvalidOffset => "invalid_offset",
        SdMmcError::DiskFull => "disk_full",
        SdMmcError::DirAlreadyExists => "dir_already_exists",
        SdMmcError::LockError => "lock_error",
    }
}
