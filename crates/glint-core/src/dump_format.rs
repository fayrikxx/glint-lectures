use std::str::FromStr;

use crate::{BgraFrame, CropArea};

/// One line of `frames.csv`: a frame the analysis got, in the order it got them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameRow {
    /// Counts up from 1 and names the image, see [`image_name`](Self::image_name).
    pub frame: u64,
    /// Milliseconds since the capture started.
    pub ms: u64,
    /// The size of the captured window.
    pub width: u32,
    pub height: u32,
    /// The part of the window the analysis read. The image holds just this part unless
    /// the whole window was recorded.
    pub area: CropArea,
    /// `false` when the writer was still busy and the frame was dropped.
    pub saved: bool,
}

impl FrameRow {
    pub const HEADER: &str = "frame,ms,width,height,left,top,right,bottom,saved";

    pub fn to_line(&self) -> String {
        format!(
            "{},{},{},{},{},{},{},{},{}",
            self.frame,
            self.ms,
            self.width,
            self.height,
            self.area.left(),
            self.area.top(),
            self.area.right(),
            self.area.bottom(),
            u8::from(self.saved),
        )
    }

    pub fn image_name(&self) -> String {
        format!("{:06}.png", self.frame)
    }

    fn parse(line: &str) -> Result<Self, String> {
        let fields: Vec<&str> = line.split(',').collect();
        let [frame, ms, width, height, left, top, right, bottom, saved] = fields[..] else {
            return Err(format!("expected 9 fields, found {}", fields.len()));
        };
        let area = CropArea::new(number(left)?, number(top)?, number(right)?, number(bottom)?)
            .ok_or("the area isn't inside the window")?;
        Ok(Self {
            frame: number(frame)?,
            ms: number(ms)?,
            width: number(width)?,
            height: number(height)?,
            area,
            saved: flag(saved)?,
        })
    }
}

/// One line of `events.csv`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventRow {
    pub ms: u64,
    pub event: Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The analysis paused: the window got minimized, or covered and stopped drawing.
    Pause,
    Resume,
}

impl EventRow {
    pub const HEADER: &str = "ms,event";

    pub fn to_line(&self) -> String {
        let event = match self.event {
            Event::Pause => "pause",
            Event::Resume => "resume",
        };
        format!("{},{event}", self.ms)
    }

    fn parse(line: &str) -> Result<Self, String> {
        let Some((ms, event)) = line.split_once(',') else {
            return Err("expected 2 fields".to_string());
        };
        let event = match event.trim() {
            "pause" => Event::Pause,
            "resume" => Event::Resume,
            other => return Err(format!("unknown event {other:?}")),
        };
        Ok(Self {
            ms: number(ms)?,
            event,
        })
    }
}

/// Reads `frames.csv`. Errors say which line is wrong.
pub fn parse_frames(text: &str) -> Result<Vec<FrameRow>, String> {
    parse_lines(text, FrameRow::HEADER, FrameRow::parse)
}

/// Reads `events.csv`. Errors say which line is wrong.
pub fn parse_events(text: &str) -> Result<Vec<EventRow>, String> {
    parse_lines(text, EventRow::HEADER, EventRow::parse)
}

fn parse_lines<T>(
    text: &str,
    header: &str,
    parse: fn(&str) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let mut lines = text.lines().enumerate();
    if lines.next().map(|(_, first)| first.trim()) != Some(header) {
        return Err(format!("line 1: expected the header {header:?}"));
    }
    lines
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| parse(line).map_err(|err| format!("line {}: {err}", index + 1)))
        .collect()
}

fn number<T: FromStr>(field: &str) -> Result<T, String> {
    field
        .trim()
        .parse()
        .map_err(|_| format!("{field:?} isn't a number"))
}

fn flag(field: &str) -> Result<bool, String> {
    match field.trim() {
        "1" => Ok(true),
        "0" => Ok(false),
        other => Err(format!("expected 0 or 1, found {other:?}")),
    }
}

/// The frame's pixels the way PNG stores them: RGB, three bytes per pixel. Captures have
/// no meaningful alpha, so it's dropped.
pub fn to_rgb(frame: &BgraFrame) -> Vec<u8> {
    frame
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[b, g, r, _]| [r, g, b])
        .collect()
}

/// Turns RGB pixels read from a PNG back into a frame. `None` if the size doesn't match.
pub fn from_rgb(width: u32, height: u32, rgb: &[u8]) -> Option<BgraFrame> {
    let (pixels, rest) = rgb.as_chunks::<3>();
    if !rest.is_empty() {
        return None;
    }
    let pixels = pixels
        .iter()
        .flat_map(|&[r, g, b]| [b, g, r, 255])
        .collect();
    BgraFrame::new(width, height, pixels)
}

/// Puts a recorded image of `area` back where it was in a `width` x `height` window, so
/// a recording that only kept the area replays exactly like the live capture did. The
/// rest of the window stays black: the analysis never reads it.
///
/// Returns `None` if the image isn't the size `area` has in that window.
pub fn place_in_window(
    image: &BgraFrame,
    width: u32,
    height: u32,
    area: CropArea,
) -> Option<BgraFrame> {
    let (columns, rows) = area.pixels(width, height);
    if (columns.len(), rows.len()) != (image.width() as usize, image.height() as usize) {
        return None;
    }
    let row_len = width as usize * 4;
    let mut pixels = vec![0; row_len * height as usize];
    if !columns.is_empty() {
        for (y, source) in rows.zip(image.pixels().chunks(columns.len() * 4)) {
            pixels[y * row_len + columns.start * 4..y * row_len + columns.end * 4]
                .copy_from_slice(source);
        }
    }
    BgraFrame::new(width, height, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RgbaImage;
    use crate::analysis::analysis_image;
    use crate::synth::slide;

    fn row(frame: u64, ms: u64, area: CropArea, saved: bool) -> FrameRow {
        FrameRow {
            frame,
            ms,
            width: 1920,
            height: 1080,
            area,
            saved,
        }
    }

    fn file(header: &str, lines: &[String]) -> String {
        let mut text = format!("{header}\n");
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    #[test]
    fn frames_survive_writing_and_reading() {
        // A third doesn't print exactly, so this checks that the area comes back the same.
        let area = CropArea::new(1.0 / 3.0, 0.1, 0.9, 0.95).unwrap();
        let rows = [
            row(1, 0, CropArea::WHOLE, true),
            row(2, 250, area, true),
            row(3, 500, area, false),
        ];
        let lines: Vec<String> = rows.iter().map(FrameRow::to_line).collect();
        assert_eq!(parse_frames(&file(FrameRow::HEADER, &lines)).unwrap(), rows);
    }

    #[test]
    fn a_broken_line_names_its_number() {
        let lines = [
            row(1, 0, CropArea::WHOLE, true).to_line(),
            "2,250,1920,1080,0,0,1,1,maybe".to_string(),
        ];
        let err = parse_frames(&file(FrameRow::HEADER, &lines)).unwrap_err();
        assert!(err.starts_with("line 3:"), "{err}");

        let err = parse_frames(&file(FrameRow::HEADER, &["1,0,5".to_string()])).unwrap_err();
        assert!(err.starts_with("line 2:"), "{err}");
    }

    #[test]
    fn a_file_without_the_header_is_rejected() {
        let err = parse_frames("1,0,1920,1080,0,0,1,1,1\n").unwrap_err();
        assert!(err.starts_with("line 1:"), "{err}");
    }

    #[test]
    fn events_survive_writing_and_reading() {
        let events = [
            EventRow {
                ms: 1200,
                event: Event::Pause,
            },
            EventRow {
                ms: 9000,
                event: Event::Resume,
            },
        ];
        let lines: Vec<String> = events.iter().map(EventRow::to_line).collect();
        assert_eq!(
            parse_events(&file(EventRow::HEADER, &lines)).unwrap(),
            events
        );
    }

    #[test]
    fn rgb_conversion_keeps_the_colors() {
        let frame = BgraFrame::new(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 0]).unwrap();
        let rgb = to_rgb(&frame);
        assert_eq!(rgb, [3, 2, 1, 6, 5, 4]);
        let back = from_rgb(2, 1, &rgb).unwrap();
        assert_eq!(back.pixels(), [1, 2, 3, 255, 4, 5, 6, 255]);
        assert!(from_rgb(2, 1, &rgb[..5]).is_none());
    }

    #[test]
    fn a_recorded_area_analyzes_like_the_live_frame() {
        let live = slide(640, 360, [250, 250, 250], [30, 30, 60], 5, 2).frame();
        let area = CropArea::new(0.1, 0.15, 0.8, 0.9).unwrap();
        let recorded = from_rgb(
            live.crop(area).width(),
            live.crop(area).height(),
            &to_rgb(&live.crop(area)),
        )
        .unwrap();
        let replayed = place_in_window(&recorded, 640, 360, area).unwrap();

        let (mut expected, mut actual) = (RgbaImage::default(), RgbaImage::default());
        analysis_image(&live, area, &mut expected);
        analysis_image(&replayed, area, &mut actual);
        assert_eq!(actual, expected);
    }

    #[test]
    fn placing_needs_an_image_of_the_right_size() {
        let image = BgraFrame::new(10, 10, vec![0; 400]).unwrap();
        assert!(place_in_window(&image, 640, 360, CropArea::WHOLE).is_none());
        let empty = BgraFrame::new(0, 0, Vec::new()).unwrap();
        assert!(place_in_window(&empty, 0, 0, CropArea::WHOLE).is_some());
    }
}
