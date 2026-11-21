/// A grayscale frame, one byte per pixel, stored row by row.
///
/// Slide detection works on these rather than on captured [`BgraFrame`](crate::BgraFrame)s,
/// so it never has to care about color or the full capture resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Frame {
    /// Returns `None` if `pixels` doesn't hold exactly `width * height` bytes.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let expected = (width as usize).checked_mul(height as usize)?;
        (pixels.len() == expected).then_some(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<u8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.pixels
            .get(y as usize * self.width as usize + x as usize)
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_buffer_size() {
        assert!(Frame::new(4, 3, vec![0; 11]).is_none());
        assert!(Frame::new(4, 3, vec![0; 13]).is_none());
        assert!(Frame::new(4, 3, vec![0; 12]).is_some());
    }

    #[test]
    fn reads_pixels_row_by_row() {
        let frame = Frame::new(3, 2, vec![1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(frame.pixel(0, 0), Some(1));
        assert_eq!(frame.pixel(2, 0), Some(3));
        assert_eq!(frame.pixel(0, 1), Some(4));
        assert_eq!(frame.pixel(2, 1), Some(6));
    }

    #[test]
    fn out_of_bounds_pixel_is_none() {
        let frame = Frame::new(3, 2, vec![0; 6]).unwrap();
        assert_eq!(frame.pixel(3, 0), None);
        assert_eq!(frame.pixel(0, 2), None);
    }

    #[test]
    fn empty_frame_is_allowed() {
        let frame = Frame::new(0, 0, Vec::new()).unwrap();
        assert_eq!(frame.pixel(0, 0), None);
    }
}
