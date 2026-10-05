//! Edits on an XM or IT [`Song`](super::Song), with an undo stack.
//!
//! The ProTracker editor stays in [`crate::edit`]. This stack stores the song
//! from before each change. A song's samples make an entry large; there is
//! still no depth cap, matching the module editor.

use super::song::{Cell, Format, Pattern, Sample, Song, NOTE_CUT, NOTE_FADE, NOTE_OFF};

/// Undo and redo for one XM or IT song.
#[derive(Debug, Default)]
pub struct TrackHistory {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    generation: u64,
    saved: u64,
}

#[derive(Debug)]
struct Entry {
    song: Song,
    generation_before: u64,
    generation_after: u64,
}

impl TrackHistory {
    /// No edits yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Entries that can be undone.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// The song differs from the last successful save.
    pub fn is_dirty(&self) -> bool {
        self.generation != self.saved
    }

    /// The current generation is what is on disk.
    pub fn mark_saved(&mut self) {
        self.saved = self.generation;
    }

    /// Run `change`. An unchanged song does not push an entry.
    pub fn edit(&mut self, song: &mut Song, change: impl FnOnce(&mut Song)) -> bool {
        let before = song.clone();
        let generation_before = self.generation;
        change(song);
        if *song == before {
            return false;
        }
        self.generation = self.generation.saturating_add(1);
        self.undo.push(Entry {
            song: before,
            generation_before,
            generation_after: self.generation,
        });
        self.redo.clear();
        true
    }

    /// Restore the previous song. [`false`] means the stack is empty.
    pub fn undo(&mut self, song: &mut Song) -> bool {
        let Some(mut entry) = self.undo.pop() else {
            return false;
        };
        std::mem::swap(song, &mut entry.song);
        self.generation = entry.generation_before;
        self.redo.push(entry);
        true
    }

    /// Reapply the edit undone most recently.
    pub fn redo(&mut self, song: &mut Song) -> bool {
        let Some(mut entry) = self.redo.pop() else {
            return false;
        };
        std::mem::swap(song, &mut entry.song);
        self.generation = entry.generation_after;
        self.undo.push(entry);
        true
    }
}

/// Write a note. `instrument`, when non-zero, replaces the cell's instrument.
pub fn enter_note(
    song: &mut Song,
    pattern: usize,
    row: usize,
    channel: usize,
    note: u8,
    instrument: u8,
) -> bool {
    let Some(cell) = cell_mut(song, pattern, row, channel) else {
        return false;
    };
    cell.note = note;
    if instrument != 0 {
        cell.instrument = instrument;
    }
    true
}

/// Decimal instrument digits, hex volume, or an effect/parameter nibble.
pub fn enter_digit(
    song: &mut Song,
    pattern: usize,
    row: usize,
    channel: usize,
    field: TrackField,
    digit: u8,
) -> bool {
    let it = song.format == Format::It;
    let Some(cell) = cell_mut(song, pattern, row, channel) else {
        return false;
    };
    match field {
        TrackField::Note => return false,
        TrackField::InstrumentHigh => {
            cell.instrument = digit.saturating_mul(10) + cell.instrument % 10;
        }
        TrackField::InstrumentLow => {
            let next = (cell.instrument / 10) * 10 + digit;
            if digit > 9 {
                return false;
            }
            cell.instrument = next;
        }
        TrackField::VolumeHigh | TrackField::VolumeLow => {
            if digit > 0x0F {
                return false;
            }
            cell.volume = if field == TrackField::VolumeHigh {
                (cell.volume & 0x0F) | (digit << 4)
            } else {
                (cell.volume & 0xF0) | digit
            };
            // XM volume 0 means the column is empty. IT can store volume 0.
            cell.has_volume = it || cell.volume != 0;
        }
        TrackField::Effect => {
            cell.effect = digit;
        }
        TrackField::ParamHigh => {
            if digit > 0x0F {
                return false;
            }
            cell.param = (cell.param & 0x0F) | (digit << 4);
        }
        TrackField::ParamLow => {
            if digit > 0x0F {
                return false;
            }
            cell.param = (cell.param & 0xF0) | digit;
        }
    }
    true
}

/// Clear the whole cell, or one column.
pub fn clear_field(
    song: &mut Song,
    pattern: usize,
    row: usize,
    channel: usize,
    field: TrackField,
) -> bool {
    let Some(cell) = cell_mut(song, pattern, row, channel) else {
        return false;
    };
    match field {
        TrackField::Note => *cell = Cell::empty(),
        TrackField::InstrumentHigh => cell.instrument %= 10,
        TrackField::InstrumentLow => cell.instrument = (cell.instrument / 10) * 10,
        TrackField::VolumeHigh | TrackField::VolumeLow => {
            cell.volume = 0;
            cell.has_volume = false;
        }
        TrackField::Effect => cell.effect = 0,
        TrackField::ParamHigh => cell.param &= 0x0F,
        TrackField::ParamLow => cell.param &= 0xF0,
    }
    true
}

/// Which column a digit edits. The pattern cursor uses the module field enum;
/// this is the track subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackField {
    /// The note. Clearing it empties the cell.
    Note,
    /// Tens digit of the instrument.
    InstrumentHigh,
    /// Ones digit of the instrument.
    InstrumentLow,
    /// High nibble of the volume column.
    VolumeHigh,
    /// Low nibble of the volume column.
    VolumeLow,
    /// Effect command.
    Effect,
    /// High nibble of the effect parameter.
    ParamHigh,
    /// Low nibble of the effect parameter.
    ParamLow,
}

/// Empty the cell one step above `row` (the same row when `step` is 0).
pub fn clear_above(
    song: &mut Song,
    pattern: usize,
    row: usize,
    channel: usize,
    step: u8,
) -> Option<usize> {
    let target = row.saturating_sub(usize::from(step));
    cell_mut(song, pattern, target, channel)?;
    if let Some(cell) = cell_mut(song, pattern, target, channel) {
        *cell = Cell::empty();
    }
    Some(target)
}

/// Insert or delete one cell in a channel, shifting the rest of the column.
pub fn shift_channel(
    song: &mut Song,
    pattern: usize,
    channel: usize,
    row: usize,
    insert: bool,
) -> bool {
    let Some(rows) = song
        .patterns
        .get(pattern)
        .map(|pattern| pattern.row_count())
    else {
        return false;
    };
    if row >= rows {
        return false;
    }
    let column: Vec<Cell> = (0..rows)
        .map(|index| {
            song.cell(pattern, index, channel)
                .unwrap_or_else(Cell::empty)
        })
        .collect();
    let mut next = column.clone();
    let count = rows - row - 1;
    if insert {
        next[row] = Cell::empty();
        next[row + 1..row + 1 + count].clone_from_slice(&column[row..row + count]);
    } else {
        next[row..row + count].clone_from_slice(&column[row + 1..row + 1 + count]);
        next[rows - 1] = Cell::empty();
    }
    for (index, cell) in next.into_iter().enumerate() {
        if let Some(slot) = cell_mut(song, pattern, index, channel) {
            *slot = cell;
        }
    }
    true
}

/// Insert or delete a row in every channel.
pub fn shift_row(song: &mut Song, pattern: usize, row: usize, insert: bool) -> bool {
    let Some(channels) = song
        .patterns
        .get(pattern)
        .and_then(|pattern| pattern.rows.get(row))
        .map(|row| row.len())
    else {
        return false;
    };
    let mut changed = false;
    for channel in 0..channels {
        changed |= shift_channel(song, pattern, channel, row, insert);
    }
    changed
}

/// Empty one channel.
pub fn clear_channel(song: &mut Song, pattern: usize, channel: usize) -> bool {
    let Some(rows) = song.patterns.get(pattern).map(Pattern::row_count) else {
        return false;
    };
    let mut changed = false;
    for row in 0..rows {
        if let Some(cell) = cell_mut(song, pattern, row, channel) {
            if *cell != Cell::empty() {
                *cell = Cell::empty();
                changed = true;
            }
        }
    }
    changed
}

/// Empty the pattern.
pub fn clear_pattern(song: &mut Song, pattern: usize) -> bool {
    let Some(pattern) = song.patterns.get_mut(pattern) else {
        return false;
    };
    let mut changed = false;
    for row in &mut pattern.rows {
        for cell in row {
            if *cell != Cell::empty() {
                *cell = Cell::empty();
                changed = true;
            }
        }
    }
    changed
}

/// Move ordinary notes by `semitones`. Key-off and empty cells stay put.
pub fn transpose(cell: &mut Cell, semitones: i32) -> bool {
    if cell.note == 0 || matches!(cell.note, NOTE_OFF | NOTE_CUT | NOTE_FADE) {
        return false;
    }
    let current = i32::from(cell.note);
    let next = (current + semitones).clamp(1, 120);
    if next == current {
        return false;
    }
    cell.note = u8::try_from(next).unwrap_or(cell.note);
    true
}

/// Append a blank 64-row pattern. The order list is not changed.
pub fn append_pattern(song: &mut Song) -> bool {
    if song.patterns.len() >= 256 {
        return false;
    }
    let channels = pattern_width(song);
    song.patterns.push(Pattern::empty(64, channels));
    true
}

/// Point the current order slot at a new blank pattern.
pub fn new_pattern(song: &mut Song, pos: usize) -> bool {
    if song.patterns.len() >= 256 {
        return false;
    }
    let index = u8::try_from(song.patterns.len()).unwrap_or(255);
    if !append_pattern(song) {
        return false;
    }
    let pos = play_pos(song, pos);
    ensure_order(song, pos);
    song.orders[pos] = index;
    true
}

/// Add `delta` to the pattern number at `pos`, staying inside the pattern list.
pub fn bump_order(song: &mut Song, pos: usize, delta: i32) -> bool {
    let pos = play_pos(song, pos);
    ensure_order(song, pos);
    let current = i32::from(song.orders[pos]);
    if song.format == Format::It && song.orders[pos] >= 254 {
        return false;
    }
    let max = i32::try_from(song.patterns.len().saturating_sub(1)).unwrap_or(0);
    let next = (current + delta).clamp(0, max);
    if next == current {
        return false;
    }
    song.orders[pos] = u8::try_from(next).unwrap_or(0);
    true
}

/// Insert a copy of the current order entry.
pub fn insert_order(song: &mut Song, pos: usize) -> bool {
    let len = playable(song);
    if len >= 256 {
        return false;
    }
    let pos = pos.min(len);
    let pattern = song
        .orders
        .get(pos.min(len.saturating_sub(1)))
        .copied()
        .unwrap_or(0);
    if song.format == Format::It {
        if let Some(end) = song.orders.iter().position(|order| *order == 255) {
            song.orders.insert(pos.min(end), pattern);
            return true;
        }
    }
    song.orders.insert(pos, pattern);
    true
}

/// Remove the order entry at `pos`. The song keeps one position.
pub fn delete_order(song: &mut Song, pos: usize) -> bool {
    let len = playable(song);
    if len <= 1 {
        return false;
    }
    let pos = pos.min(len - 1);
    song.orders.remove(pos);
    true
}

/// Set how many orders play. IT keeps the end marker and anything after it.
pub fn set_length(song: &mut Song, length: usize) -> bool {
    let length = length.clamp(1, 256);
    if song.format == Format::It {
        if let Some(end) = song.orders.iter().position(|order| *order == 255) {
            if end == length {
                return false;
            }
            if end < length {
                let insert = vec![0u8; length - end];
                song.orders.splice(end..end, insert);
            } else {
                song.orders.drain(length..end);
            }
            return true;
        }
    }
    if song.orders.len() == length {
        return false;
    }
    song.orders.resize(length, 0);
    true
}

fn playable(song: &Song) -> usize {
    song.order_len().max(1).min(song.orders.len().max(1))
}

fn play_pos(song: &Song, pos: usize) -> usize {
    pos.min(playable(song).saturating_sub(1))
}

fn ensure_order(song: &mut Song, pos: usize) {
    if song.orders.len() <= pos {
        song.orders.resize(pos + 1, 0);
    }
}

fn pattern_width(song: &Song) -> usize {
    song.patterns
        .first()
        .and_then(|pattern| pattern.rows.first())
        .map(|row| row.len())
        .filter(|width| *width > 0)
        .unwrap_or_else(|| song.channels.max(1))
}

fn cell_mut(song: &mut Song, pattern: usize, row: usize, channel: usize) -> Option<&mut Cell> {
    song.patterns
        .get_mut(pattern)?
        .rows
        .get_mut(row)?
        .get_mut(channel)
}

/// Sample edits that the XM/IT sample can store.
pub fn set_volume(sample: &mut Sample, volume: u8) -> Result<(), String> {
    if volume > 64 {
        return Err(format!("volume {volume} is outside 0..=64"));
    }
    sample.volume = volume;
    Ok(())
}

/// XM finetune, `-128..=127`. IT tuning is the C-5 speed, not this field.
pub fn set_finetune(sample: &mut Sample, finetune: i8) -> Result<(), String> {
    sample.finetune = finetune;
    Ok(())
}

/// Loop `start..end` in frames. `end` equal to `start` turns the loop off.
pub fn set_loop(sample: &mut Sample, start: u32, end: u32) -> Result<(), String> {
    let frames = sample.pcm.len() as u32;
    if start > frames || end > frames {
        return Err(format!(
            "loop {start}..{end} is outside the sample ({frames} frames)"
        ));
    }
    if end <= start {
        sample.loop_kind = super::song::LoopKind::None;
        sample.loop_start = 0;
        sample.loop_end = 0;
        return Ok(());
    }
    sample.loop_kind = super::song::LoopKind::Forward;
    sample.loop_start = start;
    sample.loop_end = end;
    Ok(())
}

/// Loop the whole sample, or turn the loop off.
pub fn toggle_loop(sample: &mut Sample) -> Result<(), String> {
    if sample.loops() {
        sample.loop_kind = super::song::LoopKind::None;
        sample.loop_start = 0;
        sample.loop_end = 0;
        return Ok(());
    }
    if sample.pcm.len() < 2 {
        return Err("sample is too short to loop".to_string());
    }
    sample.loop_kind = super::song::LoopKind::Forward;
    sample.loop_start = 0;
    sample.loop_end = sample.pcm.len() as u32;
    Ok(())
}

/// Keep frames `start..end`.
pub fn trim(sample: &mut Sample, start: usize, end: usize) -> Result<(), String> {
    if start > end || end > sample.pcm.len() || start == end {
        return Err(format!(
            "trim {start}..{end} is outside the sample ({} frames)",
            sample.pcm.len()
        ));
    }
    if sample.loops() {
        let loop_start = sample.loop_start as usize;
        let loop_end = sample.loop_end as usize;
        if loop_start >= start && loop_end <= end && loop_end > loop_start {
            sample.loop_start = (loop_start - start) as u32;
            sample.loop_end = (loop_end - start) as u32;
        } else {
            sample.loop_kind = super::song::LoopKind::None;
            sample.loop_start = 0;
            sample.loop_end = 0;
        }
    }
    sample.pcm = sample.pcm[start..end].to_vec();
    Ok(())
}

/// Scale PCM so the loudest frame is full scale.
pub fn normalize(sample: &mut Sample) {
    let peak = sample
        .pcm
        .iter()
        .map(|frame| frame.unsigned_abs())
        .max()
        .unwrap_or(0);
    if peak == 0 || peak == i16::MAX as u16 {
        return;
    }
    for frame in &mut sample.pcm {
        *frame = ((*frame as i32) * i32::from(i16::MAX) / i32::from(peak)) as i16;
    }
}

/// Reverse the frames. A loop stays at the mirrored position.
pub fn reverse(sample: &mut Sample) {
    sample.pcm.reverse();
    if sample.loops() {
        let len = sample.pcm.len() as u32;
        let start = len.saturating_sub(sample.loop_end);
        let end = len.saturating_sub(sample.loop_start);
        sample.loop_start = start;
        sample.loop_end = end;
    }
}

/// Fade from silence to the current level.
pub fn fade_in(sample: &mut Sample) {
    fade(sample, true);
}

/// Fade from the current level to silence.
pub fn fade_out(sample: &mut Sample) {
    fade(sample, false);
}

fn fade(sample: &mut Sample, fade_in: bool) {
    let len = sample.pcm.len();
    if len < 2 {
        return;
    }
    for (index, frame) in sample.pcm.iter_mut().enumerate() {
        let along = if fade_in { index } else { len - 1 - index };
        *frame = ((*frame as i32) * along as i32 / (len - 1) as i32) as i16;
    }
}

/// Drop the PCM and the loop. The name stays.
pub fn clear_data(sample: &mut Sample) {
    sample.pcm.clear();
    sample.loop_kind = super::song::LoopKind::None;
    sample.loop_start = 0;
    sample.loop_end = 0;
    sample.sustain_kind = super::song::LoopKind::None;
    sample.sustain_start = 0;
    sample.sustain_end = 0;
}
