mod analysis;
mod bgra;
mod blocks;
mod crop;
mod frame;
mod rect;
mod slot;
#[cfg(test)]
mod synth;

pub use analysis::{Analysis, AnalysisState};
pub use bgra::{BgraFrame, RgbaImage};
pub use crop::CropArea;
pub use frame::Frame;
pub use rect::Rect;
pub use slot::{FrameSlot, SlotWait};
