use std::ops::Range;

use crate::CropArea;

/// A captured frame in the layout capture APIs hand out: BGRA, four bytes per pixel,
/// rows packed with no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgraFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// An RGBA image, four bytes per pixel, rows packed with no padding. This is the layout
/// a canvas `ImageData` expects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl BgraFrame {
    /// Returns `None` if `pixels` doesn't hold exactly `width * height * 4` bytes.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
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

    /// Gives the buffer back so the next frame can reuse it instead of allocating.
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// Copies the part of the frame inside `area`.
    pub fn crop(&self, area: CropArea) -> BgraFrame {
        let (columns, rows) = area.pixels(self.width, self.height);
        let (width, height) = (columns.len(), rows.len());
        let row_len = self.width as usize * 4;

        let mut pixels = Vec::with_capacity(width * height * 4);
        for y in rows {
            let row = &self.pixels[y * row_len..(y + 1) * row_len];
            pixels.extend_from_slice(&row[columns.start * 4..columns.end * 4]);
        }

        BgraFrame {
            width: width as u32,
            height: height as u32,
            pixels,
        }
    }

    /// Shrinks the frame to fit in `max_width` x `max_height` by averaging square blocks
    /// of pixels.
    ///
    /// The block size is a whole number, so the result can come out a bit smaller than
    /// the box. Averaging keeps thin text strokes visible, which plain sampling drops.
    /// Alpha is set to opaque: window captures don't carry a meaningful one.
    pub fn shrink_to_rgba(&self, max_width: u32, max_height: u32) -> RgbaImage {
        let factor = shrink_factor(self.width, self.height, max_width, max_height);
        let mut image = RgbaImage::default();
        self.shrink_part(
            0..self.width as usize,
            0..self.height as usize,
            factor,
            &mut image,
        );
        image
    }

    /// Cuts `area` out of the frame and shrinks it by averaging `factor` x `factor` blocks
    /// of pixels, the same way as [`shrink_to_rgba`](Self::shrink_to_rgba).
    ///
    /// Writes into `out`, so the caller can keep one image and reuse its buffer for every
    /// frame.
    pub fn shrink_into(&self, area: CropArea, factor: u32, out: &mut RgbaImage) {
        let (columns, rows) = area.pixels(self.width, self.height);
        self.shrink_part(columns, rows, factor.max(1) as usize, out);
    }

    fn shrink_part(
        &self,
        columns: Range<usize>,
        rows: Range<usize>,
        factor: usize,
        out: &mut RgbaImage,
    ) {
        let width = columns.len();
        let out_width = width.div_ceil(factor);
        let out_height = rows.len().div_ceil(factor);
        let row_len = self.width as usize * 4;

        out.width = out_width as u32;
        out.height = out_height as u32;
        out.pixels.clear();
        out.pixels.reserve(out_width * out_height * 4);
        let mut sums = vec![[0u32; 3]; out_width];

        for block_top in rows.clone().step_by(factor) {
            let block_bottom = (block_top + factor).min(rows.end);
            sums.fill([0; 3]);

            for y in block_top..block_bottom {
                let row =
                    &self.pixels[y * row_len + columns.start * 4..y * row_len + columns.end * 4];
                for (sum, block) in sums.iter_mut().zip(row.as_chunks::<4>().0.chunks(factor)) {
                    for px in block {
                        sum[0] += u32::from(px[0]);
                        sum[1] += u32::from(px[1]);
                        sum[2] += u32::from(px[2]);
                    }
                }
            }

            let block_rows = block_bottom - block_top;
            for (column, sum) in sums.iter().enumerate() {
                // The last column and row of blocks can be narrower than `factor`.
                let block_columns = factor.min(width - column * factor);
                let count = (block_rows * block_columns) as u32;
                let average = |total: u32| ((total + count / 2) / count) as u8;
                out.pixels.extend_from_slice(&[
                    average(sum[2]),
                    average(sum[1]),
                    average(sum[0]),
                    255,
                ]);
            }
        }
    }
}

/// Smallest whole block size that makes `width` x `height` fit in the box.
fn shrink_factor(width: u32, height: u32, max_width: u32, max_height: u32) -> usize {
    let by_width = width.div_ceil(max_width.max(1));
    let by_height = height.div_ceil(max_height.max(1));
    by_width.max(by_height).max(1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bgra(width: u32, height: u32, pixels: &[[u8; 4]]) -> BgraFrame {
        BgraFrame::new(width, height, pixels.concat()).unwrap()
    }

    #[test]
    fn rejects_wrong_buffer_size() {
        assert!(BgraFrame::new(2, 2, vec![0; 15]).is_none());
        assert!(BgraFrame::new(2, 2, vec![0; 17]).is_none());
        assert!(BgraFrame::new(2, 2, vec![0; 16]).is_some());
    }

    #[test]
    fn small_frame_is_only_converted_to_rgba() {
        let frame = bgra(2, 1, &[[10, 20, 30, 0], [40, 50, 60, 128]]);
        let image = frame.shrink_to_rgba(960, 960);
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixels, [30, 20, 10, 255, 60, 50, 40, 255]);
    }

    #[test]
    fn averages_each_block() {
        // 4x2 shrunk into 2x1: each output pixel is the average of a 2x2 block.
        let frame = bgra(
            4,
            2,
            &[
                [0, 0, 0, 255],
                [100, 0, 0, 255],
                [0, 0, 200, 255],
                [0, 0, 200, 255],
                [0, 0, 0, 255],
                [100, 0, 0, 255],
                [0, 40, 200, 255],
                [0, 40, 201, 255],
            ],
        );
        let image = frame.shrink_to_rgba(2, 2);
        assert_eq!((image.width, image.height), (2, 1));
        // First block: blue averages to 50. Second block: green 20, red 200.25 rounds to 200.
        assert_eq!(image.pixels, [0, 0, 50, 255, 200, 20, 0, 255]);
    }

    #[test]
    fn edge_blocks_average_only_the_pixels_they_cover() {
        // 3x3 into a 2x2 box: factor 2, so the right column and bottom row are 1 pixel wide.
        let mut pixels = vec![[0, 0, 0, 255]; 9];
        pixels[2] = [90, 90, 90, 255];
        pixels[5] = [30, 30, 30, 255];
        pixels[8] = [60, 60, 60, 255];
        let image = bgra(3, 3, &pixels).shrink_to_rgba(2, 2);
        assert_eq!((image.width, image.height), (2, 2));
        // Top-right block covers pixels 2 and 5: (90 + 30) / 2 = 60.
        assert_eq!(&image.pixels[4..8], &[60, 60, 60, 255]);
        // Bottom-right block is just pixel 8.
        assert_eq!(&image.pixels[12..16], &[60, 60, 60, 255]);
    }

    #[test]
    fn tall_frames_are_limited_by_height() {
        let frame = BgraFrame::new(10, 1000, vec![0; 10 * 1000 * 4]).unwrap();
        let image = frame.shrink_to_rgba(960, 960);
        assert_eq!((image.width, image.height), (5, 500));
        assert_eq!(image.pixels.len(), 5 * 500 * 4);
    }

    #[test]
    fn full_hd_fits_the_preview_box() {
        let frame = BgraFrame::new(1920, 1080, vec![0; 1920 * 1080 * 4]).unwrap();
        let image = frame.shrink_to_rgba(960, 960);
        assert_eq!((image.width, image.height), (960, 540));
    }

    #[test]
    fn crop_copies_the_area() {
        // Every pixel holds its own x and y, so the result shows where it came from.
        let pixels: Vec<[u8; 4]> = (0..4)
            .flat_map(|y| (0..4).map(move |x| [x, y, 0, 255]))
            .collect();
        let frame = bgra(4, 4, &pixels);
        let cropped = frame.crop(CropArea::new(0.25, 0.5, 0.75, 1.0).unwrap());
        assert_eq!((cropped.width(), cropped.height()), (2, 2));
        assert_eq!(
            cropped.pixels(),
            [
                [1, 2, 0, 255],
                [2, 2, 0, 255],
                [1, 3, 0, 255],
                [2, 3, 0, 255]
            ]
            .concat()
        );
    }

    #[test]
    fn crop_to_the_whole_frame_keeps_it() {
        let frame = bgra(
            2,
            2,
            &[
                [1, 2, 3, 4],
                [5, 6, 7, 8],
                [9, 10, 11, 12],
                [13, 14, 15, 16],
            ],
        );
        assert_eq!(
            frame.crop(CropArea::new(0.0, 0.0, 1.0, 1.0).unwrap()),
            frame
        );
    }

    #[test]
    fn empty_frame_gives_empty_image() {
        let image = BgraFrame::new(0, 0, Vec::new())
            .unwrap()
            .shrink_to_rgba(960, 960);
        assert_eq!((image.width, image.height), (0, 0));
        assert!(image.pixels.is_empty());
    }

    /// Every pixel holds 10 times its x and y in blue and green, so averages show where
    /// they came from. Big frames wrap around, which is fine where only sizes matter.
    fn numbered(width: u32, height: u32) -> BgraFrame {
        let pixels: Vec<[u8; 4]> = (0..height)
            .flat_map(|y| (0..width).map(move |x| [(x * 10) as u8, (y * 10) as u8, 0, 255]))
            .collect();
        bgra(width, height, &pixels)
    }

    #[test]
    fn shrink_into_averages_blocks_of_the_area() {
        // The right half of a 4x4 frame, shrunk by 2: one block of columns 2-3 and rows 0-1,
        // and one of rows 2-3. Blue holds x * 10, so it averages to 25; green holds y * 10.
        let mut image = RgbaImage::default();
        numbered(4, 4).shrink_into(CropArea::new(0.5, 0.0, 1.0, 1.0).unwrap(), 2, &mut image);
        assert_eq!((image.width, image.height), (1, 2));
        assert_eq!(image.pixels, [0, 5, 25, 255, 0, 25, 25, 255]);
    }

    #[test]
    fn shrink_into_handles_odd_sizes_at_the_frame_edge() {
        // The bottom-right 3x3 of a 5x5 frame: blocks of 2x2, 1x2, 2x1 and 1x1 pixels.
        let mut image = RgbaImage::default();
        numbered(5, 5).shrink_into(CropArea::new(0.4, 0.4, 1.0, 1.0).unwrap(), 2, &mut image);
        assert_eq!((image.width, image.height), (2, 2));
        let blue: Vec<u8> = image.pixels.chunks(4).map(|px| px[2]).collect();
        let green: Vec<u8> = image.pixels.chunks(4).map(|px| px[1]).collect();
        assert_eq!(blue, [25, 40, 25, 40]);
        assert_eq!(green, [25, 25, 40, 40]);
    }

    #[test]
    fn shrink_into_matches_shrink_to_rgba_on_the_whole_frame() {
        let frame = numbered(9, 7);
        let mut image = RgbaImage::default();
        frame.shrink_into(CropArea::WHOLE, 3, &mut image);
        assert_eq!(image, frame.shrink_to_rgba(3, 3));
    }

    #[test]
    fn shrink_into_reuses_the_image_for_a_new_size() {
        let mut image = RgbaImage::default();
        numbered(8, 8).shrink_into(CropArea::WHOLE, 2, &mut image);
        assert_eq!((image.width, image.height), (4, 4));
        numbered(4, 2).shrink_into(CropArea::WHOLE, 2, &mut image);
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixels.len(), 2 * 4);
    }

    #[test]
    fn shrink_into_copes_with_tiny_frames_and_areas() {
        let mut image = RgbaImage::default();
        bgra(1, 1, &[[1, 2, 3, 255]]).shrink_into(CropArea::WHOLE, 2, &mut image);
        assert_eq!(image.pixels, [3, 2, 1, 255]);

        let one_pixel = CropArea::new(0.5, 0.5, 0.5001, 0.5001).unwrap();
        numbered(100, 100).shrink_into(one_pixel, 2, &mut image);
        assert_eq!((image.width, image.height), (1, 1));

        BgraFrame::new(0, 0, Vec::new())
            .unwrap()
            .shrink_into(CropArea::WHOLE, 2, &mut image);
        assert!(image.pixels.is_empty());

        // A factor of 0 is treated as 1 instead of dividing by zero.
        numbered(2, 2).shrink_into(CropArea::WHOLE, 0, &mut image);
        assert_eq!((image.width, image.height), (2, 2));
    }
}
