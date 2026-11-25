use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::BgraFrame;

/// Holds the most recent captured frame. Capture writes into it, readers wait for something
/// newer than what they already have. A new frame replaces the old one, so a slow reader
/// skips frames instead of falling behind.
#[derive(Debug, Default)]
pub struct FrameSlot {
    state: Mutex<SlotState>,
    changed: Condvar,
}

#[derive(Debug, Default)]
struct SlotState {
    latest: Option<(u64, Arc<BgraFrame>)>,
    closed: bool,
}

#[derive(Debug)]
pub enum SlotWait {
    Frame { id: u64, frame: Arc<BgraFrame> },
    TimedOut,
    Closed,
}

impl FrameSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores `frame` as the latest one. Ids must only go up.
    ///
    /// Returns the frame it replaced when no reader still holds it, so the caller can
    /// reuse its buffer. After [`close`](Self::close) the frame is handed straight back.
    pub fn publish(&self, id: u64, frame: BgraFrame) -> Option<BgraFrame> {
        let mut state = self.lock();
        if state.closed {
            return Some(frame);
        }
        let previous = state.latest.replace((id, Arc::new(frame)));
        drop(state);
        self.changed.notify_all();
        previous.and_then(|(_, frame)| Arc::into_inner(frame))
    }

    /// Wakes every waiting reader and stops accepting frames.
    pub fn close(&self) {
        self.lock().closed = true;
        self.changed.notify_all();
    }

    /// Waits up to `timeout` for a frame with an id above `after`.
    pub fn wait_newer(&self, after: u64, timeout: Duration) -> SlotWait {
        let state = self.lock();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |state| {
                !state.closed && state.latest.as_ref().is_none_or(|(id, _)| *id <= after)
            })
            .unwrap_or_else(PoisonError::into_inner);

        if state.closed {
            return SlotWait::Closed;
        }
        match &state.latest {
            Some((id, frame)) if *id > after => SlotWait::Frame {
                id: *id,
                frame: Arc::clone(frame),
            },
            _ => SlotWait::TimedOut,
        }
    }

    // A panic while holding the lock can't leave the state half-updated: every change
    // under it is a single assignment. So a poisoned lock is still safe to use.
    fn lock(&self) -> MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Instant;

    fn frame(value: u8) -> BgraFrame {
        BgraFrame::new(1, 1, vec![value; 4]).unwrap()
    }

    fn expect_frame(wait: SlotWait) -> (u64, Arc<BgraFrame>) {
        match wait {
            SlotWait::Frame { id, frame } => (id, frame),
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn reader_gets_the_latest_frame_only() {
        let slot = FrameSlot::new();
        slot.publish(1, frame(1));
        slot.publish(2, frame(2));
        let (id, frame) = expect_frame(slot.wait_newer(0, Duration::ZERO));
        assert_eq!(id, 2);
        assert_eq!(frame.pixels()[0], 2);
    }

    #[test]
    fn times_out_when_nothing_newer_arrives() {
        let slot = FrameSlot::new();
        slot.publish(5, frame(5));
        assert!(matches!(
            slot.wait_newer(5, Duration::from_millis(10)),
            SlotWait::TimedOut
        ));
        assert!(matches!(
            FrameSlot::new().wait_newer(0, Duration::from_millis(10)),
            SlotWait::TimedOut
        ));
    }

    #[test]
    fn wakes_a_waiting_reader_on_publish() {
        let slot = Arc::new(FrameSlot::new());
        let reader = {
            let slot = Arc::clone(&slot);
            thread::spawn(move || slot.wait_newer(0, Duration::from_secs(10)))
        };
        thread::sleep(Duration::from_millis(20));
        slot.publish(1, frame(1));
        let (id, _) = expect_frame(reader.join().unwrap());
        assert_eq!(id, 1);
    }

    #[test]
    fn every_waiting_reader_gets_the_frame() {
        // The preview and the analysis thread wait on the same slot.
        let slot = Arc::new(FrameSlot::new());
        let readers: Vec<_> = (0..2)
            .map(|_| {
                let slot = Arc::clone(&slot);
                thread::spawn(move || slot.wait_newer(0, Duration::from_secs(10)))
            })
            .collect();
        thread::sleep(Duration::from_millis(20));
        slot.publish(1, frame(1));
        for reader in readers {
            let (id, frame) = expect_frame(reader.join().unwrap());
            assert_eq!(id, 1);
            assert_eq!(frame.pixels()[0], 1);
        }
    }

    #[test]
    fn close_wakes_a_waiting_reader() {
        let slot = Arc::new(FrameSlot::new());
        let reader = {
            let slot = Arc::clone(&slot);
            thread::spawn(move || {
                let started = Instant::now();
                (
                    slot.wait_newer(0, Duration::from_secs(10)),
                    started.elapsed(),
                )
            })
        };
        thread::sleep(Duration::from_millis(20));
        slot.close();
        let (wait, waited) = reader.join().unwrap();
        assert!(matches!(wait, SlotWait::Closed));
        assert!(waited < Duration::from_secs(5));
    }

    #[test]
    fn hands_back_the_replaced_frame_for_reuse() {
        let slot = FrameSlot::new();
        assert!(slot.publish(1, frame(1)).is_none());
        let previous = slot.publish(2, frame(2)).unwrap();
        assert_eq!(previous.pixels()[0], 1);
    }

    #[test]
    fn keeps_a_frame_that_a_reader_still_holds() {
        let slot = FrameSlot::new();
        slot.publish(1, frame(1));
        let (_, held) = expect_frame(slot.wait_newer(0, Duration::ZERO));
        assert!(slot.publish(2, frame(2)).is_none());
        assert_eq!(held.pixels()[0], 1);
    }

    #[test]
    fn closed_slot_rejects_frames() {
        let slot = FrameSlot::new();
        slot.close();
        assert!(slot.publish(1, frame(1)).is_some());
        assert!(matches!(
            slot.wait_newer(0, Duration::ZERO),
            SlotWait::Closed
        ));
    }
}
