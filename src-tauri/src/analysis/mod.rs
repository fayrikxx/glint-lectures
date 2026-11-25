use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use glint_core::{Analysis, AnalysisState, CropArea, SlotWait};
use serde::Serialize;

use crate::capture::{CaptureFeed, CaptureState};

#[cfg(all(windows, debug_assertions))]
mod dump;
// Recording frames is a dev tool. It needs the png crate, which the app only pulls in on
// Windows.
#[cfg(not(all(windows, debug_assertions)))]
mod dump {
    use std::time::Duration;

    use glint_core::{BgraFrame, CropArea};

    pub(super) struct Recorder;

    impl Recorder {
        pub(super) fn from_env() -> Self {
            Self
        }

        pub(super) fn frame(
            &mut self,
            _frame: &BgraFrame,
            _crop: Option<CropArea>,
            _now: Duration,
        ) {
        }

        pub(super) fn running(&mut self, _now: Duration, _running: bool) {}

        pub(super) fn status(&self) -> Option<String> {
            None
        }
    }
}

use dump::Recorder;

/// The longest the thread waits for a frame. Capture state changes don't wake it, so this
/// is how soon it notices that the window got minimized or covered.
const STATE_CHECK: Duration = Duration::from_millis(500);

/// What a capture session's analysis thread shares with the commands.
pub struct Shared {
    /// The part of the window slides are read from. `None` means the whole window.
    crop: Mutex<Option<CropArea>>,
    status: Mutex<Status>,
}

struct Status {
    state: AnalysisState,
    since: Instant,
    recording: Option<String>,
}

/// The analysis state the way the UI gets it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisReport {
    state: &'static str,
    /// How long the state has lasted so far.
    seconds: f64,
    /// What the frame recorder of dev builds is doing, when it's switched on.
    recording: Option<String>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            crop: Mutex::new(None),
            status: Mutex::new(Status {
                state: AnalysisState::Changing,
                since: Instant::now(),
                recording: None,
            }),
        }
    }
}

impl Shared {
    pub fn crop(&self) -> Option<CropArea> {
        *lock(&self.crop)
    }

    pub fn set_crop(&self, crop: Option<CropArea>) {
        *lock(&self.crop) = crop;
    }

    pub fn report(&self) -> AnalysisReport {
        let status = lock(&self.status);
        AnalysisReport {
            state: match status.state {
                AnalysisState::Still => "still",
                AnalysisState::Changing => "changing",
                AnalysisState::Paused => "paused",
            },
            seconds: status.since.elapsed().as_secs_f64(),
            recording: status.recording.clone(),
        }
    }
}

/// The analysis thread of one capture session. Dropping it waits for the thread, which
/// ends once the session's frame slot is closed.
pub struct AnalysisThread {
    thread: Option<JoinHandle<()>>,
}

impl Drop for AnalysisThread {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start(feed: Arc<CaptureFeed>, shared: Arc<Shared>) -> Result<AnalysisThread, String> {
    let thread = thread::Builder::new()
        .name("glint-analysis".to_string())
        .spawn(move || run(&feed, &shared))
        .map_err(|err| format!("failed to start the analysis thread: {err}"))?;
    Ok(AnalysisThread {
        thread: Some(thread),
    })
}

fn run(feed: &CaptureFeed, shared: &Shared) {
    let started = Instant::now();
    let mut analysis = Analysis::new();
    let mut pause = PauseRule::default();
    let mut recorder = Recorder::from_env();
    let mut running = true;
    let mut last_id = 0;

    loop {
        let wait = match analysis.next_deadline() {
            Some(deadline) => deadline.saturating_sub(started.elapsed()).min(STATE_CHECK),
            None => STATE_CHECK,
        };
        let frame = match feed.frames.wait_newer(last_id, wait) {
            SlotWait::Frame { id, frame } => {
                last_id = id;
                Some(frame)
            }
            SlotWait::TimedOut => None,
            SlotWait::Closed => break,
        };

        // Frames are timed when they get here. On Windows that can be up to one capture
        // tick (250 ms) after they were drawn, which is fine next to 1.5 s of stillness.
        let now = started.elapsed();
        let should_run = pause.should_run(&feed.state(), frame.is_some());
        if should_run != running {
            analysis.set_running(now, should_run);
            recorder.running(now, should_run);
            running = should_run;
        }
        // The full frame is let go of right after it's shrunk, so capture can reuse it.
        if let Some(frame) = frame {
            let crop = shared.crop();
            recorder.frame(&frame, crop, now);
            analysis.push(&frame, crop.unwrap_or(CropArea::WHOLE), now);
        }
        analysis.tick(now);

        *lock(&shared.status) = Status {
            state: analysis.state(),
            since: started + analysis.since(),
            recording: recorder.status(),
        };
    }
}

/// Decides whether analysis runs. A covered or hidden window keeps running as long as it
/// sends frames: Windows captures covered windows too, and only some apps stop drawing
/// them. A still slide sends no frames either, so a missing frame alone means nothing,
/// but one that came after the window stopped being fully captured does.
#[derive(Debug, Default)]
struct PauseRule {
    frame_since_capturing: bool,
}

impl PauseRule {
    fn should_run(&mut self, state: &CaptureState, got_frame: bool) -> bool {
        match state {
            CaptureState::Capturing => {
                self.frame_since_capturing = false;
                true
            }
            CaptureState::Covered | CaptureState::Hidden => {
                self.frame_since_capturing |= got_frame;
                self.frame_since_capturing
            }
            CaptureState::Minimized | CaptureState::WindowClosed | CaptureState::Failed { .. } => {
                self.frame_since_capturing = false;
                false
            }
        }
    }
}

// Every change made under these locks is a single assignment, so a panic elsewhere can't
// leave them half-updated and a poisoned lock is still fine to use.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_core::BgraFrame;

    #[test]
    fn pause_follows_the_window() {
        let mut rule = PauseRule::default();
        assert!(rule.should_run(&CaptureState::Capturing, false));
        assert!(!rule.should_run(&CaptureState::Covered, false));
        assert!(rule.should_run(&CaptureState::Covered, true));
        // It sent a frame while covered, so a still slide there still counts.
        assert!(rule.should_run(&CaptureState::Covered, false));
        assert!(!rule.should_run(&CaptureState::Minimized, false));
        assert!(rule.should_run(&CaptureState::Capturing, false));
    }

    #[test]
    fn a_hidden_window_is_treated_like_a_covered_one() {
        let mut rule = PauseRule::default();
        assert!(!rule.should_run(&CaptureState::Hidden, false));
        assert!(rule.should_run(&CaptureState::Hidden, true));
    }

    #[test]
    fn frames_from_an_earlier_cover_dont_count_later() {
        let mut rule = PauseRule::default();
        rule.should_run(&CaptureState::Covered, true);
        rule.should_run(&CaptureState::Capturing, false);
        assert!(!rule.should_run(&CaptureState::Covered, false));
    }

    #[test]
    fn closed_and_failed_captures_pause() {
        let mut rule = PauseRule::default();
        assert!(!rule.should_run(&CaptureState::WindowClosed, true));
        let failed = CaptureState::Failed {
            message: "lost".to_string(),
        };
        assert!(!rule.should_run(&failed, true));
    }

    #[test]
    fn the_thread_stops_when_the_frame_slot_closes() {
        let feed = Arc::new(CaptureFeed::new());
        let shared = Arc::new(Shared::default());
        let thread = start(Arc::clone(&feed), Arc::clone(&shared)).unwrap();
        feed.frames
            .publish(1, BgraFrame::new(64, 36, vec![200; 64 * 36 * 4]).unwrap());

        let closed_at = Instant::now();
        feed.frames.close();
        drop(thread);
        assert!(closed_at.elapsed() < Duration::from_secs(2));
    }
}
