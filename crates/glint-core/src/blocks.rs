use crate::RgbaImage;

/// Changes are counted in square blocks of this many analysis-image pixels on a side.
const BLOCK_SIZE: usize = 16;
/// A pixel counts as changed when one of its channels moved by more than this many levels
/// out of 255. Video compression makes still content shimmer by a few levels.
const PIXEL_NOISE: u8 = 12;
/// A block counts as changed when at least this many of its pixels did.
const CHANGED_PIXELS: u32 = 8;

/// How many blocks differ between two analysis images of the same size.
pub(crate) fn changed_blocks(before: &RgbaImage, after: &RgbaImage) -> usize {
    debug_assert_eq!((before.width, before.height), (after.width, after.height));
    let row_len = after.width as usize * 4;
    if row_len == 0 || before.pixels.len() != after.pixels.len() {
        return 0;
    }

    let band_len = row_len * BLOCK_SIZE;
    let mut counts = vec![0u32; (after.width as usize).div_ceil(BLOCK_SIZE)];
    let mut changed = 0;
    for (band_before, band_after) in before
        .pixels
        .chunks(band_len)
        .zip(after.pixels.chunks(band_len))
    {
        counts.fill(0);
        for (row_before, row_after) in band_before.chunks(row_len).zip(band_after.chunks(row_len)) {
            let blocks = row_before
                .chunks(BLOCK_SIZE * 4)
                .zip(row_after.chunks(BLOCK_SIZE * 4));
            for (count, (block_before, block_after)) in counts.iter_mut().zip(blocks) {
                *count += changed_pixels(block_before, block_after);
            }
        }
        changed += counts
            .iter()
            .filter(|&&count| count >= CHANGED_PIXELS)
            .count();
    }
    changed
}

fn changed_pixels(before: &[u8], after: &[u8]) -> u32 {
    let before = before.as_chunks::<4>().0;
    let after = after.as_chunks::<4>().0;
    before
        .iter()
        .zip(after)
        .filter(|(a, b)| channel_change(a, b) > PIXEL_NOISE)
        .count() as u32
}

/// The biggest difference between the color channels of two pixels. Mixing the channels
/// into one brightness value first would make some colors look alike: in (R + 2G + B) / 4
/// pure red and pure blue are both 64.
fn channel_change(a: &[u8; 4], b: &[u8; 4]) -> u8 {
    a[0].abs_diff(b[0])
        .max(a[1].abs_diff(b[1]))
        .max(a[2].abs_diff(b[2]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CropArea;
    use crate::synth::{Canvas, Rng};

    fn image(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage {
            width,
            height,
            pixels: color.repeat(width as usize * height as usize),
        }
    }

    fn set(image: &mut RgbaImage, x: usize, y: usize, color: [u8; 4]) {
        let at = (y * image.width as usize + x) * 4;
        image.pixels[at..at + 4].copy_from_slice(&color);
    }

    fn shrunk(canvas: &Canvas) -> RgbaImage {
        let mut image = RgbaImage::default();
        canvas.frame().shrink_into(CropArea::WHOLE, 2, &mut image);
        image
    }

    fn biggest_change(before: &RgbaImage, after: &RgbaImage) -> u8 {
        let before = before.pixels.as_chunks::<4>().0;
        let after = after.pixels.as_chunks::<4>().0;
        before
            .iter()
            .zip(after)
            .map(|(a, b)| channel_change(a, b))
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn same_images_have_no_changed_blocks() {
        let gray = image(40, 40, [90, 90, 90, 255]);
        assert_eq!(changed_blocks(&gray, &gray.clone()), 0);
    }

    #[test]
    fn a_block_needs_enough_changed_pixels() {
        let before = image(32, 16, [100, 100, 100, 255]);
        let mut after = before.clone();
        for x in 0..7 {
            set(&mut after, x, 0, [100, 113, 100, 255]);
        }
        assert_eq!(changed_blocks(&before, &after), 0);
        set(&mut after, 7, 0, [100, 113, 100, 255]);
        assert_eq!(changed_blocks(&before, &after), 1);
    }

    #[test]
    fn noise_up_to_the_threshold_is_ignored() {
        let before = image(16, 16, [100, 100, 100, 255]);
        let after = image(16, 16, [112, 88, 112, 255]);
        assert_eq!(changed_blocks(&before, &after), 0);
    }

    #[test]
    fn partial_blocks_at_the_edges_count() {
        // 20x20 makes a 2x2 grid whose right column and bottom row are 4 pixels thin.
        let before = image(20, 20, [0, 0, 0, 255]);
        let mut after = before.clone();
        for y in 16..20 {
            for x in 16..20 {
                set(&mut after, x, y, [255, 255, 255, 255]);
            }
        }
        assert_eq!(changed_blocks(&before, &after), 1);
    }

    #[test]
    fn colored_text_on_colored_backgrounds_shows_up() {
        // Pure red and pure blue weigh the same in a common brightness formula, and blue
        // weighs little in all of them, so these would be easy to miss in grayscale.
        for (background, ink) in [([0, 0, 255], [255, 0, 0]), ([0, 0, 0], [0, 0, 255])] {
            let plain = Canvas::new(320, 180, background);
            let mut text = plain.clone();
            text.text_line(20, 40, 200, 14, ink, &mut Rng::new(5));

            let (before, after) = (shrunk(&plain), shrunk(&text));
            assert!(biggest_change(&before, &after) >= 200);
            assert!(changed_blocks(&before, &after) > 0);
        }
    }

    #[test]
    fn empty_images_have_no_blocks() {
        let empty = RgbaImage::default();
        assert_eq!(changed_blocks(&empty, &empty), 0);
    }
}
