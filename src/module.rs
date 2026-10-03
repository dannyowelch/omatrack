//! The song document.
//!
//! This is the in-memory module, independent of the terminal and of playback.
//! A future mixer should borrow a [`Module`] and keep voice state in its own
//! type. Editing should mutate this document (with an undo stack beside it).
//!
//! Fixed-width title and sample names keep their raw bytes so a parse/write
//! round trip can be identical. [`Module::display_title`] and
//! [`Sample::display_name`] are for the screen only.

use crate::error::Error;
use crate::notes::finetune_from_byte;

/// Bytes in the song title field.
pub const TITLE_LEN: usize = 20;
/// Bytes in a sample name field.
pub const SAMPLE_NAME_LEN: usize = 22;
/// Instruments in a 31-sample module. Instrument *n* in a cell is `samples[n - 1]`.
pub const SAMPLE_COUNT: usize = 31;
/// Order-list length. ProTracker always stores 128 entries.
pub const ORDER_LEN: usize = 128;
/// Rows in a pattern.
pub const ROWS: usize = 64;
/// Channels in an `M.K.` module.
pub const CHANNELS: usize = 4;
/// Largest legal song length.
pub const MAX_SONG_LENGTH: u8 = 128;
/// Largest sample the word-count field can describe.
pub const MAX_SAMPLE_BYTES: usize = u16::MAX as usize * 2;

/// 4-byte format tag at offset 1080.
///
/// All of these share the 31-sample, 4-channel layout. The tag is preserved
/// on write so a round trip does not rewrite `FLT4` into `M.K.`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tag {
    /// `M.K.` — ProTracker.
    #[default]
    Mk,
    /// `M!K!` — ProTracker with more than 64 patterns.
    Extended,
    /// `FLT4` — Startrekker 4-channel.
    Startrekker,
    /// `4CHN` — generic 4-channel.
    FourChannel,
}

impl Tag {
    /// Parse a tag, or report it if this is not a 4-channel 31-sample module.
    pub fn parse(bytes: [u8; 4]) -> Result<Self, Error> {
        match &bytes {
            b"M.K." => Ok(Self::Mk),
            b"M!K!" => Ok(Self::Extended),
            b"FLT4" => Ok(Self::Startrekker),
            b"4CHN" => Ok(Self::FourChannel),
            _ => Err(Error::UnrecognizedTag(bytes)),
        }
    }

    /// Tag bytes as stored at offset 1080.
    pub fn as_bytes(self) -> [u8; 4] {
        match self {
            Self::Mk => *b"M.K.",
            Self::Extended => *b"M!K!",
            Self::Startrekker => *b"FLT4",
            Self::FourChannel => *b"4CHN",
        }
    }

    /// Tag as text, for the song header.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mk => "M.K.",
            Self::Extended => "M!K!",
            Self::Startrekker => "FLT4",
            Self::FourChannel => "4CHN",
        }
    }
}

/// One channel on one row.
///
/// `sample` is the full byte the format stores (0 means "no instrument",
/// 1..=31 are the instruments). Values above 31 do not fit a real instrument
/// but are preserved so odd files still round-trip. `period` must be <= 4095
/// and `effect` <= 15 to be written; those are the widths of the bit fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// Instrument number, 0 for none.
    pub sample: u8,
    /// Amiga period, 0 for no note. See [`crate::notes::PERIODS`].
    pub period: u16,
    /// Effect command, 0..=15.
    pub effect: u8,
    /// Effect parameter byte.
    pub param: u8,
}

impl Cell {
    /// Empty cell: no note, no instrument, no effect.
    pub const fn empty() -> Self {
        Self {
            sample: 0,
            period: 0,
            effect: 0,
            param: 0,
        }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::empty()
    }
}

/// 64 rows by 4 channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    /// `rows[row][channel]`.
    pub rows: [[Cell; CHANNELS]; ROWS],
}

impl Pattern {
    /// A pattern of empty cells.
    pub fn empty() -> Self {
        Self {
            rows: [[Cell::empty(); CHANNELS]; ROWS],
        }
    }
}

impl Default for Pattern {
    fn default() -> Self {
        Self::empty()
    }
}

/// One instrument: name, playback parameters, and signed 8-bit PCM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    /// Raw 22-byte name. Not necessarily NUL-terminated.
    pub name: [u8; SAMPLE_NAME_LEN],
    /// Finetune byte. The low nibble is signed; high bits are preserved.
    pub finetune_raw: u8,
    /// Volume, normally 0..=64. Values above 64 are preserved.
    pub volume: u8,
    /// Loop start, in 16-bit words.
    pub loop_start: u16,
    /// Loop length, in 16-bit words. `0` or `1` means "no loop".
    pub loop_length: u16,
    /// Signed 8-bit PCM. Empty, or an even number of bytes up to 131070.
    pub data: Vec<u8>,
}

impl Default for Sample {
    fn default() -> Self {
        Self {
            name: [0; SAMPLE_NAME_LEN],
            finetune_raw: 0,
            volume: 0,
            loop_start: 0,
            // ProTracker's "no loop" sentinel. Parsed files keep the stored value.
            loop_length: 1,
            data: Vec::new(),
        }
    }
}

impl Sample {
    /// Signed finetune in `-8..=7`.
    pub fn finetune(&self) -> i8 {
        finetune_from_byte(self.finetune_raw)
    }

    /// Whether playback should loop. A word length of 0 or 1 does not loop.
    pub fn loops(&self) -> bool {
        self.loop_length > 1
    }

    /// PCM length in bytes.
    pub fn byte_length(&self) -> usize {
        self.data.len()
    }

    /// Name for display. Raw bytes are left untouched.
    pub fn display_name(&self) -> String {
        display_text(&self.name)
    }

    /// Set the name from a Latin-1 string, NUL-padding the rest of the field.
    pub fn set_name(&mut self, name: &str) -> Result<(), Error> {
        self.name = encode_text(name, "sample name")?;
        Ok(())
    }

    /// Replace PCM. Length must be even and at most [`MAX_SAMPLE_BYTES`].
    pub fn set_data(&mut self, data: Vec<u8>) -> Result<(), Error> {
        validate_sample_data(None, &data)?;
        self.data = data;
        Ok(())
    }
}

/// A ProTracker song: title, 31 samples, order, patterns, and optional suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// Raw 20-byte title.
    pub title: [u8; TITLE_LEN],
    /// Instruments 1..=31 in order.
    pub samples: [Sample; SAMPLE_COUNT],
    /// How many order entries are played, `1..=128`.
    pub song_length: u8,
    /// Order position to jump to when the song ends. Old files often store 127.
    pub restart: u8,
    /// Pattern number for each of the 128 order slots.
    ///
    /// Slots at and after [`Self::song_length`] are still stored. The number of
    /// patterns in the file is `max(order) + 1` over the whole table, not just
    /// the played prefix. That is the ProTracker rule and it is what makes a
    /// byte-identical round trip possible.
    pub order: [u8; ORDER_LEN],
    /// Format tag. Preserved on write.
    pub tag: Tag,
    /// Patterns. `patterns.len()` must equal [`Self::required_pattern_count`] to save.
    pub patterns: Vec<Pattern>,
    /// Bytes after the last sample. Preserved so a round trip matches the input.
    pub trailing: Vec<u8>,
}

impl Default for Module {
    fn default() -> Self {
        Self::new(Tag::Mk)
    }
}

impl Module {
    /// Empty song: one blank pattern, song length 1, no sample data.
    pub fn new(tag: Tag) -> Self {
        Self {
            title: [0; TITLE_LEN],
            samples: std::array::from_fn(|_| Sample::default()),
            song_length: 1,
            restart: 0,
            order: [0; ORDER_LEN],
            tag,
            patterns: vec![Pattern::empty()],
            trailing: Vec::new(),
        }
    }

    /// Title for display. Raw bytes are left untouched.
    pub fn display_title(&self) -> String {
        display_text(&self.title)
    }

    /// Set the title from a Latin-1 string, NUL-padding the rest of the field.
    pub fn set_title(&mut self, title: &str) -> Result<(), Error> {
        self.title = encode_text(title, "title")?;
        Ok(())
    }

    /// Patterns implied by the order list: highest entry, plus one.
    pub fn required_pattern_count(&self) -> usize {
        usize::from(self.order.iter().copied().max().unwrap_or(0)) + 1
    }

    /// Grow or shrink [`Self::patterns`] to [`Self::required_pattern_count`].
    ///
    /// New patterns are empty. Shrinking drops patterns the order list no
    /// longer references.
    pub fn resize_patterns(&mut self) {
        let needed = self.required_pattern_count();
        if self.patterns.len() < needed {
            self.patterns.resize_with(needed, Pattern::empty);
        } else {
            self.patterns.truncate(needed);
        }
    }
}

pub(crate) fn validate_sample_data(sample: Option<usize>, data: &[u8]) -> Result<(), Error> {
    if data.len() % 2 != 0 {
        return Err(Error::OddSampleLength {
            sample,
            bytes: data.len(),
        });
    }
    if data.len() > MAX_SAMPLE_BYTES {
        return Err(Error::SampleTooLarge {
            sample,
            bytes: data.len(),
        });
    }
    Ok(())
}

fn encode_text<const N: usize>(text: &str, kind: &'static str) -> Result<[u8; N], Error> {
    let count = text.chars().count();
    if count > N {
        return Err(Error::NameTooLong {
            kind,
            max: N,
            actual: count,
        });
    }
    let mut out = [0u8; N];
    for (index, ch) in text.chars().enumerate() {
        out[index] = u8::try_from(u32::from(ch)).map_err(|_| Error::NameNotLatin1 { kind })?;
    }
    Ok(out)
}

/// Lossy display for a fixed Amiga string: stop at the first NUL, drop trailing
/// spaces, and replace non-printable bytes. Latin-1 (U+00A0..=U+00FF) is kept.
pub fn display_text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let trimmed = trim_ascii_end(&bytes[..end]);
    trimmed.iter().copied().map(display_char).collect()
}

fn trim_ascii_end(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|&b| b != b' ')
        .map_or(0, |index| index + 1);
    &bytes[..end]
}

fn display_char(byte: u8) -> char {
    match byte {
        b' '..=b'~' | 0xA0..=0xFF => char::from(byte),
        _ => '·',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_through_the_fixed_fields() {
        let mut module = Module::default();
        module.set_title("Omatrack").unwrap();
        assert_eq!(module.display_title(), "Omatrack");
        assert_eq!(module.title[8], 0);

        module.samples[0].set_name("kick").unwrap();
        assert_eq!(module.samples[0].display_name(), "kick");

        let mut full = [0u8; TITLE_LEN];
        full.copy_from_slice(b"ABCDEFGHIJKLMNOPQRST");
        module.title = full;
        assert_eq!(module.display_title(), "ABCDEFGHIJKLMNOPQRST");
    }

    #[test]
    fn display_stops_at_nul_and_hides_controls() {
        let mut name = [0u8; SAMPLE_NAME_LEN];
        name[..6].copy_from_slice(b"ab\x01c  ");
        name[2] = 0;
        assert_eq!(display_text(&name), "ab");

        let mut controls = [0u8; 4];
        controls.copy_from_slice(b"A\x01\x7FB");
        assert_eq!(display_text(&controls), "A··B");
        assert_eq!(display_text(&[0xE9, b' ', b' ']), "é");
        assert_eq!(display_text(b"   "), "");
    }

    #[test]
    fn names_reject_overflow_and_non_latin1() {
        let mut sample = Sample::default();
        let err = sample.set_name("this name is way too long").unwrap_err();
        assert!(err.to_string().contains("sample name"));
        assert_eq!(sample.display_name(), "");

        let mut module = Module::default();
        let err = module.set_title("♪").unwrap_err();
        assert!(err.to_string().contains("Latin-1"));
        module.set_title("é").unwrap();
        assert_eq!(module.title[0], 0xE9);
    }

    #[test]
    fn sample_data_must_be_even_words() {
        let mut sample = Sample::default();
        assert!(sample.set_data(vec![1, 2, 3, 4]).is_ok());
        assert!(sample.set_data(vec![1, 2, 3]).is_err());
        assert_eq!(sample.data.len(), 4);
        assert!(sample.set_data(vec![0; MAX_SAMPLE_BYTES + 2]).is_err());
        assert_eq!(sample.data.len(), 4);
        assert!(!sample.loops());
        sample.loop_length = 8;
        assert!(sample.loops());
    }

    #[test]
    fn order_list_past_song_length_still_reserves_patterns() {
        let mut module = Module::default();
        module.order[0] = 0;
        module.order[127] = 4;
        assert_eq!(module.required_pattern_count(), 5);
        module.resize_patterns();
        assert_eq!(module.patterns.len(), 5);
        module.patterns[4].rows[0][0].period = 856;
        module.order[127] = 0;
        module.resize_patterns();
        assert_eq!(module.patterns.len(), 1);
    }
}
