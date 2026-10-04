//! Omatrack core: a ProTracker module, a four-channel replayer, and the TUI.
//!
//! [`Module`] is the song. [`Module::from_bytes`] / [`Module::to_bytes`] speak
//! the 31-sample 4-channel format. [`player::Playback`] borrows a module and
//! renders stereo PCM without an audio device. [`audio`] sends that PCM to
//! ALSA (and, through it, PipeWire or PulseAudio). [`tui`] draws the document
//! and follows the playhead.
//!
//! [`edit::Editor`] mutates a [`Module`] and keeps the undo stack beside the
//! document, not inside the file format. [`tui::Theme`] paints from a built-in
//! palette or from the active Omarchy theme. [`config`] reads
//! `~/.config/omatrack/config.toml`. [`state`] remembers the last module path.
//! [`viz`] turns the mix into spectrum bars, channel meters, and a scope.

#![forbid(unsafe_code)]

pub mod audio;
pub mod config;
pub mod convert;
pub mod demo;
pub mod edit;
pub mod error;
pub mod modfile;
pub mod module;
pub mod notes;
pub mod omarchy;
pub mod player;
pub mod sample_edit;
pub mod state;
pub mod tui;
pub mod viz;
pub mod wav;
pub mod waveform;

pub use convert::{c2_rate, import_pcm, ImportOptions, ImportedSample};
pub use edit::Editor;
pub use error::Error;
pub use modfile::{HEADER_LEN, PATTERN_BYTES};
pub use module::{Cell, Module, Pattern, Sample, Tag, CHANNELS, ORDER_LEN, ROWS, SAMPLE_COUNT};
pub use notes::{effect_description, format_period};
pub use player::{Playback, PlayerConfig, RenderStats};
pub use wav::{decode_wav, encode_mono8_wav, wav_bytes, write_mono8_wav, write_wav, DecodedWav};
