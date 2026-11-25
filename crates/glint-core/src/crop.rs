use std::ops::Range;

/// A part of a frame given as fractions of its width and height, so it stays on the same
/// part of the window when the window is resized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropArea {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl CropArea {
    pub const WHOLE: Self = Self {
        left: 0.0,
        top: 0.0,
        right: 1.0,
        bottom: 1.0,
    };

    /// Returns `None` unless `0 <= left < right <= 1` and `0 <= top < bottom <= 1`.
    pub fn new(left: f64, top: f64, right: f64, bottom: f64) -> Option<Self> {
        let inside = |edge: f64| (0.0..=1.0).contains(&edge);
        let valid =
            [left, top, right, bottom].into_iter().all(inside) && left < right && top < bottom;
        valid.then_some(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    pub fn left(&self) -> f64 {
        self.left
    }

    pub fn top(&self) -> f64 {
        self.top
    }

    pub fn right(&self) -> f64 {
        self.right
    }

    pub fn bottom(&self) -> f64 {
        self.bottom
    }

    /// The columns and rows of a `width` x `height` frame that the area touches.
    pub(crate) fn pixels(&self, width: u32, height: u32) -> (Range<usize>, Range<usize>) {
        (
            span(self.left, self.right, width),
            span(self.top, self.bottom, height),
        )
    }
}

/// The pixels between the fractions `from` and `to` of `size`. Partly covered pixels
/// count, so the span is never empty unless `size` is 0.
fn span(from: f64, to: f64, size: u32) -> Range<usize> {
    let size = size as usize;
    if size == 0 {
        return 0..0;
    }
    let start = ((from * size as f64).floor() as usize).min(size - 1);
    let end = ((to * size as f64).ceil() as usize).clamp(start + 1, size);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(left: f64, top: f64, right: f64, bottom: f64) -> CropArea {
        CropArea::new(left, top, right, bottom).unwrap()
    }

    #[test]
    fn accepts_the_whole_frame() {
        assert!(CropArea::new(0.0, 0.0, 1.0, 1.0).is_some());
    }

    #[test]
    fn rejects_edges_outside_the_frame() {
        assert!(CropArea::new(-0.1, 0.0, 1.0, 1.0).is_none());
        assert!(CropArea::new(0.0, -0.1, 1.0, 1.0).is_none());
        assert!(CropArea::new(0.0, 0.0, 1.1, 1.0).is_none());
        assert!(CropArea::new(0.0, 0.0, 1.0, 1.1).is_none());
    }

    #[test]
    fn rejects_empty_and_flipped_areas() {
        assert!(CropArea::new(0.5, 0.0, 0.5, 1.0).is_none());
        assert!(CropArea::new(0.6, 0.0, 0.4, 1.0).is_none());
        assert!(CropArea::new(0.0, 0.5, 1.0, 0.5).is_none());
        assert!(CropArea::new(0.0, 0.6, 1.0, 0.4).is_none());
    }

    #[test]
    fn rejects_nan() {
        assert!(CropArea::new(f64::NAN, 0.0, 1.0, 1.0).is_none());
        assert!(CropArea::new(0.0, 0.0, 1.0, f64::NAN).is_none());
    }

    #[test]
    fn whole_frame_covers_every_pixel() {
        assert_eq!(
            area(0.0, 0.0, 1.0, 1.0).pixels(1920, 1080),
            (0..1920, 0..1080)
        );
    }

    #[test]
    fn exact_fractions_give_exact_pixels() {
        assert_eq!(
            area(0.25, 0.5, 0.75, 1.0).pixels(1920, 1080),
            (480..1440, 540..1080)
        );
    }

    #[test]
    fn partly_covered_pixels_are_kept() {
        // 0.15 and 0.35 of 10 pixels land in the middle of pixels 1 and 3.
        assert_eq!(area(0.15, 0.15, 0.35, 0.35).pixels(10, 10), (1..4, 1..4));
    }

    #[test]
    fn a_tiny_frame_still_gets_a_pixel() {
        assert_eq!(area(0.2, 0.2, 0.3, 0.3).pixels(1, 1), (0..1, 0..1));
    }

    #[test]
    fn an_empty_frame_gives_no_pixels() {
        assert_eq!(area(0.0, 0.0, 1.0, 1.0).pixels(0, 0), (0..0, 0..0));
    }
}
