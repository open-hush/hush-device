//! MFRC522 (RFID reader) bring-up over SPI2.
//!
//! ## Wiring
//!
//! - SPI2 bus: SCK [`crate::hw::pins::RFID_SCK`], MOSI
//!   [`crate::hw::pins::RFID_MOSI`], MISO [`crate::hw::pins::RFID_MISO`].
//! - Chip select: [`crate::hw::pins::RFID_CS`] (active-low, driven by
//!   [`embedded_hal_bus::spi::ExclusiveDevice`]).
//! - Hardware reset: [`crate::hw::pins::RFID_RST`] (active-low). Held
//!   high during normal operation by retaining the [`esp_hal::gpio::Output`]
//!   handle inside [`RfidDriver`]; dropping the driver would let the pin
//!   revert and soft-reset the chip.
//! - Interrupt: [`crate::hw::pins::RFID_IRQ`] — wired but **not currently
//!   used**. See the "IRQ deferral" note in `PLAN.md`; the `mfrc522 = 0.7`
//!   crate does not expose IRQ-register access, so this phase polls.
//!
//! ## Polling instead of IRQ
//!
//! The phase-1 acceptance criterion calls for IRQ-driven UID reads. The
//! upstream `mfrc522` 0.7 crate keeps its `read` / `write` register
//! methods private and offers no `enable_irq` / `clear_irq` surface, so
//! true IRQ-driven operation would require either a fork or raw-SPI
//! register pokes alongside the high-level driver. For first bring-up we
//! poll at [`crate::tasks::rfid::POLL_INTERVAL_MS`] ms, which gives
//! sub-100 ms scan latency — well inside the UX target of "tap and it
//! plays". Switching to IRQ is tracked as a follow-up in `PLAN.md`.

use embedded_hal_bus::spi::{ExclusiveDevice, NoDelay};
use esp_hal::{
    Blocking,
    gpio::Output,
    spi::master::Spi,
};
use mfrc522::{
    Initialized, Mfrc522, Uid,
    comm::blocking::spi::{DummyDelay, SpiInterface},
    error::Error as MfrcLibError,
};

/// Concrete SPI device type: the SPI2 bus plus the dedicated CS pin,
/// wrapped through `embedded-hal-bus`'s `ExclusiveDevice` (no inter-byte
/// delay — MFRC522 does not require one) so the `mfrc522` driver can
/// consume it through the embedded-hal 1.0 `SpiDevice` trait.
pub type RfidSpiDevice = ExclusiveDevice<Spi<'static, Blocking>, Output<'static>, NoDelay>;

/// Concrete `mfrc522::Interface` implementation.
pub type RfidComm = SpiInterface<RfidSpiDevice, DummyDelay>;

/// Concrete initialised driver handle.
pub type RfidChip = Mfrc522<RfidComm, Initialized>;

/// Errors surfaced by [`RfidDriver::new`] and the [`RfidReader`]
/// implementation. Discriminates between "bus setup failed" and
/// "MFRC522 talked back wrong" so the caller can decide whether to
/// retry, soft-reset, or red-blink the LED.
#[derive(Debug)]
pub enum RfidError {
    /// The CS pin refused its initial high write — the only thing that
    /// can fail in `ExclusiveDevice::new_no_delay`. Indicates a GPIO
    /// configuration bug, not a runtime fault.
    SpiConfig,
    /// `Mfrc522::init` or a subsequent command returned an error code.
    /// The wrapped string captures the variant; we don't propagate the
    /// generic `mfrc522::Error<E>` because its bus-error variant carries
    /// an SPI-bus error type that bleeds the entire generic chain into
    /// every call site.
    Chip(&'static str),
}

impl<E> From<MfrcLibError<E>> for RfidError {
    fn from(err: MfrcLibError<E>) -> Self {
        // Tags taken verbatim from `mfrc522::error::Error` so the
        // serial log line maps 1:1 to a datasheet condition.
        let tag = match err {
            MfrcLibError::Bcc => "bcc",
            MfrcLibError::BufferOverflow => "buffer_overflow",
            MfrcLibError::Collision => "collision",
            MfrcLibError::Comm(_) => "comm",
            MfrcLibError::Crc => "crc",
            MfrcLibError::IncompleteFrame => "incomplete_frame",
            MfrcLibError::Nak => "nak",
            MfrcLibError::NoRoom => "no_room",
            MfrcLibError::Overheating => "overheating",
            MfrcLibError::Parity => "parity",
            MfrcLibError::Proprietary => "proprietary",
            MfrcLibError::Protocol => "protocol",
            MfrcLibError::Timeout => "timeout",
            MfrcLibError::Wr => "wr",
        };
        Self::Chip(tag)
    }
}

/// Card UID, type-erased over MIFARE Classic / Plus / Ultralight (4 /
/// 7 / 10 bytes). The fixed `[u8; 10]` matches the `Event::CardScanned`
/// variant in [`crate::proto::events`], so a UID can travel through the
/// pubsub channel without a heapless::Vec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardUid {
    pub bytes: [u8; 10],
    pub len: u8,
}

impl CardUid {
    fn from_mfrc(uid: &Uid) -> Self {
        let src = uid.as_bytes();
        let mut bytes = [0u8; 10];
        let len = src.len().min(bytes.len());
        bytes[..len].copy_from_slice(&src[..len]);
        Self {
            bytes,
            len: len as u8,
        }
    }
}

/// Trait the rfid task consumes. A host-side mock under
/// `feature = "mock-hardware"` injects scripted UIDs without an SPI bus.
pub trait RfidReader {
    /// Report the MFRC522 silicon version (0x91 or 0x92 on genuine
    /// parts; clones often report 0x12 or 0xFF). Used once at boot for
    /// a sanity log line.
    fn version(&mut self) -> Option<u8>;

    /// One poll cycle. Returns `Ok(Some(uid))` when a card was present
    /// and selected, `Ok(None)` when nothing was in range, `Err(_)`
    /// when the chip itself failed (parity / protocol / timeout).
    fn poll_card(&mut self) -> Result<Option<CardUid>, RfidError>;

    /// HALT the most recently selected card so the next poll won't
    /// re-detect the same one. Errors are intentionally swallowed by
    /// the caller — failing to halt is harmless, the next REQA will
    /// just re-trigger.
    fn halt(&mut self) -> Result<(), RfidError>;
}

/// Concrete RFID driver. Holds the initialised MFRC522 chip handle and
/// keeps the hardware-reset pin alive in its "released" (logic-high)
/// state so the chip is not held in reset.
pub struct RfidDriver {
    inner: RfidChip,
    /// Held high for the lifetime of the driver. The `_` prefix keeps
    /// clippy quiet about a structurally-unused field — its job is to
    /// own the GPIO so it isn't reverted to input when `Output::drop`
    /// would otherwise fire.
    _rst: Output<'static>,
}

impl RfidDriver {
    /// Build the driver from the SPI2 bus, the CS pin and the
    /// already-driven-high RST pin.
    pub fn new(
        spi: Spi<'static, Blocking>,
        cs: Output<'static>,
        rst: Output<'static>,
    ) -> Result<Self, RfidError> {
        let device = ExclusiveDevice::new_no_delay(spi, cs).map_err(|_| RfidError::SpiConfig)?;
        let interface = SpiInterface::new(device);
        let inner = Mfrc522::new(interface).init()?;
        Ok(Self { inner, _rst: rst })
    }
}

impl RfidReader for RfidDriver {
    fn version(&mut self) -> Option<u8> {
        self.inner.version().ok()
    }

    fn poll_card(&mut self) -> Result<Option<CardUid>, RfidError> {
        // `new_card_present` issues a REQA. A "Timeout" error here is
        // the **normal** outcome when no card is present — surface it
        // as `Ok(None)` rather than propagating, otherwise every
        // empty-air poll would log noise.
        let atqa = match self.inner.new_card_present() {
            Ok(atqa) => atqa,
            Err(MfrcLibError::Timeout) => return Ok(None),
            Err(other) => return Err(other.into()),
        };

        // A card answered REQA; run anticollision + SELECT to lock its
        // UID. Collisions in this step also fold into `Ok(None)` — they
        // indicate two cards in the field, which the user resolves by
        // separating them; we shouldn't blink red for it.
        match self.inner.select(&atqa) {
            Ok(uid) => Ok(Some(CardUid::from_mfrc(&uid))),
            Err(MfrcLibError::Collision) | Err(MfrcLibError::Timeout) => Ok(None),
            Err(other) => Err(other.into()),
        }
    }

    fn halt(&mut self) -> Result<(), RfidError> {
        self.inner.hlta().map_err(Into::into)
    }
}

// ---------------------------------------------------------------------
// Host-side mock for unit tests. Lets the rfid task be driven against a
// scripted sequence of UIDs / "empty air" responses without an SPI bus.
// ---------------------------------------------------------------------
#[cfg(feature = "mock-hardware")]
pub mod mock {
    use super::{CardUid, RfidError, RfidReader};

    /// Trivial mock — yields a scripted sequence of poll results and
    /// counts how many times each method was called.
    pub struct MockRfid {
        pub script: heapless::Vec<Result<Option<CardUid>, RfidError>, 8>,
        pub version_reported: u8,
        pub polls: u32,
        pub halts: u32,
    }

    impl Default for MockRfid {
        fn default() -> Self {
            Self {
                script: heapless::Vec::new(),
                version_reported: 0x91,
                polls: 0,
                halts: 0,
            }
        }
    }

    impl RfidReader for MockRfid {
        fn version(&mut self) -> Option<u8> {
            Some(self.version_reported)
        }

        fn poll_card(&mut self) -> Result<Option<CardUid>, RfidError> {
            self.polls = self.polls.wrapping_add(1);
            if self.script.is_empty() {
                Ok(None)
            } else {
                Ok(self.script.remove(0)?)
            }
        }

        fn halt(&mut self) -> Result<(), RfidError> {
            self.halts = self.halts.wrapping_add(1);
            Ok(())
        }
    }
}
