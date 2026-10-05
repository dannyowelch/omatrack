//! Pattern and song edits, with an undo stack.
//!
//! [`Editor`] is the only thing that changes a [`Module`] after load. Each
//! user action pushes one entry. Undo and redo walk that stack; there is no
//! depth cap. The terminal calls this and keeps the cursor, the selection,
//! and the clipboard for itself.
//!
//! A cell cursor has six fields, in ProTracker order: note, sample tens,
//! sample ones, effect command, effect parameter high, effect parameter low.
//! Sample digits are decimal (`00`–`31`, matching the column). Effect digits
//! are hex.

use crate::error::Error;
use crate::module::{
    Cell, Module, Pattern, Sample, Tag, CHANNELS, ORDER_LEN, ROWS, SAMPLE_COUNT, SAMPLE_NAME_LEN,
    TITLE_LEN,
};
use crate::notes::PERIODS;

/// Octave selected when a song opens. The lower piano row plays this octave.
pub const DEFAULT_OCTAVE: u8 = 2;
/// Rows the cursor moves after a note or a finished effect parameter.
pub const DEFAULT_STEP: u8 = 1;
/// Lowest ProTracker octave the piano can target.
pub const MIN_OCTAVE: u8 = 1;
/// Highest ProTracker octave. The upper piano row is silent on this octave.
pub const MAX_OCTAVE: u8 = 3;
/// Largest edit step. Zero stays on the row.
pub const MAX_STEP: u8 = 16;

/// One column inside a channel cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The note period.
    Note,
    /// Tens digit of the decimal sample number.
    SampleHigh,
    /// Ones digit of the decimal sample number.
    SampleLow,
    /// Effect command nibble.
    Effect,
    /// High nibble of the effect parameter.
    ParamHigh,
    /// Low nibble of the effect parameter.
    ParamLow,
    /// High nibble of an XM/IT volume column. ProTracker cells do not use it.
    VolumeHigh,
    /// Low nibble of an XM/IT volume column. ProTracker cells do not use it.
    VolumeLow,
}

impl Field {
    /// Fields in one channel cell.
    pub const COUNT: usize = 6;

    /// Left-to-right index, `0..6`.
    pub fn index(self) -> usize {
        match self {
            Self::Note => 0,
            Self::SampleHigh => 1,
            Self::SampleLow => 2,
            Self::Effect => 3,
            Self::ParamHigh => 4,
            Self::ParamLow => 5,
            Self::VolumeHigh => 6,
            Self::VolumeLow => 7,
        }
    }

    /// Field at `index`, wrapping into `0..6`.
    pub fn from_index(index: usize) -> Self {
        match index % Self::COUNT {
            0 => Self::Note,
            1 => Self::SampleHigh,
            2 => Self::SampleLow,
            3 => Self::Effect,
            4 => Self::ParamHigh,
            _ => Self::ParamLow,
        }
    }

    /// Short label for the pattern header.
    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::SampleHigh | Self::SampleLow => "Sample",
            Self::Effect => "Effect",
            Self::ParamHigh | Self::ParamLow => "Param",
            Self::VolumeHigh | Self::VolumeLow => "Volume",
        }
    }
}

/// Move between the eight columns of an XM/IT cell.
///
/// Note, instrument, volume, effect, parameter. `channels` is the song width.
pub fn shift_track_field(
    channel: usize,
    channels: usize,
    field: Field,
    delta: isize,
) -> (usize, Field) {
    const FIELDS: [Field; 8] = [
        Field::Note,
        Field::SampleHigh,
        Field::SampleLow,
        Field::VolumeHigh,
        Field::VolumeLow,
        Field::Effect,
        Field::ParamHigh,
        Field::ParamLow,
    ];
    let channels = channels.max(1);
    let width = channels * FIELDS.len();
    let field_index = FIELDS.iter().position(|item| *item == field).unwrap_or(0);
    let current = channel.min(channels - 1) * FIELDS.len() + field_index;
    let last = width - 1;
    let next = (current as isize + delta).clamp(0, last as isize) as usize;
    (next / FIELDS.len(), FIELDS[next % FIELDS.len()])
}

/// Where a pattern edit lands, plus the edit step used to move on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    /// Pattern index.
    pub pattern: usize,
    /// Cell cursor before the edit.
    pub cursor: PatternCursor,
    /// Rows to advance after a note or a completed parameter. `0` stays.
    pub step: u8,
}

/// Row, channel, and field inside one pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatternCursor {
    /// `0..64`.
    pub row: usize,
    /// `0..4`.
    pub channel: usize,
    /// Column inside the channel.
    pub field: Field,
}

/// Inclusive rectangle of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRange {
    /// First row.
    pub row_lo: usize,
    /// Last row.
    pub row_hi: usize,
    /// First channel.
    pub channel_lo: usize,
    /// Last channel.
    pub channel_hi: usize,
}

impl CellRange {
    /// A single cell.
    pub fn single(row: usize, channel: usize) -> Self {
        Self {
            row_lo: row,
            row_hi: row,
            channel_lo: channel,
            channel_hi: channel,
        }
    }

    /// Whether `(row, channel)` sits inside the rectangle.
    pub fn contains(self, row: usize, channel: usize) -> bool {
        (self.row_lo..=self.row_hi).contains(&row)
            && (self.channel_lo..=self.channel_hi).contains(&channel)
    }

    /// Row count.
    pub fn rows(self) -> usize {
        self.row_hi.saturating_sub(self.row_lo).saturating_add(1)
    }

    /// Channel count.
    pub fn channels(self) -> usize {
        self.channel_hi
            .saturating_sub(self.channel_lo)
            .saturating_add(1)
    }
}

/// Anchor plus the moving corner. Both corners are inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Pattern the block belongs to.
    pub pattern: usize,
    /// Row where the block was marked.
    pub anchor_row: usize,
    /// Channel where the block was marked.
    pub anchor_channel: usize,
    /// Current row corner.
    pub row: usize,
    /// Current channel corner.
    pub channel: usize,
}

impl Selection {
    /// Inclusive bounds, independent of which corner is the anchor.
    pub fn range(self) -> CellRange {
        CellRange {
            row_lo: self.anchor_row.min(self.row),
            row_hi: self.anchor_row.max(self.row),
            channel_lo: self.anchor_channel.min(self.channel),
            channel_hi: self.anchor_channel.max(self.channel),
        }
    }
}

/// Cells copied from a block. Rows are outer; each row holds one or more channels.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Clipboard {
    rows: Vec<Vec<Cell>>,
}

impl Clipboard {
    /// No cells.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() || self.rows.iter().all(Vec::is_empty)
    }

    /// Copied rows.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Channels in the first row, or 0.
    pub fn channel_count(&self) -> usize {
        self.rows.first().map(Vec::len).unwrap_or(0)
    }
}

/// Undo and redo for one document.
///
/// A new edit drops the redo stack. Entries remember the generation they
/// belonged to, so undoing back to a save clears the modified flag.
#[derive(Debug)]
pub struct Editor {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    generation: u64,
    saved_generation: u64,
}

#[derive(Debug)]
struct Entry {
    change: Change,
    generation_before: u64,
    generation_after: u64,
}

#[derive(Debug)]
enum Change {
    Cells {
        pattern: usize,
        slots: Vec<CellSlot>,
    },
    Order(Box<OrderChange>),
    Structure(Box<StructureChange>),
    Title {
        before: [u8; TITLE_LEN],
        after: [u8; TITLE_LEN],
    },
    SampleName {
        index: usize,
        before: [u8; SAMPLE_NAME_LEN],
        after: [u8; SAMPLE_NAME_LEN],
    },
    /// A whole instrument: PCM, loop, volume, finetune, and name.
    Sample(Box<SampleChange>),
}

/// Before and after for one sample slot. Boxed because the PCM can be large.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SampleChange {
    index: usize,
    before: Sample,
    after: Sample,
}

#[derive(Debug)]
struct CellSlot {
    row: usize,
    channel: usize,
    before: Cell,
    after: Cell,
}

#[derive(Debug)]
struct OrderChange {
    before_order: [u8; ORDER_LEN],
    after_order: [u8; ORDER_LEN],
    before_len: u8,
    after_len: u8,
}

#[derive(Debug)]
struct StructureChange {
    before: Snap,
    after: Snap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snap {
    order: [u8; ORDER_LEN],
    song_length: u8,
    tag: Tag,
    patterns: Vec<Pattern>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    /// An empty stack. The document is unmodified.
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            generation: 0,
            saved_generation: 0,
        }
    }

    /// Whether [`Self::undo`] would change the document.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether [`Self::redo`] would change the document.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undo entries waiting.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Redo entries waiting.
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// The document differs from the last [`Self::mark_saved`] point.
    pub fn is_dirty(&self) -> bool {
        self.generation != self.saved_generation
    }

    /// Remember the current generation as the last successful save.
    pub fn mark_saved(&mut self) {
        self.saved_generation = self.generation;
    }

    /// Restore the previous edit. The cursor is left where the caller put it.
    pub fn undo(&mut self, module: &mut Module) -> bool {
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        apply_change(module, &entry.change, false);
        self.generation = entry.generation_before;
        self.redo.push(entry);
        true
    }

    /// Reapply the edit undone most recently.
    pub fn redo(&mut self, module: &mut Module) -> bool {
        let Some(entry) = self.redo.pop() else {
            return false;
        };
        apply_change(module, &entry.change, true);
        self.generation = entry.generation_after;
        self.undo.push(entry);
        true
    }

    /// Write `period` and, when `sample` is `1..=31`, that instrument.
    ///
    /// The effect stays. The returned cursor is `step` rows down, on the note
    /// field. [`None`] means the pattern cell does not exist.
    pub fn enter_note(
        &mut self,
        module: &mut Module,
        place: Place,
        period: u16,
        sample: u8,
    ) -> Option<PatternCursor> {
        let mut cell = cell_at(
            module,
            place.pattern,
            place.cursor.row,
            place.cursor.channel,
        )?;
        cell.period = period;
        if (1..=31).contains(&sample) {
            cell.sample = sample;
        }
        self.write_cell(
            module,
            place.pattern,
            place.cursor.row,
            place.cursor.channel,
            cell,
        );
        Some(PatternCursor {
            row: advance_row(place.cursor.row, place.step),
            channel: place.cursor.channel,
            field: Field::Note,
        })
    }

    /// Write one sample or effect digit and move to the next field.
    ///
    /// Sample digits are decimal and stay in `00..=31`. Effect digits are hex.
    /// The parameter's low nibble advances by [`Place::step`] and returns to
    /// the note field. [`None`] means the digit does not belong in this field.
    pub fn enter_digit(
        &mut self,
        module: &mut Module,
        place: Place,
        digit: u8,
    ) -> Option<PatternCursor> {
        let mut cell = cell_at(
            module,
            place.pattern,
            place.cursor.row,
            place.cursor.channel,
        )?;
        if !apply_digit(&mut cell, place.cursor.field, digit) {
            return None;
        }
        self.write_cell(
            module,
            place.pattern,
            place.cursor.row,
            place.cursor.channel,
            cell,
        );
        Some(cursor_after_digit(place.cursor, place.step))
    }

    /// Clear the note field's whole cell, or one digit field.
    pub fn clear_at(&mut self, module: &mut Module, place: Place) -> bool {
        let mut cell = match cell_at(
            module,
            place.pattern,
            place.cursor.row,
            place.cursor.channel,
        ) {
            Some(cell) => cell,
            None => return false,
        };
        match place.cursor.field {
            Field::Note => cell = Cell::empty(),
            Field::SampleHigh => cell.sample = cell.sample.min(31) % 10,
            Field::SampleLow => cell.sample = (cell.sample.min(31) / 10) * 10,
            Field::Effect => cell.effect = 0,
            Field::ParamHigh => cell.param &= 0x0F,
            Field::ParamLow => cell.param &= 0xF0,
            Field::VolumeHigh | Field::VolumeLow => return false,
        }
        let before = self.undo_len();
        self.write_cell(
            module,
            place.pattern,
            place.cursor.row,
            place.cursor.channel,
            cell,
        );
        self.undo_len() != before
    }

    /// Clear the cell `step` rows above the cursor (the current row when step is 0).
    pub fn backspace(&mut self, module: &mut Module, place: Place) -> Option<PatternCursor> {
        module.patterns.get(place.pattern)?;
        let row = place.cursor.row.saturating_sub(usize::from(place.step));
        let channel = place.cursor.channel;
        cell_at(module, place.pattern, row, channel)?;
        self.write_cell(module, place.pattern, row, channel, Cell::empty());
        Some(PatternCursor {
            row,
            channel,
            field: place.cursor.field,
        })
    }

    /// Push this channel down from `row`. The last row of the channel is dropped.
    pub fn insert_channel_row(
        &mut self,
        module: &mut Module,
        pattern: usize,
        channel: usize,
        row: usize,
    ) -> bool {
        self.map_column(module, pattern, channel, row, true)
    }

    /// Pull this channel up from `row`. The last row of the channel becomes empty.
    pub fn delete_channel_row(
        &mut self,
        module: &mut Module,
        pattern: usize,
        channel: usize,
        row: usize,
    ) -> bool {
        self.map_column(module, pattern, channel, row, false)
    }

    /// Push every channel down from `row`.
    pub fn insert_pattern_row(&mut self, module: &mut Module, pattern: usize, row: usize) -> bool {
        self.map_pattern_row(module, pattern, row, true)
    }

    /// Pull every channel up from `row`.
    pub fn delete_pattern_row(&mut self, module: &mut Module, pattern: usize, row: usize) -> bool {
        self.map_pattern_row(module, pattern, row, false)
    }

    /// Empty every row of one channel.
    pub fn clear_channel(&mut self, module: &mut Module, pattern: usize, channel: usize) -> bool {
        if module.patterns.get(pattern).is_none() || channel >= CHANNELS {
            return false;
        }
        let updates: Vec<_> = (0..ROWS).map(|row| (row, channel, Cell::empty())).collect();
        let before = self.undo_len();
        self.write_cells(module, pattern, &updates);
        self.undo_len() != before
    }

    /// Empty every cell in the pattern.
    pub fn clear_pattern(&mut self, module: &mut Module, pattern: usize) -> bool {
        if module.patterns.get(pattern).is_none() {
            return false;
        }
        let mut updates = Vec::with_capacity(ROWS * CHANNELS);
        for row in 0..ROWS {
            for channel in 0..CHANNELS {
                updates.push((row, channel, Cell::empty()));
            }
        }
        let before = self.undo_len();
        self.write_cells(module, pattern, &updates);
        self.undo_len() != before
    }

    /// Copy `range` out of `pattern`. The document is unchanged.
    pub fn copy_range(module: &Module, pattern: usize, range: CellRange) -> Clipboard {
        let mut rows = Vec::new();
        for row in range.row_lo..=range.row_hi {
            let mut line = Vec::new();
            for channel in range.channel_lo..=range.channel_hi {
                line.push(cell_at(module, pattern, row, channel).unwrap_or_default());
            }
            rows.push(line);
        }
        Clipboard { rows }
    }

    /// Copy `range`, then empty it. One undo step.
    pub fn cut(&mut self, module: &mut Module, pattern: usize, range: CellRange) -> Clipboard {
        let clipboard = Self::copy_range(module, pattern, range);
        let mut updates = Vec::new();
        for row in range.row_lo..=range.row_hi {
            for channel in range.channel_lo..=range.channel_hi {
                updates.push((row, channel, Cell::empty()));
            }
        }
        self.write_cells(module, pattern, &updates);
        clipboard
    }

    /// Write `clipboard` with its top-left on `(row, channel)`. Cells past the
    /// pattern edge are dropped.
    pub fn paste(
        &mut self,
        module: &mut Module,
        pattern: usize,
        row: usize,
        channel: usize,
        clipboard: &Clipboard,
    ) -> bool {
        let mut updates = Vec::new();
        for (offset, line) in clipboard.rows.iter().enumerate() {
            for (column, cell) in line.iter().enumerate() {
                let dest_row = row.saturating_add(offset);
                let dest_channel = channel.saturating_add(column);
                if dest_row < ROWS && dest_channel < CHANNELS {
                    updates.push((dest_row, dest_channel, *cell));
                }
            }
        }
        let before = self.undo_len();
        self.write_cells(module, pattern, &updates);
        self.undo_len() != before
    }

    /// Move every note in `range` by `semitones`. Periods outside the
    /// ProTracker table, and empty notes, stay put. The ends of the table clamp.
    pub fn transpose(
        &mut self,
        module: &mut Module,
        pattern: usize,
        range: CellRange,
        semitones: i32,
    ) -> bool {
        let mut updates = Vec::new();
        for row in range.row_lo..=range.row_hi {
            for channel in range.channel_lo..=range.channel_hi {
                let Some(mut cell) = cell_at(module, pattern, row, channel) else {
                    continue;
                };
                let period = transpose_period(cell.period, semitones);
                if period != cell.period {
                    cell.period = period;
                    updates.push((row, channel, cell));
                }
            }
        }
        let before = self.undo_len();
        self.write_cells(module, pattern, &updates);
        self.undo_len() != before
    }

    /// Append one blank pattern. The order list is not changed.
    ///
    /// More than 64 patterns rewrites an `M.K.` tag to `M!K!`. [`false`] means
    /// the module already holds 256 patterns. The new pattern is not referenced
    /// until an order slot is pointed at it.
    pub fn append_pattern(&mut self, module: &mut Module) -> bool {
        if module.patterns.len() >= 256 {
            return false;
        }
        self.edit_structure(module, |module| {
            module.patterns.push(Pattern::empty());
        });
        true
    }

    /// Point order position `pos` at a new empty pattern.
    ///
    /// More than 64 patterns rewrites an `M.K.` tag to `M!K!`. [`false`] means
    /// the file already holds 256 patterns.
    pub fn new_pattern(&mut self, module: &mut Module, pos: usize) -> bool {
        if module.patterns.len() >= 256 {
            return false;
        }
        let Some(index) = u8::try_from(module.patterns.len()).ok() else {
            return false;
        };
        let pos = played_pos(module, pos);
        self.edit_structure(module, |module| {
            module.order[pos] = index;
        });
        true
    }

    /// Add `delta` to the pattern number at `pos`, staying inside the pattern list.
    ///
    /// The list includes patterns no order slot references. Stepping off the
    /// highest entry does not delete that pattern, and Up can select it again.
    pub fn bump_order_pattern(&mut self, module: &mut Module, pos: usize, delta: i32) -> bool {
        let pos = played_pos(module, pos);
        let current = i32::from(module.order[pos]);
        let max = i32::try_from(module.patterns.len().saturating_sub(1)).unwrap_or(0);
        let next = (current + delta).clamp(0, max);
        if next == current {
            return false;
        }
        let next = u8::try_from(next).unwrap_or(0);
        let before = self.undo_len();
        self.edit_structure(module, |module| {
            module.order[pos] = next;
        });
        self.undo_len() != before
    }

    /// Insert a copy of the current order entry at `pos` and grow the song.
    ///
    /// The rest of the 128-byte table shifts right. [`false`] means the song
    /// is already 128 entries.
    pub fn insert_order(&mut self, module: &mut Module, pos: usize) -> bool {
        if module.song_length >= 128 {
            return false;
        }
        let len = played_len(module);
        let pos = pos.min(len);
        let pattern = module.order[pos.min(len.saturating_sub(1))];
        self.edit_structure(module, |module| {
            for index in (pos..ORDER_LEN - 1).rev() {
                module.order[index + 1] = module.order[index];
            }
            module.order[pos] = pattern;
            module.song_length = module.song_length.saturating_add(1).min(128);
        });
        true
    }

    /// Remove the order entry at `pos` and shorten the song. Length stays at least 1.
    pub fn delete_order(&mut self, module: &mut Module, pos: usize) -> bool {
        if module.song_length <= 1 {
            return false;
        }
        let pos = played_pos(module, pos);
        self.edit_structure(module, |module| {
            for index in pos..ORDER_LEN - 1 {
                module.order[index] = module.order[index + 1];
            }
            module.order[ORDER_LEN - 1] = 0;
            module.song_length = module.song_length.saturating_sub(1).max(1);
        });
        true
    }

    /// Set the played length, clamped to `1..=128`. Order bytes are kept.
    pub fn set_song_length(&mut self, module: &mut Module, length: u8) -> bool {
        let length = length.clamp(1, 128);
        if length == module.song_length {
            return false;
        }
        let before = self.undo_len();
        self.edit_structure(module, |module| {
            module.song_length = length;
        });
        self.undo_len() != before
    }

    /// Replace the title. An unchanged title does not push an undo entry.
    pub fn set_title(&mut self, module: &mut Module, title: &str) -> Result<(), Error> {
        let before = module.title;
        module.set_title(title)?;
        if module.title != before {
            self.push(Change::Title {
                before,
                after: module.title,
            });
        }
        Ok(())
    }

    /// Replace one sample name. `index` is zero-based.
    pub fn set_sample_name(
        &mut self,
        module: &mut Module,
        index: usize,
        name: &str,
    ) -> Result<(), Error> {
        let Some(sample) = module.samples.get_mut(index) else {
            return Ok(());
        };
        let before = sample.name;
        sample.set_name(name)?;
        if module.samples[index].name != before {
            self.push(Change::SampleName {
                index,
                before,
                after: module.samples[index].name,
            });
        }
        Ok(())
    }

    /// Run `edit` on sample `index` and push one undo entry if it changed.
    ///
    /// `Ok(false)` means the sample was left as it was. A rejected edit returns
    /// the error and does not push.
    pub fn edit_sample(
        &mut self,
        module: &mut Module,
        index: usize,
        edit: impl FnOnce(&mut Sample) -> Result<(), Error>,
    ) -> Result<bool, Error> {
        if index >= SAMPLE_COUNT {
            return Err(Error::SampleEdit(format!(
                "sample slot is outside 1..={SAMPLE_COUNT}"
            )));
        }
        let before = module.samples[index].clone();
        edit(&mut module.samples[index])?;
        let after = module.samples[index].clone();
        if after == before {
            return Ok(false);
        }
        self.push(Change::Sample(Box::new(SampleChange {
            index,
            before,
            after,
        })));
        Ok(true)
    }

    /// Copy sample `from` onto sample `to`. Both indexes are zero-based.
    ///
    /// The destination is one undo entry. Copying a slot onto itself does nothing.
    pub fn copy_sample(
        &mut self,
        module: &mut Module,
        from: usize,
        to: usize,
    ) -> Result<bool, Error> {
        if from >= SAMPLE_COUNT || to >= SAMPLE_COUNT {
            return Err(Error::SampleEdit(format!(
                "sample slot is outside 1..={SAMPLE_COUNT}"
            )));
        }
        if from == to {
            return Ok(false);
        }
        let before = module.samples[to].clone();
        crate::sample_edit::copy_to(module, from, to)?;
        let after = module.samples[to].clone();
        if after == before {
            return Ok(false);
        }
        self.push(Change::Sample(Box::new(SampleChange {
            index: to,
            before,
            after,
        })));
        Ok(true)
    }

    fn map_column(
        &mut self,
        module: &mut Module,
        pattern: usize,
        channel: usize,
        row: usize,
        insert: bool,
    ) -> bool {
        if module.patterns.get(pattern).is_none() || channel >= CHANNELS || row >= ROWS {
            return false;
        }
        let current: Vec<Cell> = (0..ROWS)
            .map(|index| cell_at(module, pattern, index, channel).unwrap_or_default())
            .collect();
        let next = shift_column(&current, row, insert);
        let updates: Vec<_> = next
            .into_iter()
            .enumerate()
            .map(|(index, cell)| (index, channel, cell))
            .collect();
        let before = self.undo_len();
        self.write_cells(module, pattern, &updates);
        self.undo_len() != before
    }

    fn map_pattern_row(
        &mut self,
        module: &mut Module,
        pattern: usize,
        row: usize,
        insert: bool,
    ) -> bool {
        if module.patterns.get(pattern).is_none() || row >= ROWS {
            return false;
        }
        let mut updates = Vec::new();
        for channel in 0..CHANNELS {
            let current: Vec<Cell> = (0..ROWS)
                .map(|index| cell_at(module, pattern, index, channel).unwrap_or_default())
                .collect();
            for (index, cell) in shift_column(&current, row, insert).into_iter().enumerate() {
                updates.push((index, channel, cell));
            }
        }
        let before = self.undo_len();
        self.write_cells(module, pattern, &updates);
        self.undo_len() != before
    }

    fn write_cell(
        &mut self,
        module: &mut Module,
        pattern: usize,
        row: usize,
        channel: usize,
        cell: Cell,
    ) {
        self.write_cells(module, pattern, &[(row, channel, cell)]);
    }

    fn write_cells(
        &mut self,
        module: &mut Module,
        pattern: usize,
        updates: &[(usize, usize, Cell)],
    ) {
        if pattern >= module.patterns.len() {
            return;
        }
        let mut slots: Vec<CellSlot> = Vec::new();
        for &(row, channel, cell) in updates {
            let Some(before) = cell_at(module, pattern, row, channel) else {
                continue;
            };
            if let Some(existing) = slots
                .iter_mut()
                .find(|slot| slot.row == row && slot.channel == channel)
            {
                existing.after = cell;
            } else if before != cell {
                slots.push(CellSlot {
                    row,
                    channel,
                    before,
                    after: cell,
                });
            }
        }
        slots.retain(|slot| slot.before != slot.after);
        if slots.is_empty() {
            return;
        }
        if let Some(pat) = module.patterns.get_mut(pattern) {
            for slot in &slots {
                pat.rows[slot.row][slot.channel] = slot.after;
            }
        }
        self.push(Change::Cells { pattern, slots });
    }

    fn edit_structure(&mut self, module: &mut Module, body: impl FnOnce(&mut Module)) {
        let before_order = module.order;
        let before_len = module.song_length;
        let before_tag = module.tag;
        let before_patterns = module.patterns.clone();
        body(module);
        normalize(module);
        let patterns_changed = module.tag != before_tag || module.patterns != before_patterns;
        let order_changed = module.order != before_order || module.song_length != before_len;
        if !patterns_changed && !order_changed {
            return;
        }
        if patterns_changed {
            self.push(Change::Structure(Box::new(StructureChange {
                before: Snap {
                    order: before_order,
                    song_length: before_len,
                    tag: before_tag,
                    patterns: before_patterns,
                },
                after: Snap {
                    order: module.order,
                    song_length: module.song_length,
                    tag: module.tag,
                    patterns: module.patterns.clone(),
                },
            })));
        } else {
            self.push(Change::Order(Box::new(OrderChange {
                before_order,
                after_order: module.order,
                before_len,
                after_len: module.song_length,
            })));
        }
    }

    fn push(&mut self, change: Change) {
        let generation_before = self.generation;
        self.generation = self.generation.saturating_add(1);
        self.undo.push(Entry {
            change,
            generation_before,
            generation_after: self.generation,
        });
        self.redo.clear();
    }
}

fn apply_change(module: &mut Module, change: &Change, forward: bool) {
    match change {
        Change::Cells { pattern, slots } => {
            let Some(pat) = module.patterns.get_mut(*pattern) else {
                return;
            };
            for slot in slots {
                if slot.row < ROWS && slot.channel < CHANNELS {
                    let cell = if forward { slot.after } else { slot.before };
                    pat.rows[slot.row][slot.channel] = cell;
                }
            }
        }
        Change::Order(change) => {
            if forward {
                module.order = change.after_order;
                module.song_length = change.after_len;
            } else {
                module.order = change.before_order;
                module.song_length = change.before_len;
            }
        }
        Change::Structure(change) => {
            let snap = if forward {
                &change.after
            } else {
                &change.before
            };
            module.order = snap.order;
            module.song_length = snap.song_length;
            module.tag = snap.tag;
            module.patterns = snap.patterns.clone();
        }
        Change::Title { before, after } => {
            module.title = if forward { *after } else { *before };
        }
        Change::SampleName {
            index,
            before,
            after,
        } => {
            if let Some(sample) = module.samples.get_mut(*index) {
                sample.name = if forward { *after } else { *before };
            }
        }
        Change::Sample(change) => {
            if let Some(sample) = module.samples.get_mut(change.index) {
                *sample = if forward {
                    change.after.clone()
                } else {
                    change.before.clone()
                };
            }
        }
    }
}

fn normalize(module: &mut Module) {
    module.song_length = module.song_length.clamp(1, 128);
    // Grow so every order byte has a pattern. Never drop one the order stopped
    // naming: that pattern is still editable and still selectable.
    module.resize_patterns();
    if module.patterns.len() > 64 && module.tag == Tag::Mk {
        module.tag = Tag::Extended;
    }
}

fn played_len(module: &Module) -> usize {
    usize::from(module.song_length).clamp(1, ORDER_LEN)
}

fn played_pos(module: &Module, pos: usize) -> usize {
    pos.min(played_len(module).saturating_sub(1))
}

fn cell_at(module: &Module, pattern: usize, row: usize, channel: usize) -> Option<Cell> {
    module
        .patterns
        .get(pattern)?
        .rows
        .get(row)?
        .get(channel)
        .copied()
}

fn shift_column(current: &[Cell], row: usize, insert: bool) -> Vec<Cell> {
    let mut next = vec![Cell::empty(); ROWS];
    next[..row].copy_from_slice(&current[..row]);
    let count = ROWS - row - 1;
    if insert {
        next[row + 1..row + 1 + count].copy_from_slice(&current[row..row + count]);
    } else {
        next[row..row + count].copy_from_slice(&current[row + 1..row + 1 + count]);
    }
    next
}

fn apply_digit(cell: &mut Cell, field: Field, digit: u8) -> bool {
    match field {
        Field::Note => false,
        Field::SampleHigh => match set_sample_high(cell.sample, digit) {
            Some(sample) => {
                cell.sample = sample;
                true
            }
            None => false,
        },
        Field::SampleLow => match set_sample_low(cell.sample, digit) {
            Some(sample) => {
                cell.sample = sample;
                true
            }
            None => false,
        },
        Field::Effect | Field::ParamHigh | Field::ParamLow => {
            if digit > 0x0F {
                return false;
            }
            match field {
                Field::Effect => cell.effect = digit,
                Field::ParamHigh => cell.param = (cell.param & 0x0F) | (digit << 4),
                Field::ParamLow => cell.param = (cell.param & 0xF0) | digit,
                _ => return false,
            }
            true
        }
        Field::VolumeHigh | Field::VolumeLow => false,
    }
}

fn set_sample_high(sample: u8, digit: u8) -> Option<u8> {
    if digit > 3 {
        return None;
    }
    let ones = sample.min(31) % 10;
    Some(digit * 10 + ones)
}

fn set_sample_low(sample: u8, digit: u8) -> Option<u8> {
    if digit > 9 {
        return None;
    }
    let tens = sample.min(31) / 10;
    if tens == 3 && digit > 1 {
        return None;
    }
    Some(tens * 10 + digit)
}

fn cursor_after_digit(cursor: PatternCursor, step: u8) -> PatternCursor {
    match cursor.field {
        Field::SampleHigh => PatternCursor {
            field: Field::SampleLow,
            ..cursor
        },
        Field::SampleLow => PatternCursor {
            field: Field::Effect,
            ..cursor
        },
        Field::Effect => PatternCursor {
            field: Field::ParamHigh,
            ..cursor
        },
        Field::ParamHigh => PatternCursor {
            field: Field::ParamLow,
            ..cursor
        },
        Field::ParamLow => PatternCursor {
            row: advance_row(cursor.row, step),
            channel: cursor.channel,
            field: Field::Note,
        },
        Field::Note | Field::VolumeHigh | Field::VolumeLow => cursor,
    }
}

/// Move `delta` fields across the row. The ends stick.
pub fn shift_field(channel: usize, field: Field, delta: isize) -> (usize, Field) {
    let width = CHANNELS * Field::COUNT;
    let current = channel.min(CHANNELS - 1) * Field::COUNT + field.index();
    let current = i32::try_from(current).unwrap_or(0);
    let delta = i32::try_from(delta).unwrap_or(i32::MAX);
    let max = i32::try_from(width.saturating_sub(1)).unwrap_or(0);
    let next = current.saturating_add(delta).clamp(0, max);
    let next = usize::try_from(next).unwrap_or(0);
    (next / Field::COUNT, Field::from_index(next))
}

/// Clamp a row advance so it stays inside the pattern.
pub fn advance_row(row: usize, step: u8) -> usize {
    row.saturating_add(usize::from(step)).min(ROWS - 1)
}

/// Keep an octave inside the ProTracker piano.
pub fn clamp_octave(octave: i32) -> u8 {
    let octave = octave.clamp(i32::from(MIN_OCTAVE), i32::from(MAX_OCTAVE));
    u8::try_from(octave).unwrap_or(DEFAULT_OCTAVE)
}

/// Keep an edit step inside `0..=16`.
pub fn clamp_step(step: i32) -> u8 {
    let step = step.clamp(0, i32::from(MAX_STEP));
    u8::try_from(step).unwrap_or(DEFAULT_STEP)
}

/// ProTracker piano key to a semitone offset.
///
/// The lower row is `0..=11` (C through B of the current octave). The upper
/// row is `12..=23` (the next octave). The letter case does not matter.
pub fn semitone_from_key(key: char) -> Option<u8> {
    match key.to_ascii_lowercase() {
        'z' => Some(0),
        's' => Some(1),
        'x' => Some(2),
        'd' => Some(3),
        'c' => Some(4),
        'v' => Some(5),
        'g' => Some(6),
        'b' => Some(7),
        'h' => Some(8),
        'n' => Some(9),
        'j' => Some(10),
        'm' => Some(11),
        'q' => Some(12),
        '2' => Some(13),
        'w' => Some(14),
        '3' => Some(15),
        'e' => Some(16),
        'r' => Some(17),
        '5' => Some(18),
        't' => Some(19),
        '6' => Some(20),
        'y' => Some(21),
        '7' => Some(22),
        'u' => Some(23),
        _ => None,
    }
}

/// Finetune-0 period for `octave` (1–3) plus a semitone from [`semitone_from_key`].
pub fn period_at(octave: u8, semitone: u8) -> Option<u16> {
    let octave = usize::from(octave);
    if octave == 0 {
        return None;
    }
    let index = (octave - 1)
        .saturating_mul(12)
        .saturating_add(usize::from(semitone));
    PERIODS.get(index).copied()
}

/// Move a finetune-0 period by `semitones`. Unknown and empty periods stay.
pub fn transpose_period(period: u16, semitones: i32) -> u16 {
    if period == 0 || semitones == 0 {
        return period;
    }
    let Some(index) = PERIODS.iter().position(|&entry| entry == period) else {
        return period;
    };
    let max = i32::try_from(PERIODS.len().saturating_sub(1)).unwrap_or(0);
    let index = i32::try_from(index).unwrap_or(0);
    let next = (index + semitones).clamp(0, max);
    let next = usize::try_from(next).unwrap_or(0);
    PERIODS[next]
}

/// Latin-1 character a title or sample name can store and the view can show.
pub fn is_name_char(ch: char) -> bool {
    matches!(ch, '\u{20}'..='\u{7E}' | '\u{A0}'..='\u{FF}')
}

/// Bytes of a fixed Amiga field up to the first NUL, for the text prompt.
pub fn editable_text(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    bytes[..end].iter().copied().map(char::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(row: usize, channel: usize, field: Field, step: u8) -> Place {
        Place {
            pattern: 0,
            cursor: PatternCursor {
                row,
                channel,
                field,
            },
            step,
        }
    }

    fn cell(module: &Module, row: usize, channel: usize) -> Cell {
        module.patterns[0].rows[row][channel]
    }

    #[test]
    fn piano_keys_cover_two_octaves_and_stop_at_b3() {
        assert_eq!(semitone_from_key('z'), Some(0));
        assert_eq!(semitone_from_key('S'), Some(1));
        assert_eq!(semitone_from_key('m'), Some(11));
        assert_eq!(semitone_from_key('q'), Some(12));
        assert_eq!(semitone_from_key('2'), Some(13));
        assert_eq!(semitone_from_key('u'), Some(23));
        assert_eq!(semitone_from_key('a'), None);
        assert_eq!(semitone_from_key('1'), None);
        assert_eq!(semitone_from_key('4'), None);

        assert_eq!(period_at(1, 0), Some(856));
        assert_eq!(period_at(2, 0), Some(428));
        assert_eq!(period_at(2, 12), Some(214));
        assert_eq!(period_at(3, 11), Some(113));
        assert_eq!(period_at(3, 12), None);
        assert_eq!(period_at(0, 0), None);
        assert_eq!(clamp_octave(0), 1);
        assert_eq!(clamp_octave(9), 3);
        assert_eq!(clamp_step(-2), 0);
        assert_eq!(clamp_step(40), 16);
    }

    #[test]
    fn field_motion_crosses_channels_and_sticks_at_the_ends() {
        assert_eq!(shift_field(0, Field::Note, -1), (0, Field::Note));
        assert_eq!(shift_field(0, Field::Note, 1), (0, Field::SampleHigh));
        assert_eq!(shift_field(0, Field::ParamLow, 1), (1, Field::Note));
        assert_eq!(shift_field(3, Field::ParamLow, 1), (3, Field::ParamLow));
        assert_eq!(advance_row(60, 8), 63);
        assert_eq!(advance_row(4, 0), 4);
    }

    #[test]
    fn note_entry_writes_the_sample_steps_and_undoes() {
        let mut module = Module::default();
        let mut editor = Editor::new();
        let next = editor
            .enter_note(&mut module, place(0, 1, Field::Note, 1), 856, 4)
            .unwrap();
        assert_eq!(next.row, 1);
        assert_eq!(next.channel, 1);
        assert_eq!(next.field, Field::Note);
        assert_eq!(
            cell(&module, 0, 1),
            Cell {
                sample: 4,
                period: 856,
                effect: 0,
                param: 0,
            }
        );
        assert!(editor.is_dirty());

        module.patterns[0].rows[2][1].effect = 0xC;
        module.patterns[0].rows[2][1].param = 0x10;
        let next = editor
            .enter_note(&mut module, place(2, 1, Field::Note, 4), 428, 4)
            .unwrap();
        assert_eq!(next.row, 6);
        assert_eq!(cell(&module, 2, 1).effect, 0xC);
        assert_eq!(cell(&module, 2, 1).period, 428);

        let next = editor
            .enter_note(&mut module, place(10, 0, Field::Note, 0), 214, 1)
            .unwrap();
        assert_eq!(next.row, 10);

        assert!(editor.undo(&mut module));
        assert_eq!(cell(&module, 10, 0), Cell::empty());
        assert!(editor.undo(&mut module));
        assert_eq!(cell(&module, 2, 1).period, 0);
        assert_eq!(cell(&module, 2, 1).effect, 0xC);
        assert!(editor.undo(&mut module));
        assert_eq!(cell(&module, 0, 1), Cell::empty());
        assert!(!editor.is_dirty());
        assert!(editor.redo(&mut module));
        assert_eq!(cell(&module, 0, 1).period, 856);
        assert!(editor.is_dirty());
    }

    #[test]
    fn digits_are_decimal_for_samples_and_hex_for_effects() {
        let mut module = Module::default();
        let mut editor = Editor::new();
        let mut cursor = PatternCursor {
            row: 0,
            channel: 0,
            field: Field::SampleHigh,
        };
        cursor = editor
            .enter_digit(&mut module, place(0, 0, cursor.field, 1), 3)
            .unwrap();
        assert_eq!(cursor.field, Field::SampleLow);
        assert_eq!(cell(&module, 0, 0).sample, 30);
        assert!(editor
            .enter_digit(&mut module, place(0, 0, Field::SampleLow, 1), 5)
            .is_none());
        assert_eq!(cell(&module, 0, 0).sample, 30);
        cursor = editor
            .enter_digit(&mut module, place(0, 0, Field::SampleLow, 1), 1)
            .unwrap();
        assert_eq!(cursor.field, Field::Effect);
        assert_eq!(cell(&module, 0, 0).sample, 31);
        assert!(editor
            .enter_digit(&mut module, place(0, 0, Field::SampleHigh, 1), 4)
            .is_none());

        cursor = editor
            .enter_digit(&mut module, place(0, 0, Field::Effect, 1), 0x0C)
            .unwrap();
        assert_eq!(cursor.field, Field::ParamHigh);
        cursor = editor
            .enter_digit(&mut module, place(0, 0, cursor.field, 1), 0x04)
            .unwrap();
        assert_eq!(cursor.field, Field::ParamLow);
        cursor = editor
            .enter_digit(&mut module, place(0, 0, cursor.field, 2), 0x00)
            .unwrap();
        assert_eq!(cursor.row, 2);
        assert_eq!(cursor.field, Field::Note);
        assert_eq!(cell(&module, 0, 0).effect, 0x0C);
        assert_eq!(cell(&module, 0, 0).param, 0x40);
        assert!(editor
            .enter_digit(&mut module, place(0, 0, Field::Note, 1), 1)
            .is_none());
    }

    #[test]
    fn clear_and_backspace_restore_through_undo() {
        let mut module = Module::default();
        module.patterns[0].rows[0][0] = Cell {
            sample: 2,
            period: 428,
            effect: 0xA,
            param: 0x18,
        };
        module.patterns[0].rows[4][0].period = 214;
        let mut editor = Editor::new();

        assert!(editor.clear_at(&mut module, place(0, 0, Field::Effect, 1)));
        assert_eq!(cell(&module, 0, 0).effect, 0);
        assert_eq!(cell(&module, 0, 0).period, 428);
        assert_eq!(cell(&module, 0, 0).param, 0x18);
        assert!(editor.clear_at(&mut module, place(0, 0, Field::ParamLow, 1)));
        assert_eq!(cell(&module, 0, 0).param, 0x10);
        assert!(editor.clear_at(&mut module, place(0, 0, Field::Note, 1)));
        assert_eq!(cell(&module, 0, 0), Cell::empty());

        let landed = editor
            .backspace(&mut module, place(4, 0, Field::Note, 4))
            .unwrap();
        assert_eq!(landed.row, 0);
        assert_eq!(cell(&module, 4, 0).period, 214);
        let landed = editor
            .backspace(&mut module, place(4, 0, Field::Note, 0))
            .unwrap();
        assert_eq!(landed.row, 4);
        assert_eq!(cell(&module, 4, 0), Cell::empty());

        assert!(editor.undo(&mut module));
        assert_eq!(cell(&module, 4, 0).period, 214);
        assert!(editor.undo(&mut module));
        assert_eq!(cell(&module, 0, 0).period, 428);
        assert_eq!(cell(&module, 0, 0).param, 0x10);
        assert_eq!(cell(&module, 0, 0).effect, 0);
        assert!(editor.undo(&mut module));
        assert_eq!(cell(&module, 0, 0).param, 0x18);
        assert!(editor.undo(&mut module));
        assert_eq!(cell(&module, 0, 0).effect, 0xA);
        assert_eq!(cell(&module, 0, 0).sample, 2);
        assert!(!editor.can_undo());
    }

    #[test]
    fn insert_and_delete_shift_one_channel_or_the_whole_row() {
        let mut module = Module::default();
        let mut editor = Editor::new();
        editor
            .enter_note(&mut module, place(0, 0, Field::Note, 0), 428, 1)
            .unwrap();
        editor
            .enter_note(&mut module, place(0, 1, Field::Note, 0), 320, 2)
            .unwrap();
        editor
            .enter_note(&mut module, place(62, 0, Field::Note, 0), 856, 1)
            .unwrap();

        assert!(editor.insert_channel_row(&mut module, 0, 0, 0));
        assert_eq!(cell(&module, 0, 0).period, 0);
        assert_eq!(cell(&module, 1, 0).period, 428);
        assert_eq!(cell(&module, 63, 0).period, 856);
        assert_eq!(cell(&module, 0, 1).period, 320);
        assert!(editor.delete_channel_row(&mut module, 0, 0, 0));
        assert_eq!(cell(&module, 0, 0).period, 428);
        assert_eq!(cell(&module, 62, 0).period, 856);
        assert_eq!(cell(&module, 63, 0).period, 0);

        assert!(editor.insert_pattern_row(&mut module, 0, 0));
        assert_eq!(cell(&module, 0, 0).period, 0);
        assert_eq!(cell(&module, 0, 1).period, 0);
        assert_eq!(cell(&module, 1, 0).period, 428);
        assert_eq!(cell(&module, 1, 1).period, 320);
        assert_eq!(cell(&module, 63, 0).period, 856);
        assert!(editor.delete_pattern_row(&mut module, 0, 0));
        assert_eq!(cell(&module, 0, 1).period, 320);
        assert_eq!(cell(&module, 62, 0).period, 856);

        assert!(editor.clear_channel(&mut module, 0, 0));
        assert_eq!(cell(&module, 0, 0).period, 0);
        assert_eq!(cell(&module, 0, 1).period, 320);
        assert!(editor.undo(&mut module));
        assert_eq!(cell(&module, 0, 0).period, 428);

        assert!(editor.clear_pattern(&mut module, 0));
        assert_eq!(cell(&module, 0, 1).period, 0);
        assert!(editor.undo(&mut module));
        assert_eq!(cell(&module, 0, 1).period, 320);
        assert_eq!(editor.undo_len(), 7);
    }

    #[test]
    fn blocks_copy_cut_paste_and_transpose() {
        let mut module = Module::default();
        module.patterns[0].rows[1][1] = Cell {
            sample: 1,
            period: 428,
            effect: 0xC,
            param: 0x20,
        };
        module.patterns[0].rows[1][2].period = 214;
        module.patterns[0].rows[2][1].period = 856;
        module.patterns[0].rows[4][0].period = 999;
        let mut editor = Editor::new();
        let range = CellRange {
            row_lo: 1,
            row_hi: 2,
            channel_lo: 1,
            channel_hi: 2,
        };
        let copied = Editor::copy_range(&module, 0, range);
        assert_eq!(copied.row_count(), 2);
        assert_eq!(copied.channel_count(), 2);
        assert!(editor.paste(&mut module, 0, 10, 0, &copied));
        assert_eq!(cell(&module, 10, 0).period, 428);
        assert_eq!(cell(&module, 10, 0).effect, 0xC);
        assert_eq!(cell(&module, 10, 1).period, 214);
        assert_eq!(cell(&module, 11, 0).period, 856);

        assert!(editor.paste(&mut module, 0, 63, 3, &copied));
        assert_eq!(cell(&module, 63, 3).period, 428);
        assert_eq!(cell(&module, 63, 3).sample, 1);

        let cut = editor.cut(&mut module, 0, range);
        assert_eq!(cut.row_count(), 2);
        assert_eq!(
            cut.rows
                .first()
                .and_then(|row| row.first())
                .map(|c| c.period),
            Some(428)
        );
        assert_eq!(cell(&module, 1, 1), Cell::empty());
        assert_eq!(cell(&module, 1, 2), Cell::empty());
        assert_eq!(editor.undo_len(), 3);

        assert!(editor.transpose(&mut module, 0, CellRange::single(10, 0), 1));
        assert_eq!(cell(&module, 10, 0).period, 404);
        assert_eq!(cell(&module, 10, 0).effect, 0xC);
        assert!(editor.transpose(&mut module, 0, CellRange::single(10, 0), 12));
        assert_eq!(cell(&module, 10, 0).period, 202);
        assert_eq!(transpose_period(113, 1), 113);
        assert_eq!(transpose_period(856, -1), 856);
        assert_eq!(transpose_period(999, 3), 999);
        assert_eq!(transpose_period(0, 5), 0);
        assert!(!editor.transpose(&mut module, 0, CellRange::single(4, 0), 2));

        assert!(editor.clear_channel(&mut module, 0, 0));
        assert_eq!(cell(&module, 10, 0).period, 0);
        assert_eq!(cell(&module, 10, 1).period, 214);
        assert!(editor.clear_pattern(&mut module, 0));
        assert_eq!(cell(&module, 10, 1).period, 0);
        assert!(editor.undo(&mut module));
        assert_eq!(cell(&module, 10, 1).period, 214);
    }

    #[test]
    fn order_pattern_title_and_names_round_trip_through_undo() {
        let mut module = Module::new(Tag::Mk);
        module.song_length = 2;
        module.order[0] = 0;
        module.order[1] = 1;
        module.order[9] = 3;
        module.resize_patterns();
        module.patterns[1].rows[0][0].period = 320;
        let mut editor = Editor::new();

        assert!(editor.insert_order(&mut module, 0));
        assert_eq!(module.song_length, 3);
        assert_eq!(module.order[0], 0);
        assert_eq!(module.order[1], 0);
        assert_eq!(module.order[2], 1);
        assert_eq!(module.order[10], 3);
        assert!(editor.delete_order(&mut module, 0));
        assert_eq!(module.song_length, 2);
        assert_eq!(module.order[0], 0);
        assert_eq!(module.order[1], 1);
        assert_eq!(module.order[9], 3);

        assert!(editor.set_song_length(&mut module, 4));
        assert_eq!(module.song_length, 4);
        assert!(!editor.set_song_length(&mut module, 4));
        assert!(editor.undo(&mut module));
        assert_eq!(module.song_length, 2);

        assert!(editor.bump_order_pattern(&mut module, 0, 1));
        assert_eq!(module.order[0], 1);
        assert!(editor.bump_order_pattern(&mut module, 0, 20));
        assert_eq!(module.order[0], 3);
        assert!(!editor.bump_order_pattern(&mut module, 0, 1));
        assert!(editor.new_pattern(&mut module, 0));
        assert_eq!(module.order[0], 4);
        assert_eq!(module.patterns.len(), 5);
        assert_eq!(module.patterns[1].rows[0][0].period, 320);
        assert_eq!(module.tag, Tag::Mk);

        editor.set_title(&mut module, "Omatrack").unwrap();
        editor.set_sample_name(&mut module, 0, "kick").unwrap();
        assert_eq!(module.display_title(), "Omatrack");
        assert_eq!(module.samples[0].display_name(), "kick");
        let err = editor
            .set_sample_name(&mut module, 0, "this name is way too long")
            .unwrap_err();
        assert!(err.to_string().contains("sample name"));
        assert_eq!(module.samples[0].display_name(), "kick");

        let mut raw = [0u8; TITLE_LEN];
        raw[..6].copy_from_slice(b"Song  ");
        assert_eq!(editable_text(&raw), "Song  ");
        module.title = raw;
        let before = editor.undo_len();
        editor.set_title(&mut module, "Song  ").unwrap();
        assert_eq!(editor.undo_len(), before);

        assert!(editor.undo(&mut module));
        assert_eq!(module.samples[0].display_name(), "");
        assert!(editor.undo(&mut module));
        assert_eq!(module.display_title(), "");
        assert!(editor.undo(&mut module));
        assert_eq!(module.patterns.len(), 4);
        assert_eq!(module.order[0], 3);
        assert_eq!(module.patterns[1].rows[0][0].period, 320);
        assert!(editor.undo(&mut module));
        assert_eq!(module.order[0], 1);
        assert!(editor.undo(&mut module));
        assert_eq!(module.order[0], 0);
        assert_eq!(module.tag, Tag::Mk);
    }

    #[test]
    fn stepping_off_pattern_16_keeps_it_when_nothing_else_references_it() {
        let mut module = Module::new(Tag::Mk);
        module.song_length = 1;
        module.order[0] = 16;
        module.resize_patterns();
        assert_eq!(module.patterns.len(), 17);
        for index in 0..17 {
            module.patterns[index].rows[0][0] = Cell {
                sample: 1,
                period: 200 + u16::try_from(index).unwrap(),
                effect: 0,
                param: u8::try_from(index).unwrap(),
            };
        }
        let pattern_16 = module.patterns[16].clone();
        let mut editor = Editor::new();

        assert!(editor.bump_order_pattern(&mut module, 0, -1));
        assert_eq!(module.order[0], 15);
        assert_eq!(module.patterns.len(), 17);
        assert_eq!(module.patterns[16], pattern_16);
        assert_eq!(module.patterns[15].rows[0][0].period, 215);
        assert!(editor.bump_order_pattern(&mut module, 0, 1));
        assert_eq!(module.order[0], 16);
        assert_eq!(module.patterns[16], pattern_16);

        assert!(editor.undo(&mut module));
        assert_eq!(module.order[0], 15);
        assert_eq!(module.patterns[16], pattern_16);
        assert!(editor.redo(&mut module));
        assert_eq!(module.order[0], 16);
        assert_eq!(module.patterns.len(), 17);
        assert_eq!(module.patterns[16].rows[0][0].param, 16);

        let order = module.order;
        assert!(editor.append_pattern(&mut module));
        assert_eq!(module.patterns.len(), 18);
        assert_eq!(module.order, order);
        assert_eq!(module.patterns[17], Pattern::empty());
        assert_eq!(module.patterns[16], pattern_16);
        assert!(editor.undo(&mut module));
        assert_eq!(module.patterns.len(), 17);
        assert_eq!(module.order, order);
        assert_eq!(module.patterns[16], pattern_16);
    }

    #[test]
    fn sixty_five_patterns_switch_an_mk_tag_to_mik() {
        let mut module = Module::new(Tag::Mk);
        let mut editor = Editor::new();
        for _ in 0..64 {
            assert!(editor.new_pattern(&mut module, 0));
        }
        assert_eq!(module.patterns.len(), 65);
        assert_eq!(module.order[0], 64);
        assert_eq!(module.tag, Tag::Extended);
        module.to_bytes().unwrap();
        for _ in 0..64 {
            assert!(editor.undo(&mut module));
        }
        assert_eq!(module.patterns.len(), 1);
        assert_eq!(module.order[0], 0);
        assert_eq!(module.tag, Tag::Mk);
        assert!(!editor.can_undo());
        assert!(editor.redo(&mut module));
        assert_eq!(module.tag, Tag::Mk);
        assert_eq!(module.patterns.len(), 2);
    }

    #[test]
    fn a_new_edit_drops_redo_and_the_stack_is_not_capped() {
        let mut module = Module::default();
        let mut editor = Editor::new();
        editor
            .enter_note(&mut module, place(0, 0, Field::Note, 0), 856, 1)
            .unwrap();
        editor
            .enter_note(&mut module, place(0, 0, Field::Note, 0), 428, 1)
            .unwrap();
        assert!(editor.undo(&mut module));
        editor
            .enter_note(&mut module, place(0, 0, Field::Note, 0), 214, 1)
            .unwrap();
        assert_eq!(editor.redo_len(), 0);
        assert!(editor.undo(&mut module));
        assert!(editor.undo(&mut module));
        assert_eq!(cell(&module, 0, 0), Cell::empty());
        assert!(editor.redo(&mut module));
        assert!(editor.redo(&mut module));
        assert_eq!(cell(&module, 0, 0).period, 214);
        assert!(!editor.can_redo());

        editor.mark_saved();
        assert!(!editor.is_dirty());
        editor
            .enter_note(&mut module, place(1, 0, Field::Note, 0), 856, 1)
            .unwrap();
        assert!(editor.is_dirty());
        assert!(editor.undo(&mut module));
        assert!(!editor.is_dirty());

        let mut module = Module::default();
        let mut editor = Editor::new();
        for index in 0..1000 {
            let row = index % ROWS;
            let channel = (index / ROWS) % CHANNELS;
            let period = PERIODS[index % PERIODS.len()];
            editor
                .enter_note(&mut module, place(row, channel, Field::Note, 0), period, 1)
                .unwrap();
        }
        assert_eq!(editor.undo_len(), 1000);
        let edited = module.clone();
        while editor.undo(&mut module) {}
        assert_eq!(module.patterns[0], Pattern::empty());
        assert!(!editor.is_dirty());
        while editor.redo(&mut module) {}
        assert_eq!(module, edited);
    }

    #[test]
    fn redo_of_a_structure_change_keeps_the_note_on_top() {
        let mut module = Module::default();
        let mut editor = Editor::new();
        assert!(editor.new_pattern(&mut module, 0));
        let mut on_new = place(0, 0, Field::Note, 0);
        on_new.pattern = 1;
        editor.enter_note(&mut module, on_new, 856, 2).unwrap();
        assert_eq!(module.patterns[1].rows[0][0].sample, 2);
        assert!(editor.undo(&mut module));
        assert_eq!(module.patterns[1].rows[0][0], Cell::empty());
        assert!(editor.undo(&mut module));
        assert_eq!(module.patterns.len(), 1);
        assert!(editor.redo(&mut module));
        assert!(editor.redo(&mut module));
        assert_eq!(module.patterns[1].rows[0][0].period, 856);
        assert_eq!(module.patterns[1].rows[0][0].sample, 2);
    }
}
