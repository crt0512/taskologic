//! Turns a semantic `PrintJob` into ESC/POS bytes for a specific printer,
//! and gets those bytes to the printer.
//!
//! Used by clients only. The daemon never sees a printer byte, printers
//! belong to whichever machine the client runs on.

pub mod bitmap;
pub mod codepage;
pub mod escpos;
pub mod layout;
pub mod profile;
pub mod render;
pub mod sink;
pub mod slip;
pub mod symbology;
pub mod text;

pub use codepage::Codepage;
pub use layout::{Align, Op, TextLine, plan};
pub use slip::{SlipLayout, SlipRow, SlipSection, SlipSet};
pub use profile::{
    BITMAP_SCALE, DeviceProfile, MAX_CUSTOM_MM, MIN_CUSTOM_MM, OutputMode, PaperWidth, Rotation,
    paper_for_media_mm, queue_name_ok,
};
pub use render::{RenderError, render};
pub use sink::{FakeSink, LpSink, PrinterSink, SinkError};
