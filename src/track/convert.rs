//! Save a song as `.mod`, `.xm`, or `.it`.
//!
//! Saving in the song's own format writes what the loader kept. Saving as a
//! different extension converts, and the warnings name what the target cannot
//! store. The open song is not changed here.

use std::path::Path;

use crate::error::Error;
use crate::module::{Module, Tag, CHANNELS, ORDER_LEN, ROWS, SAMPLE_COUNT};
use crate::notes::PERIODS;

use super::it;
use super::song::{
    Cell as TrackCell, Envelope, Format, Instrument, Key, LoopKind, Pattern as TrackPattern,
    Sample as TrackSample, Song, NOTE_CUT, NOTE_FADE, NOTE_OFF,
};
use super::xm;

/// Which file to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFormat {
    /// 31-sample 4-channel ProTracker.
    Mod,
    /// FastTracker 2.
    Xm,
    /// Impulse Tracker.
    It,
}

impl SaveFormat {
    /// Format named by the extension, ignoring case.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "mod" => Some(Self::Mod),
            "xm" => Some(Self::Xm),
            "it" => Some(Self::It),
            _ => None,
        }
    }

    /// Extension without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::Xm => "xm",
            Self::It => "it",
        }
    }
}

/// Bytes to write, and what the target dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    /// Encoded file.
    pub bytes: Vec<u8>,
    /// Lossy steps, in reading order.
    pub warnings: Vec<String>,
    /// First pattern a `.mod` could not store. `None` for XM and IT.
    pub omitted_from: Option<usize>,
}

/// Write `song` in its own format.
pub fn save_song(song: &Song) -> Result<(Vec<u8>, Vec<String>), Error> {
    let format = match song.format {
        Format::Xm => SaveFormat::Xm,
        Format::It => SaveFormat::It,
    };
    let saved = save_song_as(song, format)?;
    Ok((saved.bytes, saved.warnings))
}

/// Write `song` as `format`.
pub fn save_song_as(song: &Song, format: SaveFormat) -> Result<Saved, Error> {
    match format {
        SaveFormat::Xm if song.format == Format::Xm => encode_track(song, Format::Xm),
        SaveFormat::It if song.format == Format::It => encode_track(song, Format::It),
        SaveFormat::Mod => song_to_mod(song),
        SaveFormat::Xm => {
            let (converted, warnings) = to_format(song, Format::Xm);
            let mut saved = encode_track(&converted, Format::Xm)?;
            prepend(&mut saved.warnings, warnings);
            Ok(saved)
        }
        SaveFormat::It => {
            let (converted, warnings) = to_format(song, Format::It);
            let mut saved = encode_track(&converted, Format::It)?;
            prepend(&mut saved.warnings, warnings);
            Ok(saved)
        }
    }
}

/// Write a ProTracker module as `format`.
pub fn save_module(module: &Module, format: SaveFormat) -> Result<Saved, Error> {
    match format {
        SaveFormat::Mod => {
            let (bytes, omitted_from) = module.to_stored_bytes()?;
            Ok(Saved {
                bytes,
                warnings: Vec::new(),
                omitted_from,
            })
        }
        SaveFormat::Xm | SaveFormat::It => {
            let target = if format == SaveFormat::Xm {
                Format::Xm
            } else {
                Format::It
            };
            let (song, warnings) = module_to_song(module, target);
            let mut saved = encode_track(&song, target)?;
            prepend(&mut saved.warnings, warnings);
            Ok(saved)
        }
    }
}

fn encode_track(song: &Song, format: Format) -> Result<Saved, Error> {
    let (bytes, warnings) = match format {
        Format::Xm => xm::write(song)?,
        Format::It => it::write(song)?,
    };
    Ok(Saved {
        bytes,
        warnings,
        omitted_from: None,
    })
}

fn prepend(warnings: &mut Vec<String>, earlier: Vec<String>) {
    let mut merged = earlier;
    merged.append(warnings);
    *warnings = merged;
}

fn to_format(song: &Song, target: Format) -> (Song, Vec<String>) {
    let mut warnings = Vec::new();
    let mut out = song.clone();
    out.format = target;
    out.load_notes.lines.clear();
    if target == Format::It && out.compat < 0x200 {
        out.compat = 0x0214;
        out.tracker = "IT 0x0214".to_string();
    }
    if target == Format::Xm {
        out.compat = 0;
        if out.tracker.is_empty() {
            out.tracker = "omatrack".to_string();
        }
    }
    let limit = if target == Format::Xm { 32 } else { 64 };
    if out.channels > limit {
        warnings.push(format!(
            "Channels above {limit} were left out. {} stores {limit}.",
            target.label()
        ));
        out.channels = limit;
        out.initial_pan.truncate(limit);
        out.initial_channel_volume.truncate(limit);
        out.initial_mute.truncate(limit);
    }
    let mut loss = Loss::default();
    for pattern in &mut out.patterns {
        let rows = pattern.row_count();
        let max_rows = if target == Format::Xm { 256 } else { 1024 };
        if rows > max_rows {
            pattern.rows.truncate(max_rows);
            loss.rows = true;
        }
        for row in &mut pattern.rows {
            if row.len() > out.channels {
                if row.iter().skip(out.channels).any(|cell| !track_empty(cell)) {
                    loss.channels = true;
                }
                row.truncate(out.channels);
            }
            for cell in row.iter_mut() {
                translate_cell(cell, song.format, target, &mut loss);
            }
        }
    }
    if target == Format::Xm {
        regroup_xm(&mut out, &mut loss);
        for instrument in &mut out.instruments {
            if instrument.pitch_env.filter {
                instrument.pitch_env.filter = false;
                instrument.pitch_env.enabled = false;
                loss.filter = true;
            }
            if instrument.nna != super::song::NewNoteAction::Cut {
                loss.nna = true;
            }
            trim_env(&mut instrument.volume_env, &mut loss);
            trim_env(&mut instrument.pan_env, &mut loss);
            trim_env(&mut instrument.pitch_env, &mut loss);
        }
    }
    if song.format == Format::Xm && target == Format::It {
        for instrument in &mut out.instruments {
            // XM key maps are 96 notes and do not transpose. Pad to IT's 120.
            if instrument.keys.len() < 120 {
                let next = instrument.keys.len();
                instrument.keys.resize(120, Key::default());
                for (index, key) in instrument.keys.iter_mut().enumerate().skip(next) {
                    key.note = index as u8;
                }
            }
        }
    }
    warnings.extend(loss.messages());
    (out, warnings)
}

fn trim_env(envelope: &mut Envelope, loss: &mut Loss) {
    if envelope.points.len() > 12 {
        envelope.points.truncate(12);
        loss.envelope = true;
    }
}

#[allow(clippy::field_reassign_with_default)]
fn regroup_xm(song: &mut Song, loss: &mut Loss) {
    if song
        .instruments
        .iter()
        .any(|instrument| instrument.owned_samples > 0)
        && song
            .instruments
            .iter()
            .map(|instrument| usize::from(instrument.owned_samples))
            .sum::<usize>()
            == song.samples.len()
    {
        for instrument in &mut song.instruments {
            if instrument.keys.len() > 96 {
                instrument.keys.truncate(96);
            }
            for (index, key) in instrument.keys.iter_mut().enumerate() {
                key.note = index as u8;
            }
        }
        return;
    }
    let mut samples = Vec::new();
    let mut seen = vec![0u8; song.samples.len()];
    for instrument in &mut song.instruments {
        let mut used = Vec::new();
        let mut transposed = false;
        for (index, key) in instrument.keys.iter().enumerate() {
            if key.sample == 0 {
                continue;
            }
            if usize::from(key.note) != index {
                transposed = true;
            }
            let slot = usize::from(key.sample - 1);
            if slot < song.samples.len() && !used.contains(&slot) {
                used.push(slot);
            }
        }
        used.sort_unstable();
        for slot in &used {
            seen[*slot] = seen[*slot].saturating_add(1);
        }
        let base = samples.len();
        for slot in &used {
            samples.push(song.samples[*slot].clone());
        }
        instrument.owned_samples = u16::try_from(used.len()).unwrap_or(u16::MAX);
        for key in &mut instrument.keys {
            if key.sample == 0 {
                continue;
            }
            let slot = usize::from(key.sample - 1);
            key.sample = used
                .iter()
                .position(|owned| *owned == slot)
                .map(|local| u16::try_from(base + local + 1).unwrap_or(0))
                .unwrap_or(0);
        }
        if instrument.keys.len() > 96 {
            instrument.keys.truncate(96);
        } else {
            let from = instrument.keys.len();
            instrument.keys.resize(96, Key::default());
            for (index, key) in instrument.keys.iter_mut().enumerate().skip(from) {
                key.note = index as u8;
            }
        }
        for (index, key) in instrument.keys.iter_mut().enumerate() {
            key.note = index as u8;
        }
        if transposed {
            loss.transpose = true;
        }
    }
    if seen.iter().any(|count| *count > 1) {
        loss.shared = true;
    }
    for (index, sample) in song.samples.iter().enumerate() {
        if seen.get(index).copied().unwrap_or(0) == 0 && sample.pcm.iter().any(|frame| *frame != 0)
        {
            loss.orphan = true;
            if let Some(instrument) = song.instruments.last_mut() {
                instrument.owned_samples = instrument.owned_samples.saturating_add(1);
            }
            samples.push(sample.clone());
        }
    }
    if song.instruments.is_empty() && !samples.is_empty() {
        let mut instrument = Instrument::default();
        instrument.owned_samples = u16::try_from(samples.len()).unwrap_or(u16::MAX);
        instrument.keys = (0..96)
            .map(|note| Key {
                note: note as u8,
                sample: 1,
            })
            .collect();
        song.instruments.push(instrument);
    }
    song.samples = samples;
}

fn translate_cell(cell: &mut TrackCell, from: Format, to: Format, loss: &mut Loss) {
    if cell.has_volume {
        match volume_column(from, to, cell.volume) {
            Some(volume) => {
                cell.volume = volume;
                cell.has_volume = volume != 0 || to == Format::It;
            }
            None => {
                cell.volume = 0;
                cell.has_volume = false;
                loss.volume = true;
            }
        }
    }
    if cell.effect == 0 && cell.param == 0 {
        return;
    }
    match effect_column(from, to, cell.effect, cell.param) {
        Some((effect, param)) => {
            cell.effect = effect;
            cell.param = param;
        }
        None => {
            cell.effect = 0;
            cell.param = 0;
            loss.effect = true;
        }
    }
}

fn volume_column(from: Format, to: Format, volume: u8) -> Option<u8> {
    match (from, to) {
        (Format::Xm, Format::It) => match volume {
            0x10..=0x50 => Some(volume - 0x10),
            _ => None,
        },
        (Format::It, Format::Xm) => match volume {
            0..=64 => Some(0x10 + volume),
            _ => None,
        },
        _ => Some(volume),
    }
}

fn effect_column(from: Format, to: Format, effect: u8, param: u8) -> Option<(u8, u8)> {
    match (from, to) {
        (Format::Xm, Format::It) => xm_effect_to_it(effect, param),
        (Format::It, Format::Xm) => it_effect_to_xm(effect, param),
        _ => Some((effect, param)),
    }
}

fn xm_effect_to_it(effect: u8, param: u8) -> Option<(u8, u8)> {
    Some(match effect {
        0x00 => (10, param),
        0x01 => (6, param),
        0x02 => (5, param),
        0x03 => (7, param),
        0x04 => (8, param),
        0x05 => (12, param),
        0x06 => (11, param),
        0x07 => (18, param),
        0x08 => (24, param),
        0x09 => (15, param),
        0x0A => (4, param),
        0x0B => (2, param),
        0x0C => (13, param.min(64)),
        0x0D => (3, param),
        0x0F if param < 0x20 => (1, param),
        0x0F => (20, param),
        0x10 => (22, param),
        0x11 => (23, param),
        _ => return None,
    })
}

fn it_effect_to_xm(effect: u8, param: u8) -> Option<(u8, u8)> {
    Some(match effect {
        1 => (0x0F, param.min(0x1F)),
        2 => (0x0B, param),
        3 => (0x0D, param),
        4 => (0x0A, param),
        5 => (0x02, param),
        6 => (0x01, param),
        7 => (0x03, param),
        8 => (0x04, param),
        10 => (0x00, param),
        11 => (0x06, param),
        12 => (0x05, param),
        13 => (0x0C, param.min(64)),
        15 => (0x09, param),
        18 => (0x07, param),
        20 => (0x0F, param.max(0x20)),
        22 => (0x10, param.min(64)),
        23 => (0x11, param),
        24 => (0x08, param),
        _ => return None,
    })
}

#[derive(Default)]
struct Loss {
    channels: bool,
    rows: bool,
    notes: bool,
    effect: bool,
    volume: bool,
    envelope: bool,
    samples: bool,
    bits: bool,
    orders: bool,
    tuning: bool,
    transpose: bool,
    shared: bool,
    orphan: bool,
    filter: bool,
    nna: bool,
    skip: bool,
}

impl Loss {
    fn messages(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.channels {
            lines.push("Notes on channels the target cannot store were left out.".to_string());
        }
        if self.rows {
            lines.push("Pattern rows past the target's limit were left out.".to_string());
        }
        if self.notes {
            lines.push(
                "Notes outside C-1..B-3, and key-off, were cleared. A .mod has no key-off."
                    .to_string(),
            );
        }
        if self.effect {
            lines.push(
                "Effects with no equivalent in the target were cleared, including extended effects."
                    .to_string(),
            );
        }
        if self.volume {
            lines.push("Volume-column commands other than set-volume were cleared.".to_string());
        }
        if self.envelope {
            lines.push(
                "Envelopes were left out. A .mod has none, and XM keeps 12 nodes.".to_string(),
            );
        }
        if self.samples {
            lines.push("Samples past 31 were left out. A .mod stores 31.".to_string());
        }
        if self.bits {
            lines.push("16-bit samples were stored as 8-bit.".to_string());
        }
        if self.orders {
            lines.push("The order list was shortened to 128 entries.".to_string());
        }
        if self.tuning {
            lines.push(
                "Sample tuning was reduced to ProTracker finetune, or centered XM panning replaced hard Amiga pan."
                    .to_string(),
            );
        }
        if self.transpose {
            lines.push(
                "Per-key sample transpose was left out. XM transpose is per sample.".to_string(),
            );
        }
        if self.shared {
            lines.push(
                "A sample used by more than one instrument was copied into each instrument."
                    .to_string(),
            );
        }
        if self.orphan {
            lines.push(
                "A sample no instrument played was appended to the last instrument.".to_string(),
            );
        }
        if self.filter {
            lines.push("Filter envelopes were left out. XM has no resonant filter.".to_string());
        }
        if self.nna {
            lines.push("New-note actions were left out. XM cuts the previous note.".to_string());
        }
        if self.skip {
            lines.push("IT skip markers (+++) were removed from the order list.".to_string());
        }
        lines
    }
}

fn song_to_mod(song: &Song) -> Result<Saved, Error> {
    let mut loss = Loss::default();
    if song.channels > CHANNELS {
        loss.channels = true;
    }
    let mut module = Module::new(Tag::Mk);
    let title: String = song.title.chars().take(20).collect();
    module.set_title(&title)?;
    let mut played = Vec::new();
    for order in &song.orders {
        if song.format == Format::It && *order == 255 {
            break;
        }
        if song.format == Format::It && *order == 254 {
            loss.skip = true;
            continue;
        }
        played.push(*order);
    }
    if played.len() > ORDER_LEN {
        played.truncate(ORDER_LEN);
        loss.orders = true;
    }
    if played.is_empty() {
        played.push(0);
    }
    module.song_length = u8::try_from(played.len()).unwrap_or(1).max(1);
    for (index, pattern) in played.iter().enumerate() {
        module.order[index] = *pattern;
    }
    module.restart = u8::try_from(song.restart.min(u16::from(u8::MAX))).unwrap_or(0);
    let needed = module.required_pattern_count();
    if song
        .patterns
        .iter()
        .any(|pattern| pattern.row_count() != ROWS)
    {
        loss.rows = true;
    }
    for index in 0..needed {
        let mut pattern = crate::module::Pattern::empty();
        if let Some(source) = song.patterns.get(index) {
            for row in 0..ROWS.min(source.row_count()) {
                for channel in 0..CHANNELS {
                    let cell = source
                        .rows
                        .get(row)
                        .and_then(|cells| cells.get(channel))
                        .copied()
                        .unwrap_or_else(TrackCell::empty);
                    pattern.rows[row][channel] = track_cell_to_mod(song, cell, &mut loss);
                }
            }
        }
        module.patterns.push(pattern);
    }
    if module.patterns.is_empty() {
        module.patterns.push(crate::module::Pattern::empty());
    }
    if module.patterns.len() > 64 || module.order.iter().any(|slot| *slot >= 64) {
        module.tag = Tag::Extended;
    }
    for (index, sample) in song.samples.iter().take(SAMPLE_COUNT).enumerate() {
        write_mod_sample(&mut module.samples[index], sample, &mut loss)?;
    }
    if song.samples.len() > SAMPLE_COUNT {
        loss.samples = true;
    }
    if song.instruments.iter().any(|instrument| {
        instrument.volume_env.enabled || instrument.pan_env.enabled || instrument.pitch_env.enabled
    }) {
        loss.envelope = true;
    }
    let omitted_from = if song.patterns.len() > module.patterns.len() {
        Some(module.patterns.len())
    } else {
        None
    };
    let (bytes, stored_omit) = module.to_stored_bytes()?;
    let mut warnings = loss.messages();
    if omitted_from.is_some() || stored_omit.is_some() {
        warnings.push(
            "Patterns past the highest order entry were not written. A .mod stores max(order)+1."
                .to_string(),
        );
    }
    Ok(Saved {
        bytes,
        warnings,
        omitted_from: stored_omit.or(omitted_from),
    })
}

fn track_cell_to_mod(song: &Song, cell: TrackCell, loss: &mut Loss) -> crate::module::Cell {
    let period = match cell.note {
        0 => 0,
        NOTE_OFF | NOTE_CUT | NOTE_FADE => {
            loss.notes = true;
            0
        }
        note => match period_from_note(note) {
            Some(period) => period,
            None => {
                loss.notes = true;
                0
            }
        },
    };
    let sample = mod_sample_number(song, &cell).min(31);
    let (mut effect, mut param) = (0u8, 0u8);
    if cell.effect != 0 || cell.param != 0 {
        let xm = match song.format {
            Format::Xm => Some((cell.effect, cell.param)),
            Format::It => it_effect_to_xm(cell.effect, cell.param),
        };
        match xm {
            Some((command, value)) if command <= 0x0F => {
                effect = command;
                param = value;
            }
            Some(_) | None => loss.effect = true,
        }
    }
    if cell.has_volume && effect == 0 && param == 0 {
        if let Some(volume) = set_volume(song.format, cell.volume) {
            effect = 0x0C;
            param = volume;
        } else {
            loss.volume = true;
        }
    } else if cell.has_volume {
        loss.volume = true;
    }
    crate::module::Cell {
        sample,
        period,
        effect,
        param,
    }
}

fn set_volume(format: Format, volume: u8) -> Option<u8> {
    match format {
        Format::Xm => match volume {
            0x10..=0x50 => Some(volume - 0x10),
            _ => None,
        },
        Format::It => match volume {
            0..=64 => Some(volume),
            _ => None,
        },
    }
}

fn mod_sample_number(song: &Song, cell: &TrackCell) -> u8 {
    if cell.instrument == 0 {
        return 0;
    }
    let Some(instrument) = song.instruments.get(usize::from(cell.instrument - 1)) else {
        return cell.instrument;
    };
    if (1..=120).contains(&cell.note) {
        if let Some((sample, _)) = instrument.map_note(cell.note - 1) {
            return u8::try_from(sample).unwrap_or(0);
        }
    }
    instrument
        .keys
        .iter()
        .find_map(|key| (key.sample > 0).then_some(u8::try_from(key.sample).unwrap_or(0)))
        .unwrap_or(cell.instrument)
}

fn period_from_note(note: u8) -> Option<u16> {
    let semi = usize::from(note.saturating_sub(1));
    // Note 1 is C-0. ProTracker C-1 is note 13, PERIODS[0].
    semi.checked_sub(12)
        .and_then(|index| PERIODS.get(index).copied())
}

fn period_to_note(period: u16) -> Option<u8> {
    if period == 0 {
        return Some(0);
    }
    PERIODS
        .iter()
        .position(|stored| *stored == period)
        .map(|index| u8::try_from(index + 13).unwrap_or(0))
}

fn write_mod_sample(
    dest: &mut crate::module::Sample,
    sample: &TrackSample,
    loss: &mut Loss,
) -> Result<(), Error> {
    let name: String = sample.name.chars().take(22).collect();
    dest.set_name(&name)?;
    dest.volume = sample.volume.min(64);
    let finetune = mod_finetune(sample);
    if finetune != sample.finetune / 16 || (sample.c5_speed != 0 && sample.c5_speed != 8363) {
        loss.tuning = true;
    }
    let nibble = if finetune < 0 {
        u8::try_from(finetune + 16).unwrap_or(0)
    } else {
        u8::try_from(finetune).unwrap_or(0)
    };
    dest.finetune_raw = nibble & 0x0F;
    let mut data: Vec<u8> = sample.pcm.iter().map(|frame| (*frame >> 8) as u8).collect();
    if sample.bits == 16 || sample.pcm.iter().any(|frame| frame & 0x00FF != 0) {
        loss.bits = true;
    }
    if data.len() % 2 == 1 {
        data.push(0);
    }
    if data.len() > crate::module::MAX_SAMPLE_BYTES {
        data.truncate(crate::module::MAX_SAMPLE_BYTES);
        if data.len() % 2 == 1 {
            data.pop();
        }
        loss.samples = true;
    }
    dest.set_data(data)?;
    if sample.loop_kind != LoopKind::None && sample.loop_end > sample.loop_start {
        let start = usize::try_from(sample.loop_start).unwrap_or(0);
        let end = usize::try_from(sample.loop_end)
            .unwrap_or(0)
            .min(dest.data.len());
        let start = start.min(end) & !1;
        let end = end & !1;
        let length = end.saturating_sub(start);
        if length >= 4 {
            dest.loop_start = u16::try_from(start / 2).unwrap_or(u16::MAX);
            dest.loop_length = u16::try_from(length / 2).unwrap_or(1).max(2);
        } else {
            dest.loop_length = 1;
        }
    } else {
        dest.loop_length = 1;
    }
    Ok(())
}

fn mod_finetune(sample: &TrackSample) -> i8 {
    let from_fine = (i16::from(sample.finetune) / 16).clamp(-8, 7);
    if sample.c5_speed == 0 || sample.c5_speed == 8363 {
        return from_fine as i8;
    }
    let ratio = f64::from(sample.c5_speed) / 8363.0;
    if ratio <= 0.0 {
        return from_fine as i8;
    }
    let steps = (ratio.log2() * 12.0 * 8.0).round() as i32;
    let with_note = steps + i32::from(sample.relative_note) * 8 + i32::from(from_fine);
    with_note.clamp(-8, 7) as i8
}

#[allow(clippy::field_reassign_with_default)]
fn module_to_song(module: &Module, format: Format) -> (Song, Vec<String>) {
    let mut warnings = Vec::new();
    if format == Format::Xm {
        warnings.push(
            "XM sample panning is centered. ProTracker hard pan is not stored on the sample."
                .to_string(),
        );
    }
    let mut song = Song {
        title: module.display_title(),
        tracker: if format == Format::Xm {
            "omatrack".to_string()
        } else {
            "IT 0x0214".to_string()
        },
        format,
        channels: CHANNELS,
        orders: module.order[..usize::from(module.song_length).clamp(1, ORDER_LEN)].to_vec(),
        restart: u16::from(module.restart),
        patterns: Vec::new(),
        instruments: Vec::new(),
        samples: Vec::new(),
        linear: false,
        initial_speed: 6,
        initial_tempo: 125,
        initial_global_volume: if format == Format::Xm { 64 } else { 128 },
        global_volume_max: if format == Format::Xm { 64 } else { 128 },
        initial_pan: if format == Format::It {
            // Left, right, right, left. 0 and 255 survive the IT pan scale.
            vec![0, 255, 255, 0]
        } else {
            vec![0x80; CHANNELS]
        },
        initial_channel_volume: vec![64; CHANNELS],
        initial_mute: vec![false; CHANNELS],
        instrument_mode: true,
        old_effects: false,
        compatible_gxx: false,
        compat: if format == Format::It { 0x0214 } else { 0 },
        load_notes: super::song::LoadNotes::default(),
    };
    let key_count = if format == Format::Xm { 96 } else { 120 };
    for (index, sample) in module.samples.iter().enumerate() {
        let mut track = TrackSample::default();
        track.name = sample.display_name();
        track.pcm = sample
            .data
            .iter()
            .map(|byte| i16::from(*byte as i8) << 8)
            .collect();
        track.bits = 8;
        track.volume = sample.volume.min(64);
        track.global_volume = 64;
        track.finetune = sample.finetune().saturating_mul(16);
        track.c5_speed = 8363;
        if format == Format::Xm {
            track.pan = Some(0x80);
        }
        if sample.loops() {
            track.loop_kind = LoopKind::Forward;
            track.loop_start = u32::from(sample.loop_start) * 2;
            track.loop_end = track.loop_start + u32::from(sample.loop_length) * 2;
        }
        song.samples.push(track);
        let mut instrument = Instrument::default();
        instrument.name = sample.display_name();
        instrument.owned_samples = if format == Format::Xm { 1 } else { 0 };
        instrument.keys = (0..key_count)
            .map(|note| Key {
                note: note as u8,
                sample: u16::try_from(index + 1).unwrap_or(0),
            })
            .collect();
        song.instruments.push(instrument);
    }
    let mut loss = Loss::default();
    for pattern in &module.patterns {
        let mut rows = Vec::with_capacity(ROWS);
        for row in &pattern.rows {
            let mut cells = Vec::with_capacity(CHANNELS);
            for cell in row {
                cells.push(mod_cell_to_track(cell, format, &mut loss));
            }
            rows.push(cells);
        }
        song.patterns.push(TrackPattern { rows });
    }
    if song.patterns.is_empty() {
        song.patterns.push(TrackPattern::empty(ROWS, CHANNELS));
    }
    warnings.extend(loss.messages());
    (song, warnings)
}

fn mod_cell_to_track(cell: &crate::module::Cell, format: Format, loss: &mut Loss) -> TrackCell {
    let note = match period_to_note(cell.period) {
        Some(note) => note,
        None => {
            loss.notes = true;
            0
        }
    };
    let (effect, param) = if cell.effect == 0 && cell.param == 0 {
        (0, 0)
    } else if format == Format::Xm {
        (cell.effect, cell.param)
    } else {
        match xm_effect_to_it(cell.effect, cell.param) {
            Some(effect) => effect,
            None => {
                loss.effect = true;
                (0, 0)
            }
        }
    };
    TrackCell {
        note,
        instrument: cell.sample,
        volume: 0,
        effect,
        param,
        has_volume: false,
    }
}

fn track_empty(cell: &TrackCell) -> bool {
    cell.note == 0
        && cell.instrument == 0
        && !cell.has_volume
        && cell.effect == 0
        && cell.param == 0
}
