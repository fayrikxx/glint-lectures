/// A rectangle in screen pixels. `right` and `bottom` are exclusive, as in Win32's `RECT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn is_empty(&self) -> bool {
        self.left >= self.right || self.top >= self.bottom
    }

    /// The overlap of the two rectangles, which may be empty.
    pub fn intersect(&self, other: &Rect) -> Rect {
        Rect {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        }
    }

    /// Whether `covers` together hide every pixel of this rectangle. An empty rectangle has
    /// nothing to show, so it counts as covered.
    pub fn is_covered_by(&self, covers: impl IntoIterator<Item = Rect>) -> bool {
        // What is still visible, as rectangles that don't overlap each other.
        let mut visible = Vec::new();
        if !self.is_empty() {
            visible.push(*self);
        }
        for cover in covers {
            if visible.is_empty() {
                break;
            }
            let mut rest = Vec::with_capacity(visible.len());
            for rect in &visible {
                rect.subtract_into(&cover, &mut rest);
            }
            visible = rest;
        }
        visible.is_empty()
    }

    /// Adds the parts of this rectangle that `cover` doesn't hide to `out`: up to four bands
    /// around the overlap.
    fn subtract_into(&self, cover: &Rect, out: &mut Vec<Rect>) {
        let overlap = self.intersect(cover);
        if overlap.is_empty() {
            out.push(*self);
            return;
        }
        let bands = [
            Rect {
                bottom: overlap.top,
                ..*self
            },
            Rect {
                top: overlap.bottom,
                ..*self
            },
            Rect {
                top: overlap.top,
                right: overlap.left,
                bottom: overlap.bottom,
                ..*self
            },
            Rect {
                left: overlap.right,
                top: overlap.top,
                bottom: overlap.bottom,
                ..*self
            },
        ];
        out.extend(bands.into_iter().filter(|band| !band.is_empty()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn intersect_keeps_the_overlap() {
        let overlap = rect(0, 0, 10, 10).intersect(&rect(5, -5, 20, 8));
        assert_eq!(overlap, rect(5, 0, 10, 8));
    }

    #[test]
    fn rects_that_only_touch_dont_overlap() {
        assert!(
            rect(0, 0, 10, 10)
                .intersect(&rect(10, 0, 20, 10))
                .is_empty()
        );
    }

    #[test]
    fn nothing_on_top_leaves_it_visible() {
        assert!(!rect(0, 0, 100, 100).is_covered_by([]));
    }

    #[test]
    fn a_bigger_rect_covers_it() {
        assert!(rect(10, 10, 90, 90).is_covered_by([rect(0, 0, 100, 100)]));
        assert!(rect(0, 0, 100, 100).is_covered_by([rect(0, 0, 100, 100)]));
    }

    #[test]
    fn a_rect_elsewhere_doesnt_cover_it() {
        assert!(!rect(0, 0, 100, 100).is_covered_by([rect(200, 0, 300, 100)]));
    }

    #[test]
    fn a_one_pixel_strip_is_still_visible() {
        assert!(!rect(0, 0, 100, 100).is_covered_by([rect(0, 0, 99, 100)]));
        assert!(!rect(0, 0, 100, 100).is_covered_by([rect(0, 1, 100, 100)]));
    }

    #[test]
    fn two_halves_cover_it_together() {
        let halves = [rect(-10, -10, 50, 110), rect(50, -10, 110, 110)];
        assert!(rect(0, 0, 100, 100).is_covered_by(halves));
    }

    #[test]
    fn a_gap_between_two_rects_is_visible() {
        let sides = [rect(0, 0, 49, 100), rect(51, 0, 100, 100)];
        assert!(!rect(0, 0, 100, 100).is_covered_by(sides));
    }

    #[test]
    fn a_hole_in_the_middle_is_visible_until_filled() {
        let ring = [
            rect(0, 0, 100, 40),
            rect(0, 60, 100, 100),
            rect(0, 40, 40, 60),
            rect(60, 40, 100, 60),
        ];
        assert!(!rect(0, 0, 100, 100).is_covered_by(ring));
        assert!(rect(0, 0, 100, 100).is_covered_by(ring.into_iter().chain([rect(40, 40, 60, 60)])));
    }

    #[test]
    fn many_small_tiles_cover_it() {
        let tiles = (0..10).flat_map(|row| {
            (0..10).map(move |col| rect(col * 10, row * 10, col * 10 + 10, row * 10 + 10))
        });
        assert!(rect(0, 0, 100, 100).is_covered_by(tiles));
    }

    #[test]
    fn an_empty_rect_counts_as_covered() {
        assert!(rect(0, 0, 0, 100).is_covered_by([]));
    }
}
