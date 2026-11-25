mod analysis;
mod bgra;
mod blocks;
mod crop;
/// The files a recorded capture is made of: `frames.csv` and `events.csv` next to one PNG
/// per frame. Dev builds write them, `examples/replay.rs` reads them.
pub mod dump_format;
mod frame;
mod rect;
pub mod replay;
mod slot;
#[cfg(test)]
mod synth;

pub use analysis::{Analysis, AnalysisState};
pub use bgra::{BgraFrame, RgbaImage};
pub use crop::CropArea;
pub use frame::Frame;
pub use rect::Rect;
pub use slot::{FrameSlot, SlotWait};
