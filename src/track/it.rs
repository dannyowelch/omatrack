//! Impulse Tracker `.it` reader, including IT214 / IT215 compressed samples.
//!
//! The magic is `IMPM`. Truncated and corrupt files return [`Error`](crate::Error).

use super::song::{
    Cell, Envelope, Format, Instrument, Key, LoopKind, NewNoteAction, Pattern, Sample, Song,
    Vibrato, NOTE_CUT, NOTE_FADE, NOTE_OFF,
};
use crate::error::Error;

const MAX_CHANNELS: usize = 64;
const MAX_PATTERNS: usize = 256;
const MAX_INSTRUMENTS: usize = 256;
const MAX_SAMPLES: usize = 256;
const MAX_ROWS: usize = 1024;
const MAX_SAMPLE_FRAMES: usize = 8_000_000;

/// `true` when `bytes` starts with `IMPM`.
pub fn is_it(bytes: &[u8]) -> bool {
    bytes.starts_with(b"IMPM")
}

/// Parse an IT module.
pub fn parse(bytes: &[u8]) -> Result<Song, Error> {
    if !is_it(bytes) {
        return Err(Error::Malformed(
            "IT file does not start with \"IMPM\"".to_string(),
        ));
    }
    if bytes.len() < 0xC0 {
        return Err(truncated("IT header", 0xC0, bytes.len()));
    }
    let title = latin1(&bytes[4..30]);
    let ordnum = u16_at(bytes, 0x20) as usize;
    let insnum = u16_at(bytes, 0x22) as usize;
    let smpnum = u16_at(bytes, 0x24) as usize;
    let patnum = u16_at(bytes, 0x26) as usize;
    let cmwt = u16_at(bytes, 0x2A);
    let flags = u16_at(bytes, 0x2C);
    let global_vol = bytes[0x30];
    let _mix_vol = bytes[0x31];
    let speed = bytes[0x32];
    let tempo = bytes[0x33];
    if ordnum > 256 {
        return Err(Error::Malformed(format!(
            "IT order count {ordnum} is above 256"
        )));
    }
    if insnum > MAX_INSTRUMENTS || smpnum > MAX_SAMPLES || patnum > MAX_PATTERNS {
        return Err(Error::Malformed(format!(
            "IT counts (instruments {insnum}, samples {smpnum}, patterns {patnum}) exceed the supported limits"
        )));
    }
    let mut pos = 0xC0;
    let orders = read_slice(bytes, &mut pos, ordnum, "IT order list")?.to_vec();
    let ins_off = read_offsets(bytes, &mut pos, insnum, "IT instrument offsets")?;
    let smp_off = read_offsets(bytes, &mut pos, smpnum, "IT sample offsets")?;
    let pat_off = read_offsets(bytes, &mut pos, patnum, "IT pattern offsets")?;

    let mut initial_pan = Vec::with_capacity(MAX_CHANNELS);
    let mut initial_mute = Vec::with_capacity(MAX_CHANNELS);
    let mut initial_channel_volume = Vec::with_capacity(MAX_CHANNELS);
    for index in 0..MAX_CHANNELS {
        let pan = bytes[0x40 + index];
        let muted = pan & 0x80 != 0;
        let pan = pan & 0x7F;
        let pan = if pan == 100 {
            128
        } else {
            (u16::from(pan.min(64)) * 255 / 64) as u8
        };
        initial_pan.push(pan);
        initial_mute.push(muted);
        let vol = bytes[0x80 + index].min(64);
        initial_channel_volume.push(vol);
    }

    let mut samples = Vec::with_capacity(smpnum);
    for (index, offset) in smp_off.iter().copied().enumerate() {
        samples.push(parse_sample(bytes, offset, index, cmwt)?);
    }
    let instrument_mode = flags & 0x04 != 0;
    let mut instruments = Vec::with_capacity(insnum);
    if instrument_mode {
        for (index, offset) in ins_off.iter().copied().enumerate() {
            instruments.push(parse_instrument(bytes, offset, index, cmwt)?);
        }
    } else {
        for (index, sample) in samples.iter().enumerate() {
            let mut instrument = Instrument {
                name: sample.name.clone(),
                global_volume: 128,
                ..Instrument::default()
            };
            instrument.keys = (0..120)
                .map(|note| Key {
                    note: note as u8,
                    sample: (index as u16).saturating_add(1),
                })
                .collect();
            instruments.push(instrument);
        }
    }

    let mut patterns = Vec::with_capacity(patnum);
    for (index, offset) in pat_off.iter().copied().enumerate() {
        patterns.push(parse_pattern(bytes, offset, index)?);
    }

    let linear = flags & 0x08 != 0;
    let channels = channel_span(&patterns, &initial_mute);
    let initial_pan: Vec<u8> = initial_pan.into_iter().take(channels).collect();
    let initial_mute: Vec<bool> = initial_mute.into_iter().take(channels).collect();
    let initial_channel_volume: Vec<u8> =
        initial_channel_volume.into_iter().take(channels).collect();
    Ok(Song {
        title,
        tracker: format!("IT {cmwt:#06x}"),
        format: Format::It,
        channels,
        orders,
        restart: 0,
        patterns,
        instruments,
        samples,
        linear,
        initial_speed: speed.max(1),
        initial_tempo: tempo.max(32),
        initial_global_volume: u16::from(global_vol.min(128)),
        global_volume_max: 128,
        initial_pan,
        initial_channel_volume,
        initial_mute,
        instrument_mode: true,
        old_effects: flags & 0x10 != 0,
        compatible_gxx: flags & 0x20 != 0,
    })
}

fn parse_pattern(bytes: &[u8], offset: u32, index: usize) -> Result<Pattern, Error> {
    if offset == 0 {
        return Ok(Pattern::empty(64, MAX_CHANNELS));
    }
    let at = offset as usize;
    if at + 8 > bytes.len() {
        return Err(truncated("IT pattern header", at + 8, bytes.len()));
    }
    // `length` is the packed payload. The 8-byte header (length, rows, reserved)
    // sits in front of it and is not included in the count.
    let length = u16_at(bytes, at) as usize;
    let rows = u16_at(bytes, at + 2) as usize;
    if !(1..=MAX_ROWS).contains(&rows) {
        return Err(Error::Malformed(format!(
            "IT pattern {index} has {rows} rows; expected 1..={MAX_ROWS}"
        )));
    }
    let data_end = at + 8 + length;
    if data_end > bytes.len() {
        return Err(truncated("IT pattern", data_end, bytes.len()));
    }
    let packed = &bytes[at + 8..data_end];
    let mut pattern = Pattern::empty(rows, MAX_CHANNELS);
    let mut masks = [0u8; MAX_CHANNELS];
    let mut last_note = [0u8; MAX_CHANNELS];
    let mut last_ins = [0u8; MAX_CHANNELS];
    let mut last_vol = [0u8; MAX_CHANNELS];
    let mut last_effect = [0u8; MAX_CHANNELS];
    let mut last_param = [0u8; MAX_CHANNELS];
    let mut p = 0usize;
    for row in 0..rows {
        loop {
            if p >= packed.len() {
                break;
            }
            let channel_byte = packed[p];
            p += 1;
            if channel_byte == 0 {
                break;
            }
            let channel = usize::from((channel_byte - 1) & 0x3F);
            if channel_byte & 0x80 != 0 {
                if p >= packed.len() {
                    return Err(Error::Malformed(format!(
                        "IT pattern {index} row {row} ended on a channel mask"
                    )));
                }
                masks[channel] = packed[p];
                p += 1;
            }
            let mask = masks[channel];
            let note = if mask & 0x01 != 0 {
                last_note[channel] = take(packed, &mut p, "IT note")?;
                last_note[channel]
            } else if mask & 0x10 != 0 {
                last_note[channel]
            } else {
                0
            };
            let instrument = if mask & 0x02 != 0 {
                last_ins[channel] = take(packed, &mut p, "IT instrument")?;
                last_ins[channel]
            } else if mask & 0x20 != 0 {
                last_ins[channel]
            } else {
                0
            };
            let volume = if mask & 0x04 != 0 {
                last_vol[channel] = take(packed, &mut p, "IT volume")?;
                last_vol[channel]
            } else if mask & 0x40 != 0 {
                last_vol[channel]
            } else {
                0
            };
            let (effect, param) = if mask & 0x08 != 0 {
                last_effect[channel] = take(packed, &mut p, "IT effect")?;
                last_param[channel] = take(packed, &mut p, "IT parameter")?;
                (last_effect[channel], last_param[channel])
            } else if mask & 0x80 != 0 {
                (last_effect[channel], last_param[channel])
            } else {
                (0, 0)
            };
            let note_present = mask & 0x01 != 0 || mask & 0x10 != 0;
            let volume_present = mask & 0x04 != 0 || mask & 0x40 != 0;
            pattern.rows[row][channel] = Cell {
                note: if note_present { convert_note(note)? } else { 0 },
                instrument,
                volume,
                effect,
                param,
                has_volume: volume_present,
            };
        }
    }
    Ok(pattern)
}

fn convert_note(note: u8) -> Result<u8, Error> {
    match note {
        0..=119 => Ok(note.saturating_add(1)),
        254 => Ok(NOTE_CUT),
        255 => Ok(NOTE_OFF),
        253 | 246 => Ok(NOTE_FADE),
        other => Err(Error::Malformed(format!(
            "IT note {other} is not a tone, note-off, note-cut, or note-fade"
        ))),
    }
}

fn parse_instrument(
    bytes: &[u8],
    offset: u32,
    index: usize,
    cmwt: u16,
) -> Result<Instrument, Error> {
    if offset == 0 {
        return Ok(Instrument::default());
    }
    let at = offset as usize;
    if at + 4 > bytes.len() {
        return Err(truncated("IT instrument", at + 4, bytes.len()));
    }
    if &bytes[at..at + 4] != b"IMPI" {
        return Err(Error::Malformed(format!(
            "IT instrument {} does not start with \"IMPI\"",
            index + 1
        )));
    }
    // Old instrument format (compatible with < 2.00) is a different layout.
    if cmwt < 0x200 {
        return parse_old_instrument(bytes, at, index);
    }
    if at + 0x40 > bytes.len() {
        return Err(truncated("IT instrument", at + 0x40, bytes.len()));
    }
    let name = latin1(&bytes[at + 0x20..at + 0x3A]);
    let nna = match bytes[at + 0x11] {
        1 => NewNoteAction::Continue,
        2 => NewNoteAction::Off,
        3 => NewNoteAction::Fade,
        _ => NewNoteAction::Cut,
    };
    let fadeout = u16_at(bytes, at + 0x14);
    let dfp = bytes[at + 0x19];
    let pan = if dfp & 0x80 == 0 {
        Some((u16::from(dfp.min(64)) * 255 / 64) as u8)
    } else {
        None
    };
    let gbv = bytes[at + 0x18];
    if at + 0x130 > bytes.len() {
        return Err(truncated("IT instrument keyboard", at + 0x130, bytes.len()));
    }
    let mut keys = Vec::with_capacity(120);
    for note in 0..120 {
        let pair = at + 0x40 + note * 2;
        let mapped = bytes[pair];
        let sample = bytes[pair + 1];
        keys.push(Key {
            note: mapped.min(119),
            sample: u16::from(sample),
        });
    }
    let vol = read_it_envelope(bytes, at + 0x130)?;
    let pan_env = read_it_envelope(bytes, at + 0x182)?;
    let mut pitch = read_it_envelope(bytes, at + 0x1D4)?;
    // Bit 7 of the pitch envelope flags means "this is a filter envelope".
    if pitch_is_filter(bytes, at + 0x1D4) {
        pitch.enabled = false;
    }
    Ok(Instrument {
        name,
        keys,
        volume_env: vol,
        pan_env,
        pitch_env: pitch,
        fadeout,
        nna,
        dct: bytes[at + 0x12],
        dca: bytes[at + 0x13],
        global_volume: if gbv == 0 { 128 } else { gbv.min(128) },
        pan,
        ..Instrument::default()
    })
}

fn parse_old_instrument(bytes: &[u8], at: usize, index: usize) -> Result<Instrument, Error> {
    if at + 0x30 > bytes.len() {
        return Err(truncated("IT old instrument", at + 0x30, bytes.len()));
    }
    let name = latin1(&bytes[at + 0x14..at + 0x2E.min(bytes.len())]);
    let _ = index;
    let mut keys = vec![Key::default(); 120];
    if at + 0x30 + 240 <= bytes.len() {
        for (note, key) in keys.iter_mut().enumerate() {
            let pair = at + 0x30 + note * 2;
            *key = Key {
                note: bytes[pair].min(119),
                sample: u16::from(bytes[pair + 1]),
            };
        }
    }
    Ok(Instrument {
        name,
        keys,
        ..Instrument::default()
    })
}

fn pitch_is_filter(bytes: &[u8], at: usize) -> bool {
    at < bytes.len() && bytes[at] & 0x80 != 0
}

/// Envelope is 81 bytes: 6 header bytes and 25 nodes of (value, tick).
fn read_it_envelope(bytes: &[u8], at: usize) -> Result<Envelope, Error> {
    if at + 6 > bytes.len() {
        return Ok(Envelope::default());
    }
    let flags = bytes[at];
    let count = usize::from(bytes[at + 1]).min(25);
    let loop_start = bytes[at + 2];
    let loop_end = bytes[at + 3];
    let sus_start = bytes[at + 4];
    let sus_end = bytes[at + 5];
    let mut points = Vec::with_capacity(count);
    for node in 0..count {
        let node_at = at + 6 + node * 3;
        if node_at + 3 > bytes.len() {
            return Err(truncated("IT envelope node", node_at + 3, bytes.len()));
        }
        let y = bytes[node_at];
        let tick = u16::from_le_bytes([bytes[node_at + 1], bytes[node_at + 2]]);
        points.push((tick, y.min(64)));
    }
    let last = count.saturating_sub(1) as u8;
    Ok(Envelope {
        enabled: flags & 0x01 != 0 && count > 0,
        loop_on: flags & 0x02 != 0 && count > 1,
        sustain: flags & 0x04 != 0 && count > 0,
        loop_start: loop_start.min(last),
        loop_end: loop_end.min(last),
        sustain_point: sus_start.min(last),
        sustain_end: sus_end.min(last),
        points,
    })
}

fn parse_sample(bytes: &[u8], offset: u32, index: usize, cmwt: u16) -> Result<Sample, Error> {
    if offset == 0 {
        return Ok(Sample::default());
    }
    let at = offset as usize;
    if at + 0x50 > bytes.len() {
        return Err(truncated("IT sample header", at + 0x50, bytes.len()));
    }
    if &bytes[at..at + 4] != b"IMPS" {
        return Err(Error::Malformed(format!(
            "IT sample {} does not start with \"IMPS\"",
            index + 1
        )));
    }
    let name = latin1(&bytes[at + 0x14..at + 0x2E]);
    let gvl = bytes[at + 0x11].min(64);
    let flags = bytes[at + 0x12];
    let volume = bytes[at + 0x13].min(64);
    let cvt = bytes[at + 0x2E];
    let dfp = bytes[at + 0x2F];
    let length = u32_at(bytes, at + 0x30) as usize;
    let loop_begin = u32_at(bytes, at + 0x34);
    let loop_end = u32_at(bytes, at + 0x38);
    let c5 = u32_at(bytes, at + 0x3C);
    let sus_begin = u32_at(bytes, at + 0x40);
    let sus_end = u32_at(bytes, at + 0x44);
    let pointer = u32_at(bytes, at + 0x48) as usize;
    let vibrato = Vibrato {
        rate: bytes[at + 0x4C],
        depth: bytes[at + 0x4D],
        sweep: bytes[at + 0x4E],
        kind: bytes[at + 0x4F],
    };
    let has_data = flags & 0x01 != 0 && length > 0 && pointer > 0;
    if length > MAX_SAMPLE_FRAMES {
        return Err(Error::Malformed(format!(
            "IT sample {} (\"{name}\") is {length} frames; the limit is {MAX_SAMPLE_FRAMES}",
            index + 1
        )));
    }
    let bits16 = flags & 0x02 != 0;
    let stereo = flags & 0x04 != 0;
    let compressed = flags & 0x08 != 0;
    let pan = if dfp & 0x80 != 0 {
        Some((u16::from(dfp & 0x7F).min(64) * 255 / 64) as u8)
    } else {
        None
    };
    let pcm = if !has_data {
        Vec::new()
    } else if compressed && stereo {
        return Err(Error::Unsupported(format!(
            "IT sample {} (\"{name}\") is stereo and compressed; stereo IT214 samples are not supported yet",
            index + 1
        )));
    } else if compressed {
        let it215 = cmwt >= 0x215;
        decompress_sample(bytes, pointer, length, bits16, it215, index, &name)?
    } else {
        decode_raw(bytes, pointer, length, bits16, stereo, cvt, index, &name)?
    };
    let (loop_start, loop_end, loop_kind) = loop_pair(
        flags & 0x10 != 0,
        flags & 0x40 != 0,
        loop_begin,
        loop_end,
        pcm.len(),
    );
    let (sustain_start, sustain_end, sustain_kind) = loop_pair(
        flags & 0x20 != 0,
        flags & 0x80 != 0,
        sus_begin,
        sus_end,
        pcm.len(),
    );
    Ok(Sample {
        name,
        pcm,
        bits: if bits16 { 16 } else { 8 },
        volume,
        global_volume: if gvl == 0 { 64 } else { gvl },
        pan,
        finetune: 0,
        relative_note: 0,
        c5_speed: if c5 == 0 { 8363 } else { c5 },
        loop_start,
        loop_end,
        loop_kind,
        sustain_start,
        sustain_end,
        sustain_kind,
        vibrato,
    })
}

fn loop_pair(on: bool, ping: bool, start: u32, end: u32, len: usize) -> (u32, u32, LoopKind) {
    if !on || end <= start {
        return (0, 0, LoopKind::None);
    }
    let start = start.min(len as u32);
    let end = end.min(len as u32);
    if end <= start {
        return (0, 0, LoopKind::None);
    }
    let kind = if ping {
        LoopKind::PingPong
    } else {
        LoopKind::Forward
    };
    (start, end, kind)
}

#[allow(clippy::too_many_arguments)]
fn decode_raw(
    bytes: &[u8],
    pointer: usize,
    length: usize,
    bits16: bool,
    stereo: bool,
    cvt: u8,
    index: usize,
    name: &str,
) -> Result<Vec<i16>, Error> {
    let channels = if stereo { 2 } else { 1 };
    let bytes_per = if bits16 { 2 } else { 1 };
    let need = length.saturating_mul(bytes_per).saturating_mul(channels);
    if pointer.saturating_add(need) > bytes.len() {
        return Err(truncated(
            "IT sample data",
            pointer.saturating_add(need),
            bytes.len(),
        ));
    }
    let raw = &bytes[pointer..pointer + need];
    let signed = cvt & 0x01 != 0;
    let little = cvt & 0x02 != 0 || !bits16;
    let delta = cvt & 0x04 != 0;
    let mut pcm = Vec::with_capacity(length);
    if bits16 {
        let mut acc_l: i16 = 0;
        let mut acc_r: i16 = 0;
        for frame in 0..length {
            let l = read_i16(raw, frame * channels * 2, little)?;
            let r = if stereo {
                read_i16(raw, frame * channels * 2 + 2, little)?
            } else {
                l
            };
            let (l, r) = if signed {
                (l, r)
            } else {
                (l.wrapping_add(i16::MIN), r.wrapping_add(i16::MIN))
            };
            let (l, r) = if delta {
                acc_l = acc_l.wrapping_add(l);
                acc_r = acc_r.wrapping_add(r);
                (acc_l, acc_r)
            } else {
                (l, r)
            };
            pcm.push(if stereo { l.wrapping_add(r) / 2 } else { l });
        }
    } else {
        let mut acc_l: i16 = 0;
        let mut acc_r: i16 = 0;
        for frame in 0..length {
            let l = raw[frame * channels] as i8;
            let r = if stereo {
                raw[frame * channels + 1] as i8
            } else {
                l
            };
            let widen = |v: i8| {
                let v = if signed { v } else { v.wrapping_add(i8::MIN) };
                i16::from(v).wrapping_shl(8)
            };
            let (mut l, mut r) = (widen(l), widen(r));
            if delta {
                acc_l = acc_l.wrapping_add(l);
                acc_r = acc_r.wrapping_add(r);
                l = acc_l;
                r = acc_r;
            }
            pcm.push(if stereo { l.wrapping_add(r) / 2 } else { l });
        }
    }
    let _ = (index, name);
    Ok(pcm)
}

fn read_i16(raw: &[u8], at: usize, little: bool) -> Result<i16, Error> {
    if at + 2 > raw.len() {
        return Err(Error::Malformed(
            "IT 16-bit sample ended before a full frame".to_string(),
        ));
    }
    let value = if little {
        i16::from_le_bytes([raw[at], raw[at + 1]])
    } else {
        i16::from_be_bytes([raw[at], raw[at + 1]])
    };
    Ok(value)
}

fn decompress_sample(
    bytes: &[u8],
    pointer: usize,
    length: usize,
    bits16: bool,
    it215: bool,
    index: usize,
    name: &str,
) -> Result<Vec<i16>, Error> {
    if pointer >= bytes.len() {
        return Err(truncated("IT compressed sample", pointer + 2, bytes.len()));
    }
    let src = &bytes[pointer..];
    let decoded = if bits16 {
        decompress16(src, length, it215)
    } else {
        decompress8(src, length, it215)
    }
    .map_err(|err| {
        Error::Malformed(format!(
            "IT sample {} (\"{name}\") could not be decompressed: {err}",
            index + 1
        ))
    })?;
    Ok(if bits16 {
        decoded
    } else {
        decoded.into_iter().map(|s| s.wrapping_shl(8)).collect()
    })
}

/// IT214/IT215 8-bit decompression. Output is signed 8-bit stored in `i16`.
fn decompress8(src: &[u8], length: usize, it215: bool) -> Result<Vec<i16>, String> {
    let mut out = Vec::with_capacity(length);
    let mut pos = 0usize;
    while out.len() < length {
        if pos + 2 > src.len() {
            return Err("compressed block header is truncated".to_string());
        }
        let block_len = u16::from_le_bytes([src[pos], src[pos + 1]]) as usize;
        pos += 2;
        if pos + block_len > src.len() {
            return Err(format!(
                "compressed block needs {block_len} bytes, {} remain",
                src.len() - pos
            ));
        }
        let block = &src[pos..pos + block_len];
        pos += block_len;
        let want = (length - out.len()).min(0x8000);
        unpack_block(block, want, false, it215, &mut out)?;
    }
    out.truncate(length);
    Ok(out)
}

fn decompress16(src: &[u8], length: usize, it215: bool) -> Result<Vec<i16>, String> {
    let mut out = Vec::with_capacity(length);
    let mut pos = 0usize;
    while out.len() < length {
        if pos + 2 > src.len() {
            return Err("compressed block header is truncated".to_string());
        }
        let block_len = u16::from_le_bytes([src[pos], src[pos + 1]]) as usize;
        pos += 2;
        if pos + block_len > src.len() {
            return Err(format!(
                "compressed block needs {block_len} bytes, {} remain",
                src.len() - pos
            ));
        }
        let block = &src[pos..pos + block_len];
        pos += block_len;
        let want = (length - out.len()).min(0x4000);
        unpack_block(block, want, true, it215, &mut out)?;
    }
    out.truncate(length);
    Ok(out)
}

fn unpack_block(
    block: &[u8],
    count: usize,
    bits16: bool,
    it215: bool,
    out: &mut Vec<i16>,
) -> Result<(), String> {
    let mut bits = BitReader::new(block);
    let mut width: u32 = if bits16 { 17 } else { 9 };
    let mut d1: i32 = 0;
    let mut d2: i32 = 0;
    let mut produced = 0usize;
    while produced < count {
        if width == 0 || width > if bits16 { 17 } else { 9 } {
            return Err(format!("illegal bit width {width}"));
        }
        let value = bits.read(width)?;
        if !bits16 && width < 7 && value == (1 << (width - 1)) {
            let new_w = bits.read(3)? + 1;
            width = if new_w < width { new_w } else { new_w + 1 };
            continue;
        } else if bits16 && width < 7 && value == (1 << (width - 1)) {
            let new_w = bits.read(4)? + 1;
            width = if new_w < width { new_w } else { new_w + 1 };
            continue;
        } else if !bits16 && (7..9).contains(&width) {
            let border = (0xFFu32 >> (9 - width)) - 4;
            if value > border && value <= border + 8 {
                let new_w = value - border;
                width = if new_w < width { new_w } else { new_w + 1 };
                continue;
            }
        } else if bits16 && (7..17).contains(&width) {
            let border = (0xFFFFu32 >> (17 - width)) - 8;
            if value > border && value <= border + 16 {
                let new_w = value - border;
                width = if new_w < width { new_w } else { new_w + 1 };
                continue;
            }
        } else if (!bits16 && width == 9 && value & 0x100 != 0)
            || (bits16 && width == 17 && value & 0x1_0000 != 0)
        {
            width = (value + 1) & 0xFF;
            continue;
        }
        let sample = if bits16 {
            sign_extend(value, width, 16)
        } else {
            sign_extend(value, width, 8)
        };
        d1 = d1.wrapping_add(sample);
        d2 = d2.wrapping_add(d1);
        let mixed = if it215 { d2 } else { d1 };
        let stored = if bits16 {
            mixed as i16
        } else {
            (mixed as i8) as i16
        };
        out.push(stored);
        produced += 1;
    }
    Ok(())
}

fn sign_extend(value: u32, width: u32, bits: u32) -> i32 {
    if width == 0 || width >= bits {
        return if bits >= 16 {
            value as i16 as i32
        } else {
            value as i8 as i32
        };
    }
    let shift = 32 - width;
    ((value << shift) as i32) >> shift
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    bits: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            buf: 0,
            bits: 0,
        }
    }

    /// Read `width` bits. The first bit in the file is the high bit of the result,
    /// taken from the low bit of each source byte.
    fn read(&mut self, width: u32) -> Result<u32, String> {
        if width == 0 || width > 24 {
            return Err(format!("cannot read {width} bits"));
        }
        let mut value = 0u32;
        for _ in 0..width {
            if self.bits == 0 {
                if self.pos >= self.data.len() {
                    return Err("ran out of compressed bits".to_string());
                }
                self.buf = u32::from(self.data[self.pos]);
                self.pos += 1;
                self.bits = 8;
            }
            value >>= 1;
            if self.buf & 1 != 0 {
                value |= 1 << 31;
            }
            self.buf >>= 1;
            self.bits -= 1;
        }
        Ok(value >> (32 - width))
    }
}

fn take(packed: &[u8], p: &mut usize, ctx: &str) -> Result<u8, Error> {
    if *p >= packed.len() {
        return Err(Error::Malformed(format!(
            "{ctx} ended early in an IT pattern"
        )));
    }
    let byte = packed[*p];
    *p += 1;
    Ok(byte)
}

fn read_slice<'a>(
    bytes: &'a [u8],
    pos: &mut usize,
    len: usize,
    ctx: &'static str,
) -> Result<&'a [u8], Error> {
    let end = pos.saturating_add(len);
    if end > bytes.len() {
        return Err(truncated(ctx, end, bytes.len()));
    }
    let slice = &bytes[*pos..end];
    *pos = end;
    Ok(slice)
}

fn read_offsets(
    bytes: &[u8],
    pos: &mut usize,
    count: usize,
    ctx: &'static str,
) -> Result<Vec<u32>, Error> {
    let raw = read_slice(bytes, pos, count * 4, ctx)?;
    Ok(raw
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn truncated(context: &'static str, expected: usize, actual: usize) -> Error {
    Error::Truncated {
        context,
        expected,
        actual,
    }
}

fn latin1(bytes: &[u8]) -> String {
    // Interior NULs show up in IT sample names (a short name padded on the
    // left). They must not reach the terminal: a NUL is not a column, so the
    // rest of the row slides left and eats the pane border.
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

/// IT channel count is fixed at 64 in the header. Songs still only *use* the
/// channels that appear. [`used_channels`] reports the highest channel with a
/// cell, pan, or volume, at least 1 and at most 64.
fn channel_span(patterns: &[Pattern], muted: &[bool]) -> usize {
    let mut highest = 1usize;
    for pattern in patterns {
        for row in &pattern.rows {
            for (channel, cell) in row.iter().enumerate() {
                if cell.note != 0
                    || cell.instrument != 0
                    || cell.has_volume
                    || cell.effect != 0
                    || cell.param != 0
                {
                    highest = highest.max(channel + 1);
                }
            }
        }
    }
    for (channel, mute) in muted.iter().enumerate() {
        if *mute {
            highest = highest.max(channel + 1);
        }
    }
    highest.clamp(1, MAX_CHANNELS)
}
