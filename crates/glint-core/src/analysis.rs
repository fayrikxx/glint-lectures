use std::mem;
use std::time::Duration;

use crate::blocks::changed_blocks;
use crate::{BgraFrame, CropArea, RgbaImage};

/// How long the picture has to stay the same before it counts as still.
const SETTLE_TIME: Duration = Duration::from_millis(1500);
/// The analysis image's long side stays at or under this, for areas twice as long and more.
const MAX_SIDE: usize = 768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisState {
    Still,
    Changing,
    /// Nobody can see the window right now (minimized, or covered and no longer drawn),
    /// so nothing it shows can settle.
    Paused,
}

/// Watches the slide area and tells whether it is still or changing.
///
/// Time comes from the caller, as the time since the capture started, so tests don't have
/// to sleep. A window that doesn't change sends no frames at all, so the caller also calls
/// [`tick`](Self::tick) once [`next_deadline`](Self::next_deadline) has passed.
#[derive(Debug)]
pub struct Analysis {
    state: AnalysisState,
    since: Duration,
    last_change: Duration,
    /// The state a pause interrupted.
    resume_to: AnalysisState,
    /// The analysis image of the last frame and the size and area it was made from.
    previous: RgbaImage,
    geometry: Option<Geometry>,
    scratch: RgbaImage,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Geometry {
    width: u32,
    height: u32,
    area: CropArea,
}

impl Default for Analysis {
    fn default() -> Self {
        // Nothing is known at the start, so the first picture has to settle like any other.
        Self {
            state: AnalysisState::Changing,
            since: Duration::ZERO,
            last_change: Duration::ZERO,
            resume_to: AnalysisState::Changing,
            previous: RgbaImage::default(),
            geometry: None,
            scratch: RgbaImage::default(),
        }
    }
}

impl Analysis {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> AnalysisState {
        self.state
    }

    /// When the current state began.
    pub fn since(&self) -> Duration {
        self.since
    }

    /// When the picture will count as still if nothing changes before then. `None` while
    /// it is still or paused: then nothing happens until a frame comes or the pause ends.
    pub fn next_deadline(&self) -> Option<Duration> {
        (self.state == AnalysisState::Changing).then(|| self.last_change + SETTLE_TIME)
    }

    /// Takes a frame that arrived at `now`, with `area` as the part of it slides are read
    /// from. Frames that come in while paused are ignored, so the first one after the
    /// pause is compared with the last one before it.
    pub fn push(&mut self, frame: &BgraFrame, area: CropArea, now: Duration) {
        self.tick(now);
        if self.state == AnalysisState::Paused {
            return;
        }

        let (columns, rows) = area.pixels(frame.width(), frame.height());
        let factor = shrink_factor(columns.len(), rows.len());
        frame.shrink_into(area, factor, &mut self.scratch);

        let geometry = Some(Geometry {
            width: frame.width(),
            height: frame.height(),
            area,
        });
        // A resized window or a new crop gives a picture to compare against from now on,
        // not a change: the old one isn't comparable pixel by pixel.
        if geometry == self.geometry && changed_blocks(&self.previous, &self.scratch) > 0 {
            self.changed(now);
        }
        self.geometry = geometry;
        mem::swap(&mut self.previous, &mut self.scratch);
    }

    /// Lets time pass: a changing picture becomes still once its deadline is reached.
    pub fn tick(&mut self, now: Duration) {
        if let Some(deadline) = self.next_deadline()
            && now >= deadline
        {
            self.state = AnalysisState::Still;
            self.since = deadline;
        }
    }

    /// Pauses or resumes. After a pause a changing picture needs the full settle time
    /// again, since nobody saw what the window did meanwhile.
    pub fn set_running(&mut self, now: Duration, running: bool) {
        let paused = self.state == AnalysisState::Paused;
        if running != paused {
            return;
        }
        if running {
            self.state = self.resume_to;
            self.last_change = now;
        } else {
            self.tick(now);
            self.resume_to = self.state;
            self.state = AnalysisState::Paused;
        }
        self.since = now;
    }

    fn changed(&mut self, now: Duration) {
        if self.state != AnalysisState::Changing {
            self.state = AnalysisState::Changing;
            self.since = now;
        }
        self.last_change = now;
    }
}

/// The shrink factor for an area of `width` x `height` pixels. It is at least 2, so
/// neighboring pixels always get averaged, which smooths out some compression noise.
fn shrink_factor(width: usize, height: usize) -> u32 {
    width.max(height).div_ceil(MAX_SIDE).max(2) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{Canvas, Rng, slide};
    use std::time::Instant;

    const WHITE: [u8; 3] = [250, 250, 250];
    const INK: [u8; 3] = [30, 30, 60];

    fn at(seconds: f64) -> Duration {
        Duration::from_secs_f64(seconds)
    }

    fn page(lines: u32) -> BgraFrame {
        slide(640, 360, WHITE, INK, lines, 7).frame()
    }

    /// An analysis that has seen `frame` at 0 s and settled.
    fn settled_on(frame: &BgraFrame) -> Analysis {
        let mut analysis = Analysis::new();
        analysis.push(frame, CropArea::WHOLE, at(0.0));
        analysis.tick(at(2.0));
        assert_eq!(analysis.state(), AnalysisState::Still);
        analysis
    }

    #[test]
    fn shrink_factor_keeps_the_long_side_in_range() {
        assert_eq!(shrink_factor(1, 1), 2);
        assert_eq!(shrink_factor(800, 450), 2);
        assert_eq!(shrink_factor(1300, 700), 2);
        assert_eq!(shrink_factor(1920, 1080), 3);
        assert_eq!(shrink_factor(2560, 1440), 4);
        assert_eq!(shrink_factor(3840, 2160), 5);
        assert_eq!(shrink_factor(1080, 1920), 3);
    }

    #[test]
    fn settles_after_a_change_without_any_more_frames() {
        let mut analysis = settled_on(&page(3));
        analysis.push(&page(4), CropArea::WHOLE, at(10.0));
        assert_eq!(analysis.state(), AnalysisState::Changing);
        assert_eq!(analysis.next_deadline(), Some(at(11.5)));

        analysis.tick(at(11.4));
        assert_eq!(analysis.state(), AnalysisState::Changing);
        analysis.tick(at(11.5));
        assert_eq!(analysis.state(), AnalysisState::Still);
        assert_eq!(analysis.since(), at(11.5));
    }

    #[test]
    fn a_late_frame_still_counts_the_stillness_before_it() {
        // The frame at 5 s changes the picture, but the picture had already been still
        // since 1.5 s. Missing that would lose a slide whenever ticks run late.
        let mut analysis = Analysis::new();
        analysis.push(&page(3), CropArea::WHOLE, at(0.0));
        analysis.push(&page(4), CropArea::WHOLE, at(5.0));
        assert_eq!(analysis.state(), AnalysisState::Changing);
        assert_eq!(analysis.since(), at(5.0));
    }

    #[test]
    fn nothing_is_scheduled_while_still() {
        let mut analysis = settled_on(&page(3));
        assert_eq!(analysis.next_deadline(), None);
        analysis.tick(at(600.0));
        assert_eq!(analysis.state(), AnalysisState::Still);
        assert_eq!(analysis.since(), at(1.5));
    }

    #[test]
    fn codec_noise_on_a_static_slide_stays_still() {
        let canvas = slide(640, 360, WHITE, INK, 6, 3);
        let mut rng = Rng::new(11);
        let mut analysis = Analysis::new();
        for step in 0..80 {
            let now = at(f64::from(step) * 0.25);
            // Every 2 s a keyframe shifts the whole picture by a few levels.
            let offset = if step % 8 == 0 { 3.0 } else { 0.0 };
            analysis.push(
                &canvas.noisy_frame(3.0, offset, &mut rng),
                CropArea::WHOLE,
                now,
            );
            analysis.tick(now);
            if now >= SETTLE_TIME {
                assert_eq!(analysis.state(), AnalysisState::Still, "at {now:?}");
            }
        }
    }

    #[test]
    fn a_new_line_on_a_dark_slide_is_a_change() {
        let dark = |lines| slide(640, 360, [25, 25, 25], [85, 85, 85], lines, 9).frame();
        let mut analysis = settled_on(&dark(2));
        analysis.push(&dark(3), CropArea::WHOLE, at(5.0));
        assert_eq!(analysis.state(), AnalysisState::Changing);
    }

    #[test]
    fn video_keeps_changing() {
        let mut rng = Rng::new(4);
        let mut canvas = Canvas::new(640, 360, [0, 0, 0]);
        let mut analysis = Analysis::new();
        for step in 0..40 {
            let now = at(f64::from(step) * 0.25);
            canvas.texture(0, 0, 640, 360, &mut rng);
            analysis.push(&canvas.frame(), CropArea::WHOLE, now);
            analysis.tick(now);
            assert_eq!(analysis.state(), AnalysisState::Changing, "at {now:?}");
        }
    }

    #[test]
    fn a_pause_stops_the_clock() {
        let mut analysis = settled_on(&page(3));
        analysis.push(&page(4), CropArea::WHOLE, at(10.0));
        analysis.set_running(at(10.5), false);
        assert_eq!(analysis.state(), AnalysisState::Paused);
        assert_eq!(analysis.next_deadline(), None);

        analysis.tick(at(60.0));
        assert_eq!(analysis.state(), AnalysisState::Paused);

        analysis.set_running(at(70.0), true);
        assert_eq!(analysis.state(), AnalysisState::Changing);
        assert_eq!(analysis.next_deadline(), Some(at(71.5)));
        analysis.tick(at(71.4));
        assert_eq!(analysis.state(), AnalysisState::Changing);
        analysis.tick(at(71.5));
        assert_eq!(analysis.state(), AnalysisState::Still);
    }

    #[test]
    fn a_still_picture_stays_still_after_a_pause() {
        let mut analysis = settled_on(&page(3));
        analysis.set_running(at(5.0), false);
        analysis.set_running(at(9.0), true);
        assert_eq!(analysis.state(), AnalysisState::Still);
        assert_eq!(analysis.since(), at(9.0));
        // The window looks the same as before the pause, so its first frame isn't a change.
        analysis.push(&page(3), CropArea::WHOLE, at(9.2));
        assert_eq!(analysis.state(), AnalysisState::Still);
    }

    #[test]
    fn a_pause_that_starts_after_the_deadline_settles_first() {
        let mut analysis = Analysis::new();
        analysis.push(&page(3), CropArea::WHOLE, at(0.0));
        analysis.set_running(at(4.0), false);
        analysis.set_running(at(8.0), true);
        assert_eq!(analysis.state(), AnalysisState::Still);
    }

    #[test]
    fn a_covered_window_that_keeps_drawing_settles_after_every_change() {
        // Covered windows that still send frames keep running, so each change settles.
        let mut analysis = settled_on(&page(2));
        let mut settled = 0;
        let mut last_state = analysis.state();
        for step in 20..80u32 {
            let now = at(f64::from(step) * 0.25);
            if step % 20 == 0 {
                analysis.push(&page(2 + step / 20), CropArea::WHOLE, now);
            }
            analysis.tick(now);
            if last_state == AnalysisState::Changing && analysis.state() == AnalysisState::Still {
                settled += 1;
            }
            last_state = analysis.state();
        }
        assert_eq!(settled, 3);
    }

    #[test]
    fn a_window_that_stops_drawing_mid_change_doesnt_settle_until_seen_again() {
        let mut analysis = settled_on(&page(3));
        analysis.push(&page(4), CropArea::WHOLE, at(10.0));
        // Covered at 10.3 s and the app stopped drawing it: no frames come, and the
        // half-finished picture must not count as still.
        analysis.set_running(at(10.3), false);
        analysis.tick(at(40.0));
        assert_eq!(analysis.state(), AnalysisState::Paused);

        analysis.set_running(at(50.0), true);
        analysis.push(&page(5), CropArea::WHOLE, at(50.25));
        assert_eq!(analysis.state(), AnalysisState::Changing);
        analysis.tick(at(51.7));
        assert_eq!(analysis.state(), AnalysisState::Changing);
        analysis.tick(at(51.75));
        assert_eq!(analysis.state(), AnalysisState::Still);
    }

    #[test]
    fn frames_during_a_pause_are_ignored() {
        let mut analysis = settled_on(&page(3));
        analysis.set_running(at(5.0), false);
        analysis.push(&page(6), CropArea::WHOLE, at(6.0));
        assert_eq!(analysis.state(), AnalysisState::Paused);
        analysis.set_running(at(7.0), true);
        // Compared with the frame from before the pause, not the ignored one.
        analysis.push(&page(6), CropArea::WHOLE, at(7.2));
        assert_eq!(analysis.state(), AnalysisState::Changing);
    }

    #[test]
    fn a_new_crop_or_window_size_is_not_a_change() {
        let mut analysis = settled_on(&page(3));
        let top_half = CropArea::new(0.0, 0.0, 1.0, 0.5).unwrap();
        analysis.push(&page(3), top_half, at(5.0));
        assert_eq!(analysis.state(), AnalysisState::Still);

        let bigger = slide(800, 450, WHITE, INK, 3, 7).frame();
        analysis.push(&bigger, top_half, at(6.0));
        assert_eq!(analysis.state(), AnalysisState::Still);

        // The new size is the reference from now on.
        let changed = slide(800, 450, WHITE, INK, 4, 7).frame();
        analysis.push(&changed, CropArea::WHOLE, at(7.0));
        assert_eq!(analysis.state(), AnalysisState::Still);
        analysis.push(&changed, CropArea::WHOLE, at(7.25));
        analysis.push(
            &slide(800, 450, WHITE, INK, 5, 7).frame(),
            CropArea::WHOLE,
            at(7.5),
        );
        assert_eq!(analysis.state(), AnalysisState::Changing);
    }

    #[test]
    fn tiny_frames_and_areas_dont_panic() {
        let mut analysis = Analysis::new();
        let one_pixel = BgraFrame::new(1, 1, vec![10, 20, 30, 255]).unwrap();
        analysis.push(&one_pixel, CropArea::WHOLE, at(0.0));
        analysis.push(
            &one_pixel,
            CropArea::new(0.4, 0.4, 0.41, 0.41).unwrap(),
            at(0.25),
        );
        analysis.push(
            &BgraFrame::new(0, 0, Vec::new()).unwrap(),
            CropArea::WHOLE,
            at(0.5),
        );
        analysis.push(
            &page(3),
            CropArea::new(0.5, 0.5, 0.5001, 0.5001).unwrap(),
            at(0.75),
        );
        analysis.tick(at(5.0));
        assert_eq!(analysis.state(), AnalysisState::Still);
    }

    #[test]
    #[ignore = "measures speed; run cargo test -p glint-core --release -- --ignored timing --nocapture"]
    fn timing_per_frame() {
        for (width, height) in [(1920, 1080), (2560, 1440), (3840, 2160)] {
            let frames = [
                slide(width, height, WHITE, INK, 6, 1).frame(),
                slide(width, height, WHITE, INK, 7, 1).frame(),
            ];
            let mut analysis = Analysis::new();
            let mut times = Vec::new();
            for step in 0..40u32 {
                let started = Instant::now();
                analysis.push(
                    &frames[step as usize % 2],
                    CropArea::WHOLE,
                    at(f64::from(step) * 0.25),
                );
                times.push(started.elapsed());
            }
            times.sort();
            let ms = |time: Duration| time.as_secs_f64() * 1000.0;
            println!(
                "{width}x{height}: median {:.2} ms, slowest {:.2} ms per frame",
                ms(times[times.len() / 2]),
                ms(times[times.len() - 1]),
            );
        }
    }
}
