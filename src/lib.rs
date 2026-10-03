//! Omatrack core: a ProTracker module document, `.mod` I/O, and the read-only TUI.
//!
//! [`Module`] is the song. [`Module::from_bytes`] / [`Module::to_bytes`] speak
//! the 31-sample 4-channel format. [`tui`] draws that document and remembers
//! the cursor.
//!
//! Later milestones should keep their state out of the file bytes:
//! a mixer borrows a [`Module`], edit commands mutate one, and
//! [`tui::Theme`] supplies colors so an Omarchy palette can replace the
//! ProTracker blue without touching input handling.

#![forbid(unsafe_code)]

pub mod error;
pub mod modfile;
pub mod module;
pub mod notes;
pub mod tui;

pub use error::Error;
pub use modfile::{HEADER_LEN, PATTERN_BYTES};
pub use module::{Cell, Module, Pattern, Sample, Tag, CHANNELS, ORDER_LEN, ROWS, SAMPLE_COUNT};
pub use notes::{effect_description, format_period};
