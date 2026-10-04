//! Failures from `.mod` I/O and from constructing song data.
//!
//! Library paths return these instead of panicking. A short or corrupt module
//! is a [`Error::Truncated`] or [`Error::UnrecognizedTag`], not a crash.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Why a read or write failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoKind {
    /// Reading a `.mod` from disk.
    Read,
    /// Writing a `.mod` to disk.
    Write,
}

/// A recoverable omatrack error.
#[derive(Debug)]
pub enum Error {
    /// The file ended before `context` was complete.
    Truncated {
        /// What was being read.
        context: &'static str,
        /// File offset the reader needed to reach, in bytes.
        expected: usize,
        /// Actual file length, in bytes.
        actual: usize,
    },
    /// The 4-byte tag at offset 1080 is not a supported 4-channel format.
    UnrecognizedTag([u8; 4]),
    /// Song length is outside `1..=128`.
    InvalidSongLength(u8),
    /// Stored patterns do not match `max(order) + 1`.
    PatternCount {
        /// Patterns implied by the order list.
        expected: usize,
        /// Patterns currently stored.
        actual: usize,
    },
    /// Sample PCM length is odd. The format counts 16-bit words.
    OddSampleLength {
        /// 1-based instrument number, when known.
        sample: Option<usize>,
        /// Byte length that was rejected.
        bytes: usize,
    },
    /// Sample PCM is larger than a `u16` word count can express.
    SampleTooLarge {
        /// 1-based instrument number, when known.
        sample: Option<usize>,
        /// Byte length that was rejected.
        bytes: usize,
    },
    /// A cell field does not fit in the bits the format stores.
    InvalidCell {
        /// `period` or `effect`.
        field: &'static str,
        /// The rejected value.
        value: u16,
    },
    /// A title or sample name is longer than its fixed field.
    NameTooLong {
        /// `title` or `sample name`.
        kind: &'static str,
        /// Capacity of the field, in bytes.
        max: usize,
        /// Length of the string, in Unicode scalars.
        actual: usize,
    },
    /// A title or sample name contains a scalar above U+00FF.
    NameNotLatin1 {
        /// `title` or `sample name`.
        kind: &'static str,
    },
    /// Filesystem failure, with the path when we have one.
    Io {
        /// Whether this was a read or a write.
        kind: IoKind,
        /// Path that failed.
        path: PathBuf,
        /// Underlying OS error.
        source: io::Error,
    },
    /// The terminal could not be switched into or out of the TUI.
    Terminal(io::Error),
    /// Playback could not start, or a render could not be stored.
    Audio(String),
    /// A WAV file is not a RIFF/WAVE, is truncated, or uses an encoding we do not convert.
    Wav(String),
    /// A sample edit was rejected (range, volume, finetune, loop, or slot).
    SampleEdit(String),
}

impl Error {
    pub(crate) fn io(kind: IoKind, path: PathBuf, source: io::Error) -> Self {
        Self::Io { kind, path, source }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated {
                context,
                expected,
                actual,
            } => write!(
                f,
                "{context} is truncated: need at least {expected} bytes, have {actual}"
            ),
            Self::UnrecognizedTag(tag) => write!(
                f,
                "unrecognized module tag {}; want M.K., M!K!, FLT4, or 4CHN (31-sample, 4-channel)",
                format_tag(tag)
            ),
            Self::InvalidSongLength(n) => {
                write!(f, "song length {n} is outside 1..=128")
            }
            Self::PatternCount { expected, actual } => write!(
                f,
                "pattern count is {actual}, but the order list requires {expected} (highest order entry plus one)"
            ),
            Self::OddSampleLength { sample, bytes } => match sample {
                Some(n) => write!(
                    f,
                    "sample {n} data length {bytes} is odd; ProTracker stores length in 16-bit words"
                ),
                None => write!(
                    f,
                    "sample data length {bytes} is odd; ProTracker stores length in 16-bit words"
                ),
            },
            Self::SampleTooLarge { sample, bytes } => match sample {
                Some(n) => write!(
                    f,
                    "sample {n} is {bytes} bytes; the format stores length as a u16 word count (max 131070 bytes)"
                ),
                None => write!(
                    f,
                    "sample is {bytes} bytes; the format stores length as a u16 word count (max 131070 bytes)"
                ),
            },
            Self::InvalidCell { field, value } => match *field {
                "period" => write!(f, "cell period {value} does not fit in 12 bits (max 4095)"),
                "effect" => write!(f, "cell effect {value} does not fit in 4 bits (max 15)"),
                _ => write!(f, "invalid cell {field} {value}"),
            },
            Self::NameTooLong { kind, max, actual } => {
                write!(f, "{kind} is {actual} characters; the field holds {max} bytes")
            }
            Self::NameNotLatin1 { kind } => {
                write!(f, "{kind} contains a character outside Latin-1")
            }
            Self::Io { kind, path, source } => {
                let verb = match kind {
                    IoKind::Read => "read",
                    IoKind::Write => "write",
                };
                write!(f, "failed to {verb} {}: {source}", path.display())
            }
            Self::Terminal(source) => write!(f, "terminal error: {source}"),
            Self::Audio(message) | Self::Wav(message) | Self::SampleEdit(message) => {
                write!(f, "{message}")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::Terminal(source) => Some(source),
            _ => None,
        }
    }
}

fn format_tag(tag: &[u8; 4]) -> String {
    if tag.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        let text: String = tag.iter().map(|b| char::from(*b)).collect();
        format!("'{text}'")
    } else {
        format!(
            "{:02X} {:02X} {:02X} {:02X}",
            tag[0], tag[1], tag[2], tag[3]
        )
    }
}
