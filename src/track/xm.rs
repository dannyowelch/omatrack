//! FastTracker 2 `.xm` reader.
//!
//! The magic is `Extended Module: ` at the start of the file. A short or
//! inconsistent file returns [`Error`](crate::Error); it does not panic.

use super::song::{
    Cell, EnvPoint, Envelope, Format, Instrument, Key, LoopKind, Pattern, Sample, Song, Vibrato,
    NOTE_CUT, NOTE_FADE, NOTE_OFF,
};
use crate::error::Error;

const MAGIC: &[u8] = b"Extended Module: ";
const MAX_CHANNELS: usize = 32;
const MAX_PATTERNS: usize = 256;
const MAX_INSTRUMENTS: usize = 256;
const MAX_SAMPLES: usize = 256;
const MAX_ROWS: usize = 256;
const MAX_SAMPLE_BYTES: usize = 8_000_000;

/// `true` when `bytes` starts with the XM magic.
pub fn is_xm(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

/// Parse an XM module.
pub fn parse(bytes: &[u8]) -> Result<Song, Error> {
    if !is_xm(bytes) {
        return Err(Error::Malformed(
            "XM file does not start with \"Extended Module: \"".to_string(),
        ));
    }
    let mut c = Cursor::new(bytes);
    c.skip(17, "XM magic")?;
    let title = latin1(c.bytes(20, "XM title")?);
    let marker = c.u8("XM title marker")?;
    if marker != 0x1A {
        return Err(Error::Malformed(format!(
            "XM title marker is {marker:#04x}; expected 0x1A"
        )));
    }
    let tracker = latin1(c.bytes(20, "XM tracker name")?);
    let version = c.u16("XM version")?;
    let header_size = c.u32("XM header size")? as usize;
    if header_size < 20 {
        return Err(Error::Malformed(format!(
            "XM header size {header_size} is shorter than the required fields"
        )));
    }
    let header_end = c.pos + header_size - 4;
    if header_end > bytes.len() {
        return Err(truncated("XM header", header_end, bytes.len()));
    }
    let song_length = c.u16("XM song length")? as usize;
    let restart = c.u16("XM restart")?;
    let channels = c.u16("XM channel count")? as usize;
    let pattern_count = c.u16("XM pattern count")? as usize;
    let instrument_count = c.u16("XM instrument count")? as usize;
    let flags = c.u16("XM flags")?;
    let speed = c.u16("XM speed")?;
    let tempo = c.u16("XM tempo")?;
    if !(1..=256).contains(&song_length) {
        return Err(Error::Malformed(format!(
            "XM song length {song_length} is outside 1..=256"
        )));
    }
    if !(1..=MAX_CHANNELS).contains(&channels) {
        return Err(Error::Malformed(format!(
            "XM channel count {channels} is outside 1..={MAX_CHANNELS}"
        )));
    }
    if pattern_count > MAX_PATTERNS {
        return Err(Error::Malformed(format!(
            "XM pattern count {pattern_count} is above {MAX_PATTERNS}"
        )));
    }
    if instrument_count > MAX_INSTRUMENTS {
        return Err(Error::Malformed(format!(
            "XM instrument count {instrument_count} is above {MAX_INSTRUMENTS}"
        )));
    }
    let mut orders = vec![0u8; 256];
    let have = header_end.saturating_sub(c.pos).min(256);
    if have > 0 {
        orders[..have].copy_from_slice(c.bytes(have, "XM order list")?);
    }
    if c.pos < header_end {
        c.skip(header_end - c.pos, "XM header padding")?;
    }
    let orders: Vec<u8> = orders.into_iter().take(song_length).collect();
    let mut initial_pan = Vec::new();
    // Some files store one pan byte per channel after the 256-byte order table.
    // Those bytes were already consumed as padding when header_size > 276.
    // Re-read them from the header when they are present.
    if header_size >= 276 + channels {
        let pan_at = 60 + 20 + 256;
        if pan_at + channels <= bytes.len() {
            initial_pan = bytes[pan_at..pan_at + channels].to_vec();
        }
    }
    if initial_pan.len() != channels {
        initial_pan = vec![0x80; channels];
    }

    let mut patterns = Vec::with_capacity(pattern_count);
    for index in 0..pattern_count {
        patterns.push(parse_pattern(&mut c, channels, version, index)?);
    }
    let mut instruments = Vec::with_capacity(instrument_count);
    let mut samples = Vec::new();
    for index in 0..instrument_count {
        let (mut instrument, more) = parse_instrument(&mut c, index)?;
        let owned = more.len();
        instrument.owned_samples = u16::try_from(owned).unwrap_or(u16::MAX);
        let base = u16::try_from(samples.len()).unwrap_or(u16::MAX);
        assign_sample_base(&mut instrument, base, owned);
        samples.extend(more);
        if samples.len() > MAX_SAMPLES {
            return Err(Error::Malformed(format!(
                "XM sample count is above {MAX_SAMPLES}"
            )));
        }
        instruments.push(instrument);
    }

    let linear = flags & 1 != 0;
    let mut load_notes = Vec::new();
    if flags & !1 != 0 {
        load_notes.push(
            "XM flags other than linear slides were not kept and will not be written back."
                .to_string(),
        );
    }
    Ok(Song {
        title,
        tracker,
        format: Format::Xm,
        channels,
        orders,
        restart,
        patterns,
        instruments,
        samples,
        linear,
        initial_speed: u8::try_from(speed).unwrap_or(6).max(1),
        initial_tempo: u8::try_from(tempo.max(32)).unwrap_or(255),
        initial_global_volume: 64,
        global_volume_max: 64,
        initial_pan,
        initial_channel_volume: vec![64; channels],
        initial_mute: vec![false; channels],
        instrument_mode: true,
        old_effects: false,
        compatible_gxx: false,
        compat: 0,
        load_notes: super::song::LoadNotes { lines: load_notes },
    })
}

fn parse_pattern(
    c: &mut Cursor<'_>,
    channels: usize,
    version: u16,
    index: usize,
) -> Result<Pattern, Error> {
    let ctx = "XM pattern header";
    let header_size = c.u32(ctx)? as usize;
    if header_size < 4 {
        return Err(Error::Malformed(format!(
            "XM pattern {index} header size {header_size} is too small"
        )));
    }
    let header_end = c.pos + header_size - 4;
    if header_end > c.data.len() {
        return Err(truncated(ctx, header_end, c.data.len()));
    }
    let mut rows = 64usize;
    let mut packed_size = 0usize;
    if header_size >= 9 {
        let _packing = c.u8("XM pattern packing")?;
        rows = if version >= 0x0104 {
            c.u16("XM pattern rows")? as usize
        } else {
            let _ = c.u16("XM pattern rows")?;
            64
        };
        packed_size = c.u16("XM pattern size")? as usize;
    } else if header_size >= 5 {
        let _packing = c.u8("XM pattern packing")?;
    }
    if !(1..=MAX_ROWS).contains(&rows) {
        return Err(Error::Malformed(format!(
            "XM pattern {index} has {rows} rows; expected 1..={MAX_ROWS}"
        )));
    }
    if c.pos < header_end {
        c.skip(header_end - c.pos, "XM pattern header padding")?;
    }
    let packed = c.bytes(packed_size, "XM pattern data")?;
    unpack_pattern(packed, rows, channels, index)
}

fn unpack_pattern(
    packed: &[u8],
    rows: usize,
    channels: usize,
    index: usize,
) -> Result<Pattern, Error> {
    let mut pattern = Pattern::empty(rows, channels);
    if packed.is_empty() {
        return Ok(pattern);
    }
    let mut p = 0usize;
    for row in 0..rows {
        for channel in 0..channels {
            if p >= packed.len() {
                return Err(Error::Malformed(format!(
                    "XM pattern {index} ran out of packed data at row {row}, channel {}",
                    channel + 1
                )));
            }
            let flags = packed[p];
            p += 1;
            let (note, instrument, volume, effect, param) = if flags & 0x80 != 0 {
                let note = take_flag(packed, &mut p, flags, 0x01)?;
                let instrument = take_flag(packed, &mut p, flags, 0x02)?;
                let volume = take_flag(packed, &mut p, flags, 0x04)?;
                let effect = take_flag(packed, &mut p, flags, 0x08)?;
                let param = take_flag(packed, &mut p, flags, 0x10)?;
                (note, instrument, volume, effect, param)
            } else {
                let note = flags;
                let instrument = take(packed, &mut p, "XM instrument")?;
                let volume = take(packed, &mut p, "XM volume")?;
                let effect = take(packed, &mut p, "XM effect")?;
                let param = take(packed, &mut p, "XM parameter")?;
                (note, instrument, volume, effect, param)
            };
            pattern.rows[row][channel] = Cell {
                note: match note {
                    97 => NOTE_OFF,
                    0 => 0,
                    1..=96 => note,
                    other => {
                        return Err(Error::Malformed(format!(
                            "XM pattern {index} row {row} has note {other}; expected 0..=97"
                        )));
                    }
                },
                instrument,
                volume,
                effect,
                param,
                has_volume: volume != 0,
            };
        }
    }
    Ok(pattern)
}

fn take_flag(packed: &[u8], p: &mut usize, flags: u8, bit: u8) -> Result<u8, Error> {
    if flags & bit == 0 {
        Ok(0)
    } else {
        take(packed, p, "XM packed cell")
    }
}

fn take(packed: &[u8], p: &mut usize, ctx: &'static str) -> Result<u8, Error> {
    if *p >= packed.len() {
        return Err(Error::Malformed(format!(
            "{ctx} ended early ({len} packed bytes)",
            len = packed.len()
        )));
    }
    let byte = packed[*p];
    *p += 1;
    Ok(byte)
}

fn parse_instrument(c: &mut Cursor<'_>, index: usize) -> Result<(Instrument, Vec<Sample>), Error> {
    let size = c.u32("XM instrument header size")? as usize;
    if size < 29 {
        return Err(Error::Malformed(format!(
            "XM instrument {} header size {size} is shorter than 29",
            index + 1
        )));
    }
    let header_end = c.pos + size - 4;
    if header_end > c.data.len() {
        return Err(truncated("XM instrument header", header_end, c.data.len()));
    }
    let name = latin1(c.bytes(22, "XM instrument name")?);
    let _type = c.u8("XM instrument type")?;
    let sample_count = c.u16("XM sample count")? as usize;
    if sample_count > 128 {
        return Err(Error::Malformed(format!(
            "XM instrument {} has {sample_count} samples; expected at most 128",
            index + 1
        )));
    }
    let mut instrument = Instrument {
        name,
        ..Instrument::default()
    };
    let mut sample_header_size = 40usize;
    if sample_count > 0 {
        if c.pos + 4 > header_end {
            return Err(Error::Malformed(format!(
                "XM instrument {} names {sample_count} samples but the header ends first",
                index + 1
            )));
        }
        sample_header_size = c.u32("XM sample header size")? as usize;
        if !(4..=256).contains(&sample_header_size) {
            return Err(Error::Malformed(format!(
                "XM instrument {} sample header size {sample_header_size} is not usable",
                index + 1
            )));
        }
        let extra = header_end.saturating_sub(c.pos);
        let mut extra_bytes = vec![0u8; extra];
        if extra > 0 {
            extra_bytes.copy_from_slice(c.bytes(extra, "XM instrument extra")?);
        }
        if extra >= 96 + 48 + 48 + 14 {
            instrument.keys = (0..96)
                .map(|note| Key {
                    note: note as u8,
                    sample: 0,
                })
                .collect();
            // Sample numbers in the map are 0-based into this instrument. The
            // caller assigns global numbers after the samples are appended.
            let map = &extra_bytes[..96];
            for (note, slot) in map.iter().enumerate() {
                instrument.keys[note].sample = u16::from(*slot).saturating_add(1);
            }
            instrument.volume_env = read_envelope(&extra_bytes[96..144], true);
            instrument.pan_env = read_envelope(&extra_bytes[144..192], true);
            let tail = &extra_bytes[192..];
            if tail.len() >= 14 {
                apply_env_flags(&mut instrument.volume_env, tail);
                apply_pan_flags(&mut instrument.pan_env, tail);
                instrument.vibrato = Vibrato {
                    kind: tail[10],
                    sweep: tail[11],
                    depth: tail[12],
                    rate: tail[13],
                };
            }
            if tail.len() >= 16 {
                instrument.fadeout = u16::from_le_bytes([tail[14], tail[15]]);
            }
        }
    }
    if instrument.keys.is_empty() && sample_count > 0 {
        instrument.keys = (0..96)
            .map(|note| Key {
                note: note as u8,
                sample: 1,
            })
            .collect();
    }
    if c.pos < header_end {
        c.skip(header_end - c.pos, "XM instrument padding")?;
    }
    let mut headers = Vec::with_capacity(sample_count);
    for sample_index in 0..sample_count {
        headers.push(read_sample_header(
            c,
            sample_header_size,
            index,
            sample_index,
        )?);
    }
    let mut samples = Vec::with_capacity(sample_count);
    for header in headers {
        samples.push(read_sample_body(c, header)?);
    }
    // Key map samples are 1-based within the instrument. Leave them; the
    // engine adds the instrument's sample base. Store the base in unused
    // global volume? No: remap here once we know how many samples this
    // instrument owns, but the global base is the count BEFORE these samples
    // were conceptually appended. The caller extends `samples` after return,
    // so the base is unknown here.
    //
    // Encode the instrument-local index (already 1-based) and let `parse`
    // remap. Done below by the caller... I'll remap in `parse` by passing
    // the base. Easiest: set a sentinel and remap in parse().
    Ok((instrument, samples))
}

struct SampleHeader {
    length: usize,
    loop_start: u32,
    loop_length: u32,
    volume: u8,
    finetune: i8,
    flags: u8,
    panning: u8,
    relative: i8,
    name: String,
}

fn read_sample_header(
    c: &mut Cursor<'_>,
    size: usize,
    instrument: usize,
    sample: usize,
) -> Result<SampleHeader, Error> {
    let end = c.pos + size;
    if end > c.data.len() {
        return Err(truncated("XM sample header", end, c.data.len()));
    }
    if size < 18 {
        return Err(Error::Malformed(format!(
            "XM instrument {} sample {} header is {size} bytes; need at least 18",
            instrument + 1,
            sample + 1
        )));
    }
    let length = c.u32("XM sample length")? as usize;
    let loop_start = c.u32("XM loop start")?;
    let loop_length = c.u32("XM loop length")?;
    let volume = c.u8("XM sample volume")?;
    let finetune = c.i8("XM sample finetune")?;
    let flags = c.u8("XM sample flags")?;
    let panning = c.u8("XM sample panning")?;
    let relative = c.i8("XM relative note")?;
    let _reserved = c.u8("XM sample reserved")?;
    let name = if c.pos + 22 <= end {
        latin1(c.bytes(22, "XM sample name")?)
    } else {
        String::new()
    };
    if c.pos < end {
        c.skip(end - c.pos, "XM sample header padding")?;
    }
    if length > MAX_SAMPLE_BYTES {
        return Err(Error::Malformed(format!(
            "XM instrument {} sample {} is {length} bytes; the limit is {MAX_SAMPLE_BYTES}",
            instrument + 1,
            sample + 1
        )));
    }
    Ok(SampleHeader {
        length,
        loop_start,
        loop_length,
        volume,
        finetune,
        flags,
        panning,
        relative,
        name,
    })
}

fn read_sample_body(c: &mut Cursor<'_>, header: SampleHeader) -> Result<Sample, Error> {
    let raw = c.bytes(header.length, "XM sample data")?;
    let bits16 = header.flags & 0x10 != 0;
    let bps = if bits16 { 2 } else { 1 };
    if bits16 && header.length % 2 != 0 {
        return Err(Error::Malformed(format!(
            "XM 16-bit sample \"{}\" has odd length {}",
            header.name, header.length
        )));
    }
    let frames = header.length / bps;
    let mut pcm = Vec::with_capacity(frames);
    let mut acc: i16 = 0;
    if bits16 {
        for chunk in raw.chunks_exact(2) {
            let delta = i16::from_le_bytes([chunk[0], chunk[1]]);
            acc = acc.wrapping_add(delta);
            pcm.push(acc);
        }
    } else {
        for byte in raw {
            acc = acc.wrapping_add(i16::from(*byte as i8));
            pcm.push(acc.wrapping_shl(8));
        }
    }
    let (loop_start, loop_end, loop_kind) = sample_loop(&header, frames, bps);
    Ok(Sample {
        name: header.name,
        pcm,
        bits: if bits16 { 16 } else { 8 },
        volume: header.volume.min(64),
        global_volume: 64,
        pan: Some(header.panning),
        finetune: header.finetune,
        relative_note: header.relative,
        c5_speed: 8363,
        loop_start,
        loop_end,
        loop_kind,
        ..Sample::default()
    })
}

fn sample_loop(header: &SampleHeader, frames: usize, bps: usize) -> (u32, u32, LoopKind) {
    let kind = match header.flags & 0x03 {
        1 => LoopKind::Forward,
        2 | 3 => LoopKind::PingPong,
        _ => LoopKind::None,
    };
    if kind == LoopKind::None || header.loop_length == 0 {
        return (0, 0, LoopKind::None);
    }
    let start = (header.loop_start as usize / bps).min(frames);
    let len = (header.loop_length as usize / bps).min(frames.saturating_sub(start));
    if len == 0 {
        return (0, 0, LoopKind::None);
    }
    (start as u32, (start + len) as u32, kind)
}

fn read_envelope(bytes: &[u8], _signed_y: bool) -> Envelope {
    let mut points = Vec::new();
    for point in 0..12 {
        let at = point * 4;
        if at + 4 > bytes.len() {
            break;
        }
        let tick = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let value = u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]).min(64) as u8;
        points.push((tick, value));
    }
    Envelope {
        points,
        ..Envelope::default()
    }
}

fn apply_env_flags(env: &mut Envelope, tail: &[u8]) {
    let count = usize::from(tail[0]).min(12).min(env.points.len());
    env.points.truncate(count);
    env.sustain_point = tail[2].min(count.saturating_sub(1) as u8);
    env.sustain_end = env.sustain_point;
    env.loop_start = tail[3].min(count.saturating_sub(1) as u8);
    env.loop_end = tail[4].min(count.saturating_sub(1) as u8);
    let flags = tail[8];
    env.enabled = flags & 1 != 0 && count > 0;
    env.sustain = flags & 2 != 0 && count > 0;
    env.loop_on = flags & 4 != 0 && count > 1;
    sort_points(env);
}

fn apply_pan_flags(env: &mut Envelope, tail: &[u8]) {
    let count = usize::from(tail[1]).min(12).min(env.points.len());
    env.points.truncate(count);
    env.sustain_point = tail[5].min(count.saturating_sub(1) as u8);
    env.sustain_end = env.sustain_point;
    env.loop_start = tail[6].min(count.saturating_sub(1) as u8);
    env.loop_end = tail[7].min(count.saturating_sub(1) as u8);
    let flags = tail[9];
    env.enabled = flags & 1 != 0 && count > 0;
    env.sustain = flags & 2 != 0 && count > 0;
    env.loop_on = flags & 4 != 0 && count > 1;
    sort_points(env);
}

fn sort_points(env: &mut Envelope) {
    env.points.sort_by_key(|point: &EnvPoint| point.0);
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn u8(&mut self, ctx: &'static str) -> Result<u8, Error> {
        Ok(self.bytes(1, ctx)?[0])
    }

    fn i8(&mut self, ctx: &'static str) -> Result<i8, Error> {
        Ok(self.u8(ctx)? as i8)
    }

    fn u16(&mut self, ctx: &'static str) -> Result<u16, Error> {
        let b = self.bytes(2, ctx)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self, ctx: &'static str) -> Result<u32, Error> {
        let b = self.bytes(4, ctx)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn bytes(&mut self, n: usize, ctx: &'static str) -> Result<&'a [u8], Error> {
        let end = self.pos.saturating_add(n);
        if n > self.data.len() || end > self.data.len() {
            return Err(truncated(ctx, self.pos.saturating_add(n), self.data.len()));
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn skip(&mut self, n: usize, ctx: &'static str) -> Result<(), Error> {
        let _ = self.bytes(n, ctx)?;
        Ok(())
    }
}

fn truncated(context: &'static str, expected: usize, actual: usize) -> Error {
    Error::Truncated {
        context,
        expected,
        actual,
    }
}

fn latin1(bytes: &[u8]) -> String {
    // Drop C0 controls and DEL. An interior NUL is not a column, so drawing
    // it would slide the rest of the row over the pane border.
    let end = bytes
        .iter()
        .rposition(|byte| *byte > b' ' && *byte != 0x7F)
        .map(|index| index + 1)
        .unwrap_or(0);
    bytes[..end]
        .iter()
        .filter(|byte| **byte >= 0x20 && **byte != 0x7F)
        .map(|byte| char::from(*byte))
        .collect::<String>()
        .trim()
        .to_string()
}

pub(crate) fn assign_sample_base(instrument: &mut Instrument, base: u16, owned: usize) {
    for key in &mut instrument.keys {
        if key.sample == 0 {
            continue;
        }
        let local = usize::from(key.sample - 1);
        if local < owned {
            key.sample += base;
        } else {
            key.sample = 0;
        }
    }
}

/// Encode `song` as a FastTracker 2 module.
///
/// The bytes load back to the same song for everything [`parse`] keeps.
/// `warnings` names notes, rows, or samples that had to be narrowed.
pub(crate) fn write(song: &Song) -> Result<(Vec<u8>, Vec<String>), Error> {
    let mut warnings = Vec::new();
    if !(1..=MAX_CHANNELS).contains(&song.channels) {
        return Err(Error::Malformed(format!(
            "XM stores 1 to {MAX_CHANNELS} channels; this song has {}. Nothing was written.",
            song.channels
        )));
    }
    if song.patterns.len() > MAX_PATTERNS || song.instruments.len() > MAX_INSTRUMENTS {
        return Err(Error::Malformed(format!(
            "XM stores at most {MAX_PATTERNS} patterns and {MAX_INSTRUMENTS} instruments. Nothing was written."
        )));
    }
    let mut orders = song.orders.clone();
    if orders.is_empty() {
        orders.push(0);
        warnings
            .push("The order list was empty. One order pointing at pattern 0 was written.".into());
    }
    if orders.len() > 256 {
        orders.truncate(256);
        warnings.push("XM stores 256 orders. Later orders were left out of the file.".into());
    }
    let (ranges, range_warning) = sample_ranges(song);
    if let Some(warning) = range_warning {
        warnings.push(warning);
    }
    if ranges.iter().map(|(start, end)| end - start).sum::<usize>() > MAX_SAMPLES {
        return Err(Error::Malformed(format!(
            "XM stores at most {MAX_SAMPLES} samples. Nothing was written."
        )));
    }

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put_text(&mut out, &song.title, 20);
    out.push(0x1A);
    put_text(&mut out, &song.tracker, 20);
    push_u16(&mut out, 0x0104);
    let header_size = 276 + song.channels as u32;
    push_u32(&mut out, header_size);
    push_u16(&mut out, orders.len() as u16);
    push_u16(&mut out, song.restart);
    push_u16(&mut out, song.channels as u16);
    push_u16(&mut out, song.patterns.len() as u16);
    push_u16(&mut out, song.instruments.len() as u16);
    push_u16(&mut out, u16::from(song.linear));
    push_u16(&mut out, u16::from(song.initial_speed.max(1)));
    push_u16(&mut out, u16::from(song.initial_tempo.max(32)));
    let mut order_bytes = [0u8; 256];
    order_bytes[..orders.len()].copy_from_slice(&orders);
    out.extend_from_slice(&order_bytes);
    for channel in 0..song.channels {
        out.push(song.initial_pan.get(channel).copied().unwrap_or(0x80));
    }

    for (index, pattern) in song.patterns.iter().enumerate() {
        write_pattern(&mut out, pattern, song.channels, index, &mut warnings)?;
    }
    for (index, instrument) in song.instruments.iter().enumerate() {
        let (start, end) = ranges.get(index).copied().unwrap_or((0, 0));
        write_instrument(
            &mut out,
            instrument,
            &song.samples[start..end],
            start,
            index,
            &mut warnings,
        )?;
    }
    Ok((out, warnings))
}

fn sample_ranges(song: &Song) -> (Vec<(usize, usize)>, Option<String>) {
    let mut ranges = Vec::with_capacity(song.instruments.len());
    let mut cursor = 0usize;
    let mut warning = None;
    for instrument in &song.instruments {
        let want = usize::from(instrument.owned_samples);
        let end = cursor.saturating_add(want).min(song.samples.len());
        if cursor + want > song.samples.len() && warning.is_none() {
            warning = Some(
                "An instrument claimed more samples than the song has. The extra slots were left empty."
                    .to_string(),
            );
        }
        ranges.push((cursor, end));
        cursor = end;
    }
    if cursor < song.samples.len() {
        let extra = song.samples.len() - cursor;
        warning = Some(format!(
            "{extra} samples were not owned by an instrument. They were appended to the last one."
        ));
        if let Some(last) = ranges.last_mut() {
            last.1 = song.samples.len();
        }
    }
    (ranges, warning)
}

fn write_pattern(
    out: &mut Vec<u8>,
    pattern: &Pattern,
    channels: usize,
    index: usize,
    warnings: &mut Vec<String>,
) -> Result<(), Error> {
    let mut rows = pattern.row_count();
    if rows == 0 {
        rows = 1;
        warnings.push(format!(
            "Pattern {index} had no rows. One empty row was written."
        ));
    }
    if rows > MAX_ROWS {
        rows = MAX_ROWS;
        warnings.push(format!(
            "Pattern {index} has more than {MAX_ROWS} rows. XM keeps the first {MAX_ROWS}."
        ));
    }
    let packed = pack_rows(
        &pattern.rows[..rows.min(pattern.row_count())],
        channels,
        index,
        warnings,
    );
    if packed.len() > u16::MAX as usize {
        return Err(Error::Malformed(format!(
            "Pattern {index} does not fit in an XM file. Nothing was written."
        )));
    }
    push_u32(out, 9);
    out.push(0);
    push_u16(out, rows as u16);
    push_u16(out, packed.len() as u16);
    out.extend_from_slice(&packed);
    Ok(())
}

fn pack_rows(
    rows: &[Vec<Cell>],
    channels: usize,
    index: usize,
    warnings: &mut Vec<String>,
) -> Vec<u8> {
    let mut packed = Vec::new();
    let mut clamped = false;
    let mut specials = false;
    let mut any = false;
    for row in rows {
        for channel in 0..channels {
            let cell = row.get(channel).copied().unwrap_or_else(Cell::empty);
            let (stored, flagged) = xm_note(cell.note, &mut clamped, &mut specials);
            let volume = if cell.has_volume { cell.volume } else { 0 };
            if stored == 0
                && cell.instrument == 0
                && volume == 0
                && cell.effect == 0
                && cell.param == 0
            {
                packed.push(0x80);
                continue;
            }
            any = true;
            let mut flags = 0x80u8;
            let mut body = Vec::new();
            if stored != 0 {
                flags |= 0x01;
                body.push(stored);
            }
            if cell.instrument != 0 {
                flags |= 0x02;
                body.push(cell.instrument);
            }
            if volume != 0 {
                flags |= 0x04;
                body.push(volume);
            }
            if cell.effect != 0 || cell.param != 0 {
                flags |= 0x18;
                body.push(cell.effect);
                body.push(cell.param);
            }
            let _ = flagged;
            packed.push(flags);
            packed.extend(body);
        }
    }
    if clamped {
        warnings.push(format!(
            "Pattern {index} has notes above B-7. XM stores C-0 through B-7; those notes were clamped to B-7."
        ));
    }
    if specials {
        warnings.push(format!(
            "Pattern {index} has note cut or note fade. XM stores those as key-off."
        ));
    }
    if any {
        packed
    } else {
        Vec::new()
    }
}

fn xm_note(note: u8, clamped: &mut bool, specials: &mut bool) -> (u8, bool) {
    match note {
        0 => (0, false),
        NOTE_OFF => (97, false),
        NOTE_CUT | NOTE_FADE => {
            *specials = true;
            (97, true)
        }
        1..=96 => (note, false),
        _ => {
            *clamped = true;
            (96, true)
        }
    }
}

fn write_instrument(
    out: &mut Vec<u8>,
    instrument: &Instrument,
    samples: &[Sample],
    base: usize,
    index: usize,
    warnings: &mut Vec<String>,
) -> Result<(), Error> {
    if samples.is_empty() {
        push_u32(out, 29);
        put_text(out, &instrument.name, 22);
        out.push(0);
        push_u16(out, 0);
        return Ok(());
    }
    if samples.len() > 128 {
        return Err(Error::Malformed(format!(
            "XM instrument {} has {} samples; the limit is 128. Nothing was written.",
            index + 1,
            samples.len()
        )));
    }
    let mut header = vec![0u8; 263];
    header[0..4].copy_from_slice(&263u32.to_le_bytes());
    write_text(&mut header[4..26], &instrument.name);
    header[27..29].copy_from_slice(&(samples.len() as u16).to_le_bytes());
    header[29..33].copy_from_slice(&40u32.to_le_bytes());
    for note in 0..96 {
        let global = instrument.keys.get(note).map(|key| key.sample).unwrap_or(0);
        header[33 + note] = local_sample_byte(global, base, samples.len());
    }
    write_xm_envelope(
        &mut header[129..177],
        &instrument.volume_env,
        warnings,
        index,
        "volume",
    );
    write_xm_envelope(
        &mut header[177..225],
        &instrument.pan_env,
        warnings,
        index,
        "panning",
    );
    let vol_count = instrument.volume_env.points.len().min(12) as u8;
    let pan_count = instrument.pan_env.points.len().min(12) as u8;
    header[225] = vol_count;
    header[226] = pan_count;
    header[227] = instrument.volume_env.sustain_point;
    header[228] = instrument.volume_env.loop_start;
    header[229] = instrument.volume_env.loop_end;
    header[230] = instrument.pan_env.sustain_point;
    header[231] = instrument.pan_env.loop_start;
    header[232] = instrument.pan_env.loop_end;
    header[233] = env_flags(&instrument.volume_env, vol_count);
    header[234] = env_flags(&instrument.pan_env, pan_count);
    header[235] = instrument.vibrato.kind;
    header[236] = instrument.vibrato.sweep;
    header[237] = instrument.vibrato.depth;
    header[238] = instrument.vibrato.rate;
    header[239..241].copy_from_slice(&instrument.fadeout.to_le_bytes());
    out.extend_from_slice(&header);

    let mut bodies = Vec::with_capacity(samples.len());
    for sample in samples {
        let (pcm, bits16) = encode_pcm(sample, warnings);
        bodies.push((sample, pcm, bits16));
    }
    for (sample, pcm, bits16) in &bodies {
        write_sample_header(out, sample, pcm.len(), *bits16);
    }
    for (_, pcm, _) in &bodies {
        out.extend_from_slice(pcm);
    }
    Ok(())
}

fn local_sample_byte(global: u16, base: usize, owned: usize) -> u8 {
    if global == 0 || owned == 0 {
        return 0xFF;
    }
    let index = usize::from(global - 1);
    if index < base {
        return 0xFF;
    }
    let local = index - base;
    if local < owned && local < 0xFF {
        local as u8
    } else {
        0xFF
    }
}

fn write_xm_envelope(
    dest: &mut [u8],
    envelope: &Envelope,
    warnings: &mut Vec<String>,
    instrument: usize,
    kind: &str,
) {
    if envelope.points.len() > 12 {
        warnings.push(format!(
            "Instrument {} {kind} envelope has {} nodes. XM keeps 12.",
            instrument + 1,
            envelope.points.len()
        ));
    }
    for (index, (tick, value)) in envelope.points.iter().take(12).enumerate() {
        let at = index * 4;
        dest[at..at + 2].copy_from_slice(&tick.to_le_bytes());
        dest[at + 2..at + 4].copy_from_slice(&u16::from(*value).to_le_bytes());
    }
}

fn env_flags(envelope: &Envelope, count: u8) -> u8 {
    let mut flags = 0u8;
    if envelope.enabled && count > 0 {
        flags |= 1;
    }
    if envelope.sustain && count > 0 {
        flags |= 2;
    }
    if envelope.loop_on && count > 1 {
        flags |= 4;
    }
    flags
}

fn write_sample_header(out: &mut Vec<u8>, sample: &Sample, byte_len: usize, bits16: bool) {
    let bps = if bits16 { 2u32 } else { 1 };
    let frames = if bits16 { byte_len / 2 } else { byte_len };
    let (loop_start, loop_len, flags_loop) = xm_loop(sample, frames, bps);
    let mut flags = flags_loop;
    if bits16 {
        flags |= 0x10;
    }
    push_u32(out, byte_len as u32);
    push_u32(out, loop_start);
    push_u32(out, loop_len);
    out.push(sample.volume.min(64));
    out.push(sample.finetune as u8);
    out.push(flags);
    out.push(sample.pan.unwrap_or(0x80));
    out.push(sample.relative_note as u8);
    out.push(0);
    put_text(out, &sample.name, 22);
}

fn xm_loop(sample: &Sample, frames: usize, bps: u32) -> (u32, u32, u8) {
    if sample.loop_kind == LoopKind::None || sample.loop_end <= sample.loop_start {
        return (0, 0, 0);
    }
    let start = usize::try_from(sample.loop_start)
        .unwrap_or(usize::MAX)
        .min(frames);
    let end = usize::try_from(sample.loop_end)
        .unwrap_or(usize::MAX)
        .min(frames);
    if end <= start {
        return (0, 0, 0);
    }
    let flag = if sample.loop_kind == LoopKind::PingPong {
        2
    } else {
        1
    };
    (start as u32 * bps, (end - start) as u32 * bps, flag)
}

/// Delta-coded sample bytes, and whether they are 16-bit.
fn encode_pcm(sample: &Sample, warnings: &mut Vec<String>) -> (Vec<u8>, bool) {
    let clean8 = sample.pcm.iter().all(|frame| frame & 0x00FF == 0);
    let bits16 = sample.bits == 16 || !clean8;
    if sample.bits != 16 && !clean8 {
        warnings.push(format!(
            "Sample \"{}\" has 16-bit data and was stored as 16-bit.",
            sample.name
        ));
    }
    if bits16 {
        let mut acc = 0i16;
        let mut out = Vec::with_capacity(sample.pcm.len() * 2);
        for frame in &sample.pcm {
            let delta = frame.wrapping_sub(acc);
            acc = *frame;
            out.extend_from_slice(&delta.to_le_bytes());
        }
        (out, true)
    } else {
        let mut acc = 0i8;
        let mut out = Vec::with_capacity(sample.pcm.len());
        for frame in &sample.pcm {
            let value = (*frame >> 8) as i8;
            let delta = value.wrapping_sub(acc);
            acc = value;
            out.push(delta as u8);
        }
        (out, false)
    }
}

fn put_text(out: &mut Vec<u8>, text: &str, len: usize) {
    let mut bytes = vec![0u8; len];
    write_text(&mut bytes, text);
    out.extend_from_slice(&bytes);
}

fn write_text(dest: &mut [u8], text: &str) {
    for (slot, byte) in dest.iter_mut().zip(text_bytes(text)) {
        *slot = byte;
    }
}

fn text_bytes(text: &str) -> Vec<u8> {
    text.chars()
        .filter_map(|ch| {
            let value = u32::from(ch);
            if (0x20..0x7F).contains(&value) || (0xA0..=0xFF).contains(&value) {
                Some(value as u8)
            } else {
                None
            }
        })
        .collect()
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
