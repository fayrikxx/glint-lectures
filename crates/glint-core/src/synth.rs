use crate::BgraFrame;

/// xorshift64*: tests get the same "random" pictures on every run, without a dependency.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        // xorshift never leaves 0, so 0 can't be a state.
        Self(seed | 1)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number in `0..n`.
    pub(crate) fn below(&mut self, n: u32) -> u32 {
        ((self.next_u64() >> 32) % u64::from(n.max(1))) as u32
    }

    /// Roughly normal noise with standard deviation `sigma`: the sum of four uniform numbers.
    pub(crate) fn noise(&mut self, sigma: f64) -> f64 {
        let sum: f64 = (0..4)
            .map(|_| (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64)
            .sum();
        // Four uniforms on [0, 1) add up to a mean of 2 with a variance of 4/12.
        (sum - 2.0) * sigma / (4.0f64 / 12.0).sqrt()
    }
}

/// An RGB picture that tests draw slides on. [`frame`](Self::frame) turns it into what
/// capture would hand over.
#[derive(Debug, Clone)]
pub(crate) struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 3]>,
}

impl Canvas {
    pub(crate) fn new(width: u32, height: u32, background: [u8; 3]) -> Self {
        Self {
            width,
            height,
            pixels: vec![background; width as usize * height as usize],
        }
    }

    /// Fills a rectangle, clipped to the canvas.
    pub(crate) fn fill(&mut self, x: u32, y: u32, width: u32, height: u32, color: [u8; 3]) {
        let columns = x.min(self.width) as usize..(x + width).min(self.width) as usize;
        for row in y..(y + height).min(self.height) {
            let start = row as usize * self.width as usize;
            self.pixels[start + columns.start..start + columns.end].fill(color);
        }
    }

    /// A line of "text": letter-sized blocks with gaps between letters and words, so it
    /// changes the picture about as much as a real line does.
    pub(crate) fn text_line(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        size: u32,
        color: [u8; 3],
        rng: &mut Rng,
    ) {
        let end = x + width;
        let mut cursor = x;
        while cursor < end {
            for _ in 0..2 + rng.below(6) {
                let letter = size / 3 + rng.below(size / 2 + 1);
                self.fill(cursor, y, letter.min(end - cursor), size, color);
                cursor += letter + size / 6 + 1;
                if cursor >= end {
                    return;
                }
            }
            cursor += size / 2;
        }
    }

    /// Random colors in a rectangle, like a frame of video.
    pub(crate) fn texture(&mut self, x: u32, y: u32, width: u32, height: u32, rng: &mut Rng) {
        for row in y..(y + height).min(self.height) {
            for column in x..(x + width).min(self.width) {
                let value = rng.next_u64();
                self.pixels[row as usize * self.width as usize + column as usize] =
                    [value as u8, (value >> 8) as u8, (value >> 16) as u8];
            }
        }
    }

    pub(crate) fn frame(&self) -> BgraFrame {
        let pixels = self
            .pixels
            .iter()
            .flat_map(|&[r, g, b]| [b, g, r, 255])
            .collect();
        BgraFrame::new(self.width, self.height, pixels).expect("the canvas fills its frame")
    }

    /// The frame the way a video stream delivers it: noise on every channel, plus `offset`
    /// on the whole picture, like the small jump a keyframe makes.
    pub(crate) fn noisy_frame(&self, sigma: f64, offset: f64, rng: &mut Rng) -> BgraFrame {
        let mut channel = |value: u8| {
            (f64::from(value) + offset + rng.noise(sigma))
                .round()
                .clamp(0.0, 255.0) as u8
        };
        let pixels = self
            .pixels
            .iter()
            .flat_map(|&[r, g, b]| [channel(b), channel(g), channel(r), 255])
            .collect();
        BgraFrame::new(self.width, self.height, pixels).expect("the canvas fills its frame")
    }
}

/// A slide: a title and `lines` lines of text. The same `seed` draws the same lines, so a
/// slide with one more line is the same slide plus that line.
pub(crate) fn slide(
    width: u32,
    height: u32,
    background: [u8; 3],
    ink: [u8; 3],
    lines: u32,
    seed: u64,
) -> Canvas {
    let mut canvas = Canvas::new(width, height, background);
    let mut rng = Rng::new(seed);
    let margin = width / 12;
    canvas.text_line(
        margin,
        height / 10,
        width / 2,
        (height / 14).max(2),
        ink,
        &mut rng,
    );
    let size = (height / 27).max(1);
    for line in 0..lines {
        let line_width = width / 2 + rng.below(width / 3);
        let top = height / 4 + line * size * 2;
        canvas.text_line(margin, top, line_width, size, ink, &mut rng);
    }
    canvas
}
