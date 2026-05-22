//! RFID task — polls the MFRC522 for cards and publishes events.
//!
//! See `crate::hw::mfrc522` for the "why polling, not IRQ" trade-off.
//!
//! Stack: 4 KiB. The body does no heap allocation, but the polling path
//! goes through `mfrc522`'s anticollision logic which keeps a few
//! hundred bytes of stack-resident frame; 4 KiB leaves comfortable
//! headroom and matches the figure in `PLAN.md`.

use embassy_time::{Duration, Timer};
use log::{info, warn};

use crate::{
    hw::mfrc522::{CardUid, RfidDriver, RfidReader},
    proto::events::{EVENT_BUS, Event},
};

/// Stack size justification: see module docstring.
const RFID_TASK_STACK: usize = 4096;

/// Poll cadence. 100 ms gives sub-100 ms scan latency end-to-end (the
/// MFRC522 antenna-up window is ~5 ms inside `new_card_present`), well
/// within the UX target of "tap and it plays". When the IRQ path lands
/// this becomes a fallback timer rather than the primary trigger.
pub const POLL_INTERVAL_MS: u64 = 100;

#[embassy_executor::task]
pub async fn rfid_task(mut driver: RfidDriver) {
    // Bring-up log line — also confirms the SPI bus and the MFRC522
    // are talking. Genuine NXP MFRC522 silicon reports 0x91 / 0x92;
    // clones commonly report 0x12 or 0x88.
    match driver.version() {
        Some(v) => info!("rfid: mfrc522 reported version 0x{:02X}", v),
        None => warn!("rfid: mfrc522 version readback failed"),
    }

    let publisher = EVENT_BUS
        .publisher()
        .expect("event bus out of publisher slots — bump EVENT_BUS_PUBLISHERS");

    // `last_uid` lets us de-duplicate scans when the same card is held
    // on the reader across multiple polls. We only publish `CardScanned`
    // when the visible UID changes; "card removed" debouncing is left
    // to the consumer of `CardLost` (not emitted yet — see PLAN.md).
    let mut last_uid: Option<CardUid> = None;

    loop {
        match driver.poll_card() {
            Ok(Some(uid)) => {
                if Some(uid) != last_uid {
                    info!(
                        "rfid: card present uid={:02X?} len={}",
                        &uid.bytes[..uid.len as usize],
                        uid.len
                    );
                    publisher
                        .publish(Event::CardScanned {
                            uid: uid.bytes,
                            uid_len: uid.len,
                        })
                        .await;
                    last_uid = Some(uid);
                }
                // HALT the card so the next REQA does not re-pick the
                // same one immediately; failure is logged at debug and
                // ignored — the next REQA / collision flow self-heals.
                if let Err(err) = driver.halt() {
                    warn!("rfid: halt failed: {err:?}");
                }
            }
            Ok(None) => {
                // No card in range. Reset the dedup window so removing
                // and re-presenting the same card publishes a new
                // CardScanned. Latency: one POLL_INTERVAL_MS tick.
                last_uid = None;
            }
            Err(err) => {
                warn!("rfid: poll failed: {err:?}");
            }
        }

        Timer::after(Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
}

// Keep the stack-size constant referenced — same rationale as in the
// led task: embassy doesn't consume it directly from the macro yet,
// but the value should not be silently deleted.
const _RFID_TASK_STACK_REF: usize = RFID_TASK_STACK;
