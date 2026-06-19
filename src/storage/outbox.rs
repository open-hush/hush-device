//! Bounded, drop-oldest ring buffer of unflushed device events.
//!
//! Events are buffered here when the network is down or the sync task has
//! not run yet, and drained on every successful `POST /v1/device/events`.
//!
//! ## Idempotency contract
//!
//! Each [`DeviceEvent`] carries a client-generated `eventId`. The buffer
//! **keeps that id stable across flush retries**: a batch is read with
//! [`Outbox::batch`] (which copies, leaving the events in place) and only
//! removed with [`Outbox::ack`] after the backend returns `202`. If the POST
//! times out, the next attempt resends the same `eventId`s and the backend
//! deduplicates on `(deviceId, eventId)` — no event is lost, none is
//! double-counted.
//!
//! ## Overflow
//!
//! The buffer is bounded ([`OUTBOX_CAPACITY`]). When full, the **oldest**
//! event is dropped to make room for the newest — a fresh card scan matters
//! more than a stale one, and dropping silently would hide data loss, so
//! [`Outbox::push`] reports whether it evicted.
//!
//! ## Durability (out of scope for OPE-47)
//!
//! This is the in-RAM tier. A reboot loses unflushed events. OPE-47's
//! acceptance only requires secret / deviceId / last-config to survive a
//! reboot, not the outbox; a flash-backed durable outbox (append + fsync to
//! the `storage` partition, recovery on boot) is tracked as a Phase-3
//! follow-up. See `PLAN.md`.

#![allow(dead_code)] // drained by the Xtensa `sync` task; wired incrementally.

use heapless::Deque;

use crate::proto::api::{DeviceEvent, MAX_EVENTS_PER_BATCH};

/// Maximum events buffered in RAM. Sized so a burst of card scans / button
/// presses during an offline window does not immediately start evicting; a
/// flush of up to [`MAX_EVENTS_PER_BATCH`] drains a chunk per cycle.
pub const OUTBOX_CAPACITY: usize = 64;

/// In-RAM, FIFO, drop-oldest event buffer.
pub struct Outbox {
    events: Deque<DeviceEvent, OUTBOX_CAPACITY>,
    /// Cumulative count of events evicted due to overflow, for observability
    /// (surfaced as a log line / `wifi_signal`-style health metric later).
    dropped: u32,
}

impl Outbox {
    pub const fn new() -> Self {
        Self {
            events: Deque::new(),
            dropped: 0,
        }
    }

    /// Append an event. If the buffer is full, evict the oldest first and
    /// return `true` to signal that a drop occurred.
    pub fn push(&mut self, event: DeviceEvent) -> bool {
        let mut dropped = false;
        if self.events.is_full() {
            // Capacity is non-zero, so a full deque always has a front.
            let _ = self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
            dropped = true;
        }
        // Cannot fail: we just guaranteed room above.
        let _ = self.events.push_back(event);
        dropped
    }

    /// Number of buffered events awaiting flush.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.events.is_full()
    }

    /// Cumulative number of events dropped to overflow since boot.
    pub fn dropped_count(&self) -> u32 {
        self.dropped
    }

    /// Copy up to [`MAX_EVENTS_PER_BATCH`] of the **oldest** events into a
    /// batch for `POST /v1/device/events`, leaving them in the buffer. The
    /// caller removes them with [`ack`](Self::ack) only after a `202`.
    pub fn batch(&self) -> heapless::Vec<DeviceEvent, MAX_EVENTS_PER_BATCH> {
        let mut out = heapless::Vec::new();
        for event in self.events.iter().take(MAX_EVENTS_PER_BATCH) {
            // `take(MAX_EVENTS_PER_BATCH)` bounds the count to the Vec's
            // capacity, so push cannot fail.
            let _ = out.push(event.clone());
        }
        out
    }

    /// Remove the `n` oldest events after a successful flush. `n` is the
    /// length the matching [`batch`](Self::batch) reported; passing a larger
    /// `n` than buffered drains everything and is a no-op past the end.
    pub fn ack(&mut self, n: usize) {
        for _ in 0..n {
            if self.events.pop_front().is_none() {
                break;
            }
        }
    }
}

impl Default for Outbox {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use heapless::String as HString;

    fn card_event(seq: u8) -> DeviceEvent {
        // Distinct eventId per seq so we can assert FIFO ordering survives.
        let mut id: HString<36> = HString::try_from("00000000-0000-0000-0000-0000000000").unwrap();
        // append two hex chars to keep it a stable 36-char-ish id (length is
        // not validated here; we only need uniqueness for assertions).
        let _ = core::fmt::Write::write_fmt(&mut id, format_args!("{seq:02x}"));
        DeviceEvent::CardScanned {
            event_id: id,
            ts: HString::try_from("2026-06-19T13:17:09Z").unwrap(),
            uid: HString::try_from("04a1b2c3d4e5").unwrap(),
        }
    }

    #[test]
    fn push_and_len_track() {
        let mut ob = Outbox::new();
        assert!(ob.is_empty());
        assert!(!ob.push(card_event(1)));
        assert!(!ob.push(card_event(2)));
        assert_eq!(ob.len(), 2);
        assert_eq!(ob.dropped_count(), 0);
    }

    #[test]
    fn overflow_drops_oldest_and_reports() {
        let mut ob = Outbox::new();
        for i in 0..OUTBOX_CAPACITY {
            assert!(!ob.push(card_event(i as u8)));
        }
        assert!(ob.is_full());
        // One past capacity: evicts the oldest, keeps newest, reports drop.
        assert!(ob.push(card_event(0xff)));
        assert_eq!(ob.len(), OUTBOX_CAPACITY);
        assert_eq!(ob.dropped_count(), 1);
        // The oldest (seq 0) must be gone; the front is now seq 1.
        let batch = ob.batch();
        assert!(batch[0].event_id().ends_with("01"));
    }

    #[test]
    fn batch_is_bounded_and_non_destructive() {
        let mut ob = Outbox::new();
        for i in 0..OUTBOX_CAPACITY {
            ob.push(card_event(i as u8));
        }
        let batch = ob.batch();
        assert_eq!(batch.len(), MAX_EVENTS_PER_BATCH);
        // batch() copies; nothing was removed.
        assert_eq!(ob.len(), OUTBOX_CAPACITY);
    }

    #[test]
    fn batch_preserves_fifo_order() {
        let mut ob = Outbox::new();
        ob.push(card_event(0xa1));
        ob.push(card_event(0xb2));
        ob.push(card_event(0xc3));
        let batch = ob.batch();
        assert!(batch[0].event_id().ends_with("a1"));
        assert!(batch[1].event_id().ends_with("b2"));
        assert!(batch[2].event_id().ends_with("c3"));
    }

    #[test]
    fn ack_removes_only_flushed_prefix() {
        let mut ob = Outbox::new();
        for i in 0..5 {
            ob.push(card_event(i));
        }
        let batch = ob.batch();
        let n = batch.len().min(3);
        ob.ack(n);
        assert_eq!(ob.len(), 2);
        // Remaining front is seq 3 (0,1,2 acked).
        assert!(ob.batch()[0].event_id().ends_with("03"));
    }

    #[test]
    fn ack_past_end_is_safe() {
        let mut ob = Outbox::new();
        ob.push(card_event(1));
        ob.ack(100);
        assert!(ob.is_empty());
    }

    #[test]
    fn retry_resends_same_event_ids_until_acked() {
        // Models a flush that times out then succeeds: batch() twice yields
        // identical eventIds, so the backend dedups; ack() only after 202.
        let mut ob = Outbox::new();
        ob.push(card_event(7));
        let first = ob.batch();
        let second = ob.batch();
        assert_eq!(first[0].event_id(), second[0].event_id());
        ob.ack(second.len());
        assert!(ob.is_empty());
    }
}
