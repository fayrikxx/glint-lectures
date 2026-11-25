use std::mem;
use std::time::Duration;

use crate::analysis::analysis_image;
use crate::blocks::{BLOCK_SIZE, block_columns, changed_blocks, channel_change};
use crate::{Analysis, AnalysisState, BgraFrame, CropArea, RgbaImage};

/// One thing that happened in a recorded capture.
#[derive(Debug, Clone, Copy)]
pub enum Step<'a> {
    Frame {
        frame: &'a BgraFrame,
        area: CropArea,
    },
    Pause,
    Resume,
}

/// Runs a recorded capture through [`Analysis`] the way the analysis thread does, and
/// keeps numbers about it: when the state changed, the gaps between frames, and how much
/// the picture shimmers while the analysis sees no change.
#[derive(Debug)]
pub struct Replay {
    analysis: Analysis,
    changes: Vec<(Duration, AnalysisState)>,
    frames: usize,
    last_frame: Option<Duration>,
    gaps: Vec<Duration>,
    /// How many pixel pairs of still frames had each biggest channel difference.
    noise: [u64; 256],
    /// The last frame's analysis image, what it was made from, and its changed blocks.
    image: RgbaImage,
    geometry: Option<(u32, u32, CropArea)>,
    blocks: Vec<bool>,
    scratch: RgbaImage,
}

impl Default for Replay {
    fn default() -> Self {
        let analysis = Analysis::new();
        Self {
            changes: vec![(analysis.since(), analysis.state())],
            analysis,
            frames: 0,
            last_frame: None,
            gaps: Vec::new(),
            noise: [0; 256],
            image: RgbaImage::default(),
            geometry: None,
            blocks: Vec::new(),
            scratch: RgbaImage::default(),
        }
    }
}

impl Replay {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds what happened at `now`. Steps have to come in time order.
    pub fn step(&mut self, now: Duration, step: Step<'_>) {
        self.analysis.tick(now);
        self.note_change();
        match step {
            Step::Frame { frame, area } => self.frame(now, frame, area),
            Step::Pause => self.analysis.set_running(now, false),
            Step::Resume => self.analysis.set_running(now, true),
        }
        self.note_change();
    }

    /// The last frame's analysis image with the blocks that changed tinted red, or `None`
    /// if none did. `replay --debug` saves these.
    pub fn changed_blocks_image(&self) -> Option<RgbaImage> {
        if !self.blocks.contains(&true) {
            return None;
        }
        let mut image = self.image.clone();
        let (width, height) = (image.width as usize, image.height as usize);
        let columns = block_columns(image.width);
        for (index, _) in self
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, changed)| **changed)
        {
            let (left, top) = (index % columns * BLOCK_SIZE, index / columns * BLOCK_SIZE);
            for y in top..(top + BLOCK_SIZE).min(height) {
                for x in left..(left + BLOCK_SIZE).min(width) {
                    let pixel = &mut image.pixels[(y * width + x) * 4..][..3];
                    // Halfway to red, so the slide still shows through.
                    pixel[0] = pixel[0] / 2 + 128;
                    pixel[1] /= 2;
                    pixel[2] /= 2;
                }
            }
        }
        Some(image)
    }

    /// Lets the time run to `end` and sums everything up.
    pub fn finish(mut self, end: Duration) -> ReplayReport {
        self.analysis.tick(end);
        self.note_change();
        ReplayReport {
            frames: self.frames,
            end,
            changes: self.changes,
            gaps: self.gaps,
            noise: self.noise,
        }
    }

    fn frame(&mut self, now: Duration, frame: &BgraFrame, area: CropArea) {
        if let Some(last) = self.last_frame {
            self.gaps.push(now.saturating_sub(last));
        }
        self.last_frame = Some(now);
        self.frames += 1;

        let was_still = self.analysis.state() == AnalysisState::Still;
        self.analysis.push(frame, area, now);
        let still = self.analysis.state() == AnalysisState::Still;

        // The analysis keeps its images to itself, so the replay makes its own copy to
        // measure. It costs a second shrink per frame, which only matters offline.
        analysis_image(frame, area, &mut self.scratch);
        let geometry = Some((frame.width(), frame.height(), area));
        if geometry == self.geometry {
            self.blocks = changed_blocks(&self.image, &self.scratch);
            if was_still && still {
                self.count_noise();
            }
        } else {
            self.blocks.clear();
        }
        self.geometry = geometry;
        mem::swap(&mut self.image, &mut self.scratch);
    }

    fn count_noise(&mut self) {
        let before = self.image.pixels.as_chunks::<4>().0;
        let after = self.scratch.pixels.as_chunks::<4>().0;
        for (a, b) in before.iter().zip(after) {
            self.noise[usize::from(channel_change(a, b))] += 1;
        }
    }

    fn note_change(&mut self) {
        let state = self.analysis.state();
        if self.changes.last().map(|&(_, last)| last) != Some(state) {
            self.changes.push((self.analysis.since(), state));
        }
    }
}

/// What a replayed capture did.
#[derive(Debug, Clone)]
pub struct ReplayReport {
    pub frames: usize,
    /// When the capture ended, counted from its start.
    pub end: Duration,
    /// Every state the analysis went through, with the time it began.
    pub changes: Vec<(Duration, AnalysisState)>,
    gaps: Vec<Duration>,
    noise: [u64; 256],
}

impl ReplayReport {
    /// How many times the analysis entered `state`.
    pub fn times_in(&self, state: AnalysisState) -> usize {
        self.changes.iter().filter(|(_, s)| *s == state).count()
    }

    /// How long the analysis spent in `state` in total.
    pub fn time_in(&self, state: AnalysisState) -> Duration {
        let ends = self
            .changes
            .iter()
            .skip(1)
            .map(|&(at, _)| at)
            .chain([self.end]);
        self.changes
            .iter()
            .zip(ends)
            .filter(|((_, s), _)| *s == state)
            .map(|(&(start, _), end)| end.saturating_sub(start))
            .sum()
    }

    /// The gap between frames that `fraction` of all gaps are shorter than or equal to.
    pub fn gap_percentile(&self, fraction: f64) -> Option<Duration> {
        let mut gaps = self.gaps.clone();
        gaps.sort();
        let index = ((gaps.len() as f64 * fraction).ceil() as usize).checked_sub(1)?;
        gaps.get(index.min(gaps.len() - 1)).copied()
    }

    /// The biggest channel difference, out of 255, that `fraction` of the pixels of still
    /// frames stay at or under. This is the codec noise the change threshold has to clear.
    pub fn noise_percentile(&self, fraction: f64) -> Option<u8> {
        let total: u64 = self.noise.iter().sum();
        let wanted = (total as f64 * fraction).ceil().max(1.0) as u64;
        let mut seen = 0;
        for (value, &count) in self.noise.iter().enumerate() {
            seen += count;
            if seen >= wanted {
                return Some(value as u8);
            }
        }
        None
    }

    /// How many pixel pairs the noise numbers come from.
    pub fn noise_samples(&self) -> u64 {
        self.noise.iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{Rng, slide};

    const WHITE: [u8; 3] = [250, 250, 250];
    const INK: [u8; 3] = [30, 30, 60];

    fn at(seconds: f64) -> Duration {
        Duration::from_secs_f64(seconds)
    }

    fn page(lines: u32) -> BgraFrame {
        slide(640, 360, WHITE, INK, lines, 7).frame()
    }

    /// A lecture in miniature: two slide changes, a pause in between, and a slide that
    /// shimmers like a video stream for a while.
    fn lecture() -> Vec<(Duration, Option<BgraFrame>, Option<Step<'static>>)> {
        let shimmering = slide(640, 360, WHITE, INK, 4, 7);
        let mut rng = Rng::new(21);
        let mut steps = vec![
            (at(0.0), Some(page(3)), None),
            (at(5.0), Some(page(4)), None),
            (at(8.0), None, Some(Step::Pause)),
            (at(20.0), None, Some(Step::Resume)),
        ];
        for step in 0..20 {
            let frame = shimmering.noisy_frame(3.0, 0.0, &mut rng);
            steps.push((at(20.25 + f64::from(step) * 0.25), Some(frame), None));
        }
        steps.push((at(30.0), Some(page(5)), None));
        steps
    }

    fn replay(steps: &[(Duration, Option<BgraFrame>, Option<Step<'static>>)]) -> ReplayReport {
        let mut replay = Replay::new();
        for (now, frame, step) in steps {
            match (frame, step) {
                (Some(frame), _) => replay.step(
                    *now,
                    Step::Frame {
                        frame,
                        area: CropArea::WHOLE,
                    },
                ),
                (None, Some(step)) => replay.step(*now, *step),
                (None, None) => {}
            }
        }
        replay.finish(at(40.0))
    }

    #[test]
    fn replaying_twice_gives_the_same_story() {
        let steps = lecture();
        assert_eq!(replay(&steps).changes, replay(&steps).changes);
    }

    #[test]
    fn replay_matches_calling_the_analysis_directly() {
        let steps = lecture();
        let mut analysis = Analysis::new();
        let mut changes = vec![(analysis.since(), analysis.state())];
        let mut note = |analysis: &Analysis| {
            if changes.last().map(|&(_, state)| state) != Some(analysis.state()) {
                changes.push((analysis.since(), analysis.state()));
            }
        };
        for (now, frame, step) in &steps {
            analysis.tick(*now);
            note(&analysis);
            match (frame, step) {
                (Some(frame), _) => analysis.push(frame, CropArea::WHOLE, *now),
                (None, Some(Step::Pause)) => analysis.set_running(*now, false),
                (None, Some(Step::Resume)) => analysis.set_running(*now, true),
                _ => {}
            }
            note(&analysis);
        }
        analysis.tick(at(40.0));
        note(&analysis);

        assert_eq!(replay(&steps).changes, changes);
    }

    #[test]
    fn a_pause_stops_the_clock() {
        let report = replay(&lecture());
        let still = AnalysisState::Still;
        let changing = AnalysisState::Changing;
        let paused = AnalysisState::Paused;
        assert_eq!(
            report.changes,
            [
                (at(0.0), changing),
                (at(1.5), still),
                (at(5.0), changing),
                (at(6.5), still),
                (at(8.0), paused),
                // Still before the pause and nothing new after it.
                (at(20.0), still),
                (at(30.0), changing),
                (at(31.5), still),
            ]
        );
        assert_eq!(report.time_in(paused), at(12.0));
        assert_eq!(report.times_in(changing), 3);
    }

    #[test]
    fn codec_noise_is_measured_on_still_frames() {
        let report = replay(&lecture());
        // Noise of 3 levels per channel averages down to about 1.5 in the shrunk image.
        // The biggest of three channel differences then sits around 2-3 in the middle and
        // around 6 at the 99th percentile.
        let p50 = report.noise_percentile(0.5).unwrap();
        let p99 = report.noise_percentile(0.99).unwrap();
        assert!((1..=4).contains(&p50), "p50 {p50}");
        assert!((4..=9).contains(&p99), "p99 {p99}");
        assert!(report.noise_samples() > 0);
    }

    #[test]
    fn gaps_between_frames_are_counted() {
        let report = replay(&lecture());
        assert_eq!(report.frames, 23);
        assert_eq!(report.gap_percentile(0.5), Some(at(0.25)));
        assert_eq!(report.gap_percentile(1.0), Some(at(15.25)));
    }

    #[test]
    fn the_debug_image_shows_changed_blocks() {
        let mut replay = Replay::new();
        let area = CropArea::WHOLE;
        replay.step(
            at(0.0),
            Step::Frame {
                frame: &page(3),
                area,
            },
        );
        assert!(replay.changed_blocks_image().is_none());
        replay.step(
            at(5.0),
            Step::Frame {
                frame: &page(4),
                area,
            },
        );
        let image = replay.changed_blocks_image().unwrap();
        assert!(
            image
                .pixels
                .chunks(4)
                .any(|pixel| pixel[0] >= 128 && pixel[1] < 128)
        );
    }
}
