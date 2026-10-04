//! Format-neutral song used by FastTracker 2 and Impulse Tracker playback.
//!
//! ProTracker `.mod` files stay on [`crate::Module`] and the four-channel
//! replayer. XM and IT load into this type so the original mixer is left
//! alone.

/// Which tracker produced a [`Song`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// FastTracker 2 `.xm`.
    Xm,
    /// Impulse Tracker `.it`.
    It,
}

impl Format {
    /// Short label for the song header.
    pub fn label(self) -> &'static str {
        match self {
            Self::Xm => "XM",
            Self::It => "IT",
        }
    }
}

/// A note cell shared by both formats.
///
/// `note` is `0` when the cell has no note. `1..=120` is C-0 through B-9.
/// [`NOTE_OFF`], [`NOTE_CUT`], and [`NOTE_FADE`] are the specials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
    /// `0` empty, `1..=120` a tone, or a note-off special.
    pub note: u8,
    /// Instrument, `0` when the cell does not name one.
    pub instrument: u8,
    /// Raw volume-column byte. `0` means the column is empty.
    pub volume: u8,
    /// Effect command. XM uses the XM effect number. IT uses `1` = `A` … `26` = `Z`.
    pub effect: u8,
    /// Effect parameter.
    pub param: u8,
    /// The volume column was present. IT can store volume 0, which is silence.
    pub has_volume: bool,
}

/// Key off (XM `97`, IT note off).
pub const NOTE_OFF: u8 = 254;
/// Note cut.
pub const NOTE_CUT: u8 = 253;
/// Note fade (IT).
pub const NOTE_FADE: u8 = 252;

impl Cell {
    /// An empty cell.
    pub const fn empty() -> Self {
        Self {
            note: 0,
            instrument: 0,
            volume: 0,
            effect: 0,
            param: 0,
            has_volume: false,
        }
    }

    /// Tone `semitone` from C-0, if it fits in `0..=119`.
    pub fn tone(semitone: u8) -> Self {
        Self {
            note: semitone.saturating_add(1),
            ..Self::empty()
        }
    }
}

/// One pattern. Rows may be shorter or longer than ProTracker's 64.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    /// `rows[row][channel]`.
    pub rows: Vec<Vec<Cell>>,
}

impl Pattern {
    /// `rows` by `channels` empty cells.
    pub fn empty(rows: usize, channels: usize) -> Self {
        Self {
            rows: vec![vec![Cell::empty(); channels]; rows],
        }
    }

    /// Row count.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }
}

/// How a sample loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopKind {
    /// Play once.
    #[default]
    None,
    /// Jump back to the loop start.
    Forward,
    /// Bounce between the loop ends.
    PingPong,
}

/// One sample, stored as signed 16-bit PCM. 8-bit sources are scaled by 256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    /// Display name.
    pub name: String,
    /// Signed PCM. Empty when the slot has no data.
    pub pcm: Vec<i16>,
    /// `8` or `16`. Informational; [`Self::pcm`] is always 16-bit.
    pub bits: u8,
    /// Sample volume, `0..=64`.
    pub volume: u8,
    /// IT global sample volume, `0..=64`. XM leaves this at 64.
    pub global_volume: u8,
    /// Default pan, `0` left … `255` right. `None` keeps the channel pan.
    pub pan: Option<u8>,
    /// XM finetune, `-128..=127`.
    pub finetune: i8,
    /// Semitone transpose added to the played note.
    pub relative_note: i8,
    /// IT C-5 frequency. `0` means "use 8363".
    pub c5_speed: u32,
    /// Loop start, in samples.
    pub loop_start: u32,
    /// Loop end, exclusive, in samples.
    pub loop_end: u32,
    /// Normal loop.
    pub loop_kind: LoopKind,
    /// Sustain-loop start, in samples.
    pub sustain_start: u32,
    /// Sustain-loop end, exclusive, in samples.
    pub sustain_end: u32,
    /// Sustain loop. Used until note-off, then the normal loop (if any).
    pub sustain_kind: LoopKind,
    /// IT sample vibrato.
    pub vibrato: Vibrato,
}

impl Default for Sample {
    fn default() -> Self {
        Self {
            name: String::new(),
            pcm: Vec::new(),
            bits: 8,
            volume: 64,
            global_volume: 64,
            pan: None,
            finetune: 0,
            relative_note: 0,
            c5_speed: 0,
            loop_start: 0,
            loop_end: 0,
            loop_kind: LoopKind::None,
            sustain_start: 0,
            sustain_end: 0,
            sustain_kind: LoopKind::None,
            vibrato: Vibrato::default(),
        }
    }
}

impl Sample {
    /// Whether playback should loop this sample before note-off.
    pub fn loops(&self) -> bool {
        self.loop_kind != LoopKind::None && self.loop_end > self.loop_start
    }
}

/// One envelope node: tick, then value `0..=64` (`32` is the pitch center).
pub type EnvPoint = (u16, u8);

/// Volume, panning, or pitch envelope.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Envelope {
    /// The envelope is applied.
    pub enabled: bool,
    /// Hold at the sustain node until note-off.
    pub sustain: bool,
    /// Loop between [`Self::loop_start`] and [`Self::loop_end`].
    pub loop_on: bool,
    /// Sustain node index. With [`Self::sustain_end`], this is the sustain loop.
    pub sustain_point: u8,
    /// Sustain loop end. Equal to [`Self::sustain_point`] when the envelope holds one node.
    pub sustain_end: u8,
    /// Loop start node index.
    pub loop_start: u8,
    /// Loop end node index.
    pub loop_end: u8,
    /// Nodes in tick order.
    pub points: Vec<EnvPoint>,
}

/// What happens to the previous note when a new one starts on that channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NewNoteAction {
    /// Cut the previous note.
    #[default]
    Cut,
    /// Keep it playing.
    Continue,
    /// Key-off the previous note.
    Off,
    /// Fade the previous note.
    Fade,
}

/// Auto-vibrato on an instrument (XM) or a sample (IT).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Vibrato {
    /// `0` sine, `1` square, `2` ramp, `3` random.
    pub kind: u8,
    /// How fast the depth opens. `0` is full depth immediately.
    pub sweep: u8,
    /// Depth.
    pub depth: u8,
    /// Speed.
    pub rate: u8,
}

/// Keyboard entry: the note to play and the 1-based sample, or `0` for none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Key {
    /// Semitone from C-0.
    pub note: u8,
    /// 1-based sample index. `0` plays nothing.
    pub sample: u16,
}

/// One instrument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instrument {
    /// Display name.
    pub name: String,
    /// Note-to-sample map. Index 0 is C-0. Missing entries do not play.
    pub keys: Vec<Key>,
    /// Volume envelope.
    pub volume_env: Envelope,
    /// Panning envelope.
    pub pan_env: Envelope,
    /// Pitch envelope. Ignored when the IT "filter envelope" flag was set.
    pub pitch_env: Envelope,
    /// Fadeout speed after note-off. `0` does not fade.
    pub fadeout: u16,
    /// Instrument vibrato.
    pub vibrato: Vibrato,
    /// IT new-note action.
    pub nna: NewNoteAction,
    /// Duplicate-check type. `0` off, `1` note, `2` sample, `3` instrument.
    pub dct: u8,
    /// Duplicate-check action. `0` cut, `1` note off, `2` note fade.
    pub dca: u8,
    /// Instrument global volume, `0..=128`.
    pub global_volume: u8,
    /// Default pan, if the instrument names one.
    pub pan: Option<u8>,
}

impl Default for Instrument {
    fn default() -> Self {
        Self {
            name: String::new(),
            keys: Vec::new(),
            volume_env: Envelope::default(),
            pan_env: Envelope::default(),
            pitch_env: Envelope::default(),
            fadeout: 0,
            vibrato: Vibrato::default(),
            nna: NewNoteAction::Cut,
            dct: 0,
            dca: 0,
            global_volume: 128,
            pan: None,
        }
    }
}

impl Instrument {
    /// Sample and transposed note for `semitone` (0 = C-0).
    pub fn map_note(&self, semitone: u8) -> Option<(u16, u8)> {
        let key = self.keys.get(usize::from(semitone))?;
        if key.sample == 0 {
            None
        } else {
            Some((key.sample, key.note))
        }
    }
}

/// An XM or IT module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Song {
    /// Song title.
    pub title: String,
    /// Tracker name from the file, when it has one.
    pub tracker: String,
    /// Source format.
    pub format: Format,
    /// Pattern channels. XM is at most 32, IT at most 64.
    pub channels: usize,
    /// Order list. IT uses `254` as a skip and `255` as the end marker.
    pub orders: Vec<u8>,
    /// Order position to restart at when the song ends.
    pub restart: u16,
    /// Patterns, indexed by the order list.
    pub patterns: Vec<Pattern>,
    /// Instruments. XM notes name these. IT notes name them in instrument mode.
    pub instruments: Vec<Instrument>,
    /// Samples. Instrument key maps point here, 1-based.
    pub samples: Vec<Sample>,
    /// Linear frequency table (XM flag, IT linear slides).
    pub linear: bool,
    /// Ticks per row at the start.
    pub initial_speed: u8,
    /// Tempo at the start.
    pub initial_tempo: u8,
    /// Global volume. XM is `0..=64`, IT is `0..=128`.
    pub initial_global_volume: u16,
    /// Denominator for [`Self::initial_global_volume`].
    pub global_volume_max: u16,
    /// Initial channel pan, `0..=255`.
    pub initial_pan: Vec<u8>,
    /// Initial channel volume, `0..=64`.
    pub initial_channel_volume: Vec<u8>,
    /// Channels that start muted (IT pan bit 7).
    pub initial_mute: Vec<bool>,
    /// IT instrument mode. Sample mode maps the note's instrument number onto a sample.
    pub instrument_mode: bool,
    /// IT old-effects flag. Changes a few effect details.
    pub old_effects: bool,
    /// IT "compatible Gxx" flag. Gxx shares memory with Exx/Fxx when set.
    pub compatible_gxx: bool,
}

impl Song {
    /// Channels, at least 1.
    pub fn channel_count(&self) -> usize {
        self.channels.max(1)
    }

    /// Playable order length. An IT `255` ends the list.
    pub fn order_len(&self) -> usize {
        if self.format == Format::It {
            self.orders
                .iter()
                .position(|order| *order == 255)
                .unwrap_or(self.orders.len())
                .max(1)
        } else {
            self.orders.len().max(1)
        }
    }

    /// Pattern index at an order position, if that pattern exists.
    pub fn order_pattern(&self, order: usize) -> Option<usize> {
        let raw = usize::from(*self.orders.get(order)?);
        if self.format == Format::It && raw >= 254 {
            return None;
        }
        self.patterns.get(raw).map(|_| raw)
    }

    /// Cell at `pattern` / `row` / `channel`.
    pub fn cell(&self, pattern: usize, row: usize, channel: usize) -> Option<Cell> {
        self.patterns
            .get(pattern)?
            .rows
            .get(row)?
            .get(channel)
            .copied()
    }

    /// Rows in `pattern`, or 0 when the pattern is absent.
    pub fn row_count(&self, pattern: usize) -> usize {
        self.patterns
            .get(pattern)
            .map(Pattern::row_count)
            .unwrap_or(0)
    }
}

/// Note text: `C-4`, `---` , `===` for key off, `^^^` for cut, `~~~` for fade.
pub fn format_note(note: u8) -> String {
    match note {
        0 => "---".to_string(),
        NOTE_OFF => "===".to_string(),
        NOTE_CUT => "^^^".to_string(),
        NOTE_FADE => "~~~".to_string(),
        other => {
            let semi = usize::from(other.saturating_sub(1));
            const NAMES: [&str; 12] = [
                "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-",
            ];
            format!("{}{}", NAMES[semi % 12], semi / 12)
        }
    }
}

/// Volume column as two characters. Empty is `--`.
pub fn format_volume(volume: u8) -> String {
    if volume == 0 {
        "--".to_string()
    } else {
        format!("{volume:02X}")
    }
}

/// Effect column as three characters.
pub fn format_effect(format: Format, effect: u8, param: u8) -> String {
    if effect == 0 && param == 0 {
        return "...".to_string();
    }
    let letter = match format {
        Format::Xm => xm_effect_char(effect),
        Format::It => it_effect_char(effect),
    };
    format!("{letter}{param:02X}")
}

fn xm_effect_char(effect: u8) -> char {
    match effect {
        0..=9 => char::from(b'0' + effect),
        10..=35 => char::from(b'A' + (effect - 10)),
        _ => '?',
    }
}

fn it_effect_char(effect: u8) -> char {
    if (1..=26).contains(&effect) {
        char::from(b'A' + (effect - 1))
    } else if effect == 0 {
        '.'
    } else {
        '?'
    }
}

/// Shown when a save would overwrite an XM or IT file, or pretend to convert it.
pub const SAVE_UNSUPPORTED: &str = "Saving XM and IT is not supported yet. .mod is the only write format; converting this song would drop extra channels, instruments, and envelopes, so the original file was left unchanged.";
