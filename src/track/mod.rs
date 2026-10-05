//! FastTracker 2 `.xm` and Impulse Tracker `.it` loading, saving, and playback.
//!
//! `.mod` files stay on [`crate::Module`]. [`open_bytes`] picks the parser
//! from the file header, not from the extension.

mod convert;
pub(crate) mod edit;
mod engine;
mod it;
mod pitch;
mod song;
mod xm;

pub use convert::{save_module, save_song, save_song_as, SaveFormat, Saved};
pub use edit::TrackHistory;

use std::path::Path;

use crate::error::{Error, IoKind};
use crate::module::Module;

pub use engine::{render_to_wav, Playback};
pub use song::*;

/// A file the loader understood.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Opened {
    /// 31-sample 4-channel ProTracker module.
    Mod(Module),
    /// FastTracker 2 or Impulse Tracker module.
    Track(Song),
}

/// Read a module, XM, or IT file from memory.
///
/// XM is recognized by the `Extended Module: ` magic. IT is recognized by
/// `IMPM`. Anything else is parsed as a ProTracker module.
pub fn open_bytes(bytes: &[u8]) -> Result<Opened, Error> {
    if xm::is_xm(bytes) {
        Ok(Opened::Track(xm::parse(bytes)?))
    } else if it::is_it(bytes) {
        Ok(Opened::Track(it::parse(bytes)?))
    } else {
        Ok(Opened::Mod(Module::from_bytes(bytes)?))
    }
}

/// Read a module, XM, or IT file from `path`.
pub fn open_path(path: &Path) -> Result<Opened, Error> {
    let bytes = std::fs::read(path)
        .map_err(|source| Error::io(IoKind::Read, path.to_path_buf(), source))?;
    open_bytes(&bytes)
}

/// `true` when the header is XM or IT, regardless of the file name.
pub fn is_tracked(bytes: &[u8]) -> bool {
    xm::is_xm(bytes) || it::is_it(bytes)
}
