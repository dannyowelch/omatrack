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
    let mut stereo_notes = Vec::new();
    for (index, offset) in smp_off.iter().copied().enumerate() {
        let (sample, stereo) = parse_sample(bytes, offset, index, cmwt)?;
        if stereo {
            stereo_notes.push(format!(
                "Sample {} (\"{}\") was stereo and was mixed to mono. Saving writes the mono sample.",
                index + 1,
                sample.name
            ));
        }
        samples.push(sample);
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
    let mut load_notes = Vec::new();
    if flags & 0xC0 != 0 {
        load_notes.push(
            "MIDI pitch and embedded MIDI configuration were not kept and will not be written back."
                .to_string(),
        );
    }
    let special = u16_at(bytes, 0x2E);
    if special & 0x01 != 0 {
        load_notes.push("The song message was not kept and will not be written back.".to_string());
    }
    if (channels..MAX_CHANNELS).any(|index| {
        let pan = bytes[0x40 + index];
        let vol = bytes[0x80 + index];
        let raw = pan & 0x7F;
        pan & 0x80 != 0 || vol != 64 || (raw != 32 && raw != 0)
    }) {
        load_notes.push(format!(
            "Channel pan, mute, and volume after channel {channels} were not kept."
        ));
    }
    let initial_pan: Vec<u8> = initial_pan.into_iter().take(channels).collect();
    let initial_mute: Vec<bool> = initial_mute.into_iter().take(channels).collect();
    let initial_channel_volume: Vec<u8> =
        initial_channel_volume.into_iter().take(channels).collect();
    load_notes.extend(stereo_notes);
    if instruments
        .iter()
        .any(|instrument| instrument.pitch_env.filter)
    {
        load_notes.push("A filter envelope was kept in the file and is not played.".to_string());
    }
    if !instrument_mode {
        load_notes.push(
            "Sample mode was loaded as instruments (one per sample) and is saved that way."
                .to_string(),
        );
    }
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
        compat: cmwt,
        load_notes: super::song::LoadNotes { lines: load_notes },
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
    // Bit 7 means the nodes are a filter envelope. Keep them, and keep the
    // flag, but playback will not treat the envelope as pitch.
    if pitch_is_filter(bytes, at + 0x1D4) {
        pitch.filter = true;
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
        filter: false,
    })
}

fn parse_sample(
    bytes: &[u8],
    offset: u32,
    index: usize,
    cmwt: u16,
) -> Result<(Sample, bool), Error> {
    if offset == 0 {
        return Ok((Sample::default(), false));
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
    let mixed_stereo = stereo && has_data && !compressed;
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
    let sample = Sample {
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
    };
    Ok((sample, mixed_stereo))
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

/// Encode `song` as an Impulse Tracker module.
///
/// Samples are stored uncompressed. A reload matches what [`parse`] kept,
/// including envelopes and the filter-envelope flag. `warnings` names fields
/// the format cannot store.
pub(crate) fn write(song: &Song) -> Result<(Vec<u8>, Vec<String>), Error> {
    let mut warnings = Vec::new();
    if song.patterns.len() > MAX_PATTERNS
        || song.instruments.len() > MAX_INSTRUMENTS
        || song.samples.len() > MAX_SAMPLES
    {
        return Err(Error::Malformed(format!(
            "IT stores at most {MAX_PATTERNS} patterns, {MAX_INSTRUMENTS} instruments, and {MAX_SAMPLES} samples. Nothing was written."
        )));
    }
    let mut orders = song.orders.clone();
    if orders.is_empty() {
        orders.push(0);
        warnings.push(
            "The order list was empty. One order pointing at pattern 0 was written.".to_string(),
        );
    }
    if orders.len() > 256 {
        return Err(Error::Malformed(
            "IT stores at most 256 orders. Nothing was written.".to_string(),
        ));
    }
    if song.restart != 0 {
        warnings.push("Restart position is not part of an IT file and was left out.".to_string());
    }
    let cmwt = if song.compat == 0 {
        0x0214
    } else {
        song.compat
    };

    let ordnum = orders.len();
    let insnum = song.instruments.len();
    let smpnum = song.samples.len();
    let patnum = song.patterns.len();
    let header_len = 0xC0 + ordnum + (insnum + smpnum + patnum) * 4;
    let mut out = vec![0u8; header_len];
    out[0..4].copy_from_slice(b"IMPM");
    write_text(&mut out[4..30], &song.title);
    put_u16(&mut out, 0x20, ordnum as u16);
    put_u16(&mut out, 0x22, insnum as u16);
    put_u16(&mut out, 0x24, smpnum as u16);
    put_u16(&mut out, 0x26, patnum as u16);
    put_u16(&mut out, 0x28, cmwt);
    put_u16(&mut out, 0x2A, cmwt);
    put_u16(&mut out, 0x2C, it_flags(song));
    out[0x30] = u8::try_from(song.initial_global_volume.min(128)).unwrap_or(128);
    out[0x31] = 48;
    out[0x32] = song.initial_speed.max(1);
    out[0x33] = song.initial_tempo.max(32);
    out[0x34] = 128;
    let mut pan_quantized = false;
    for channel in 0..MAX_CHANNELS {
        let (pan, exact) = if channel < song.initial_pan.len() {
            let mute = song.initial_mute.get(channel).copied().unwrap_or(false);
            channel_pan_byte(song.initial_pan[channel], mute)
        } else {
            (32, true)
        };
        if !exact {
            pan_quantized = true;
        }
        out[0x40 + channel] = pan;
        let volume = if channel < song.initial_channel_volume.len() {
            song.initial_channel_volume[channel].min(64)
        } else {
            64
        };
        out[0x80 + channel] = volume;
    }
    if pan_quantized {
        warnings.push(
            "Channel panning was quantized to the IT scale (0..=64, or surround).".to_string(),
        );
    }
    out[0xC0..0xC0 + ordnum].copy_from_slice(&orders);

    let ins_table = 0xC0 + ordnum;
    let smp_table = ins_table + insnum * 4;
    let pat_table = smp_table + smpnum * 4;

    for (index, instrument) in song.instruments.iter().enumerate() {
        if instrument == &Instrument::default() {
            continue;
        }
        let blob = instrument_bytes(instrument, cmwt, index, &mut warnings);
        let at = out.len() as u32;
        put_u32(&mut out, ins_table + index * 4, at);
        out.extend_from_slice(&blob);
    }
    for (index, sample) in song.samples.iter().enumerate() {
        if sample == &Sample::default() {
            continue;
        }
        let offset = out.len() as u32;
        let (header, data) = sample_bytes(sample, offset, &mut warnings);
        put_u32(&mut out, smp_table + index * 4, offset);
        out.extend_from_slice(&header);
        out.extend_from_slice(&data);
    }
    for (index, pattern) in song.patterns.iter().enumerate() {
        let Some(packed) = pack_pattern(pattern, index, &mut warnings)? else {
            continue;
        };
        let at = out.len() as u32;
        put_u32(&mut out, pat_table + index * 4, at);
        put_u16_vec(&mut out, packed.len() as u16);
        put_u16_vec(&mut out, pattern_rows(pattern, index, &mut warnings));
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&packed);
    }
    Ok((out, warnings))
}

fn it_flags(song: &Song) -> u16 {
    let mut flags = 0x0001 | 0x0004;
    if song.linear {
        flags |= 0x0008;
    }
    if song.old_effects {
        flags |= 0x0010;
    }
    if song.compatible_gxx {
        flags |= 0x0020;
    }
    flags
}

/// IT channel pan byte. `100` is surround, which the loader maps back to 128.
pub(crate) fn channel_pan_byte(model: u8, mute: bool) -> (u8, bool) {
    let (raw, exact) = if model == 128 {
        (100, true)
    } else {
        linear_pan(model)
    };
    let byte = raw | u8::from(mute) << 7;
    (byte, exact)
}

/// Sample or instrument pan, without the surround code (those fields have no
/// 100 = surround case). `(raw, exact)`.
pub(crate) fn linear_pan(model: u8) -> (u8, bool) {
    for raw in 0..=64 {
        if (u16::from(raw) * 255 / 64) as u8 == model {
            return (raw, true);
        }
    }
    let raw = (0..=64).min_by_key(|raw| ((u16::from(*raw) * 255 / 64) as u8).abs_diff(model));
    (raw.unwrap_or(32), false)
}

fn pattern_rows(pattern: &Pattern, index: usize, warnings: &mut Vec<String>) -> u16 {
    let rows = pattern.row_count();
    if rows == 0 {
        warnings.push(format!(
            "Pattern {index} had no rows. One empty row was written."
        ));
        1
    } else if rows > MAX_ROWS {
        warnings.push(format!(
            "Pattern {index} has more than {MAX_ROWS} rows. IT keeps the first {MAX_ROWS}."
        ));
        MAX_ROWS as u16
    } else {
        rows as u16
    }
}

fn pack_pattern(
    pattern: &Pattern,
    index: usize,
    warnings: &mut Vec<String>,
) -> Result<Option<Vec<u8>>, Error> {
    let rows = pattern.row_count().clamp(1, MAX_ROWS);
    let empty = pattern
        .rows
        .iter()
        .take(rows)
        .all(|row| row.iter().all(cell_empty));
    if empty && pattern.row_count() == 64 {
        return Ok(None);
    }
    let mut packed = Vec::new();
    let mut odd_note = false;
    let mut wide = false;
    for row in pattern.rows.iter().take(rows) {
        for (channel, cell) in row.iter().enumerate() {
            if cell_empty(cell) {
                continue;
            }
            if channel >= MAX_CHANNELS {
                wide = true;
                continue;
            }
            let mut mask = 0u8;
            let mut body = Vec::new();
            if cell.note != 0 {
                if let Some(note) = it_note(cell.note) {
                    mask |= 0x01;
                    body.push(note);
                } else {
                    odd_note = true;
                }
            }
            if cell.instrument != 0 {
                mask |= 0x02;
                body.push(cell.instrument);
            }
            if cell.has_volume {
                mask |= 0x04;
                body.push(cell.volume);
            }
            if cell.effect != 0 || cell.param != 0 {
                mask |= 0x08;
                body.push(cell.effect);
                body.push(cell.param);
            }
            if mask == 0 {
                continue;
            }
            packed.push((channel as u8 + 1) | 0x80);
            packed.push(mask);
            packed.extend(body);
        }
        packed.push(0);
    }
    if odd_note {
        warnings.push(format!(
            "Pattern {index} has a note IT cannot store. That note was left out."
        ));
    }
    if wide {
        warnings.push(format!(
            "Pattern {index} uses more than {MAX_CHANNELS} channels. The extra channels were left out."
        ));
    }
    if packed.len() > u16::MAX as usize {
        return Err(Error::Malformed(format!(
            "Pattern {index} does not fit in an IT file (packed data is larger than 65535 bytes). Nothing was written."
        )));
    }
    Ok(Some(packed))
}

fn cell_empty(cell: &Cell) -> bool {
    cell.note == 0
        && cell.instrument == 0
        && !cell.has_volume
        && cell.effect == 0
        && cell.param == 0
}

fn it_note(note: u8) -> Option<u8> {
    match note {
        NOTE_OFF => Some(255),
        NOTE_CUT => Some(254),
        NOTE_FADE => Some(253),
        1..=120 => Some(note - 1),
        _ => None,
    }
}

fn instrument_bytes(
    instrument: &Instrument,
    cmwt: u16,
    index: usize,
    warnings: &mut Vec<String>,
) -> Vec<u8> {
    if cmwt < 0x200 {
        if instrument.volume_env.enabled
            || instrument.pan_env.enabled
            || instrument.pitch_env.enabled
            || instrument.fadeout != 0
        {
            warnings.push(format!(
                "Instrument {} uses envelopes, but compatible version {cmwt:#06x} stores the old instrument layout without them.",
                index + 1
            ));
        }
        return old_instrument(instrument);
    }
    let mut bytes = vec![0u8; 0x1D4 + 82];
    bytes[0..4].copy_from_slice(b"IMPI");
    bytes[0x11] = match instrument.nna {
        NewNoteAction::Continue => 1,
        NewNoteAction::Off => 2,
        NewNoteAction::Fade => 3,
        NewNoteAction::Cut => 0,
    };
    bytes[0x12] = instrument.dct;
    bytes[0x13] = instrument.dca;
    bytes[0x14..0x16].copy_from_slice(&instrument.fadeout.to_le_bytes());
    let gbv = instrument.global_volume.min(128);
    bytes[0x18] = if gbv == 0 { 128 } else { gbv };
    bytes[0x19] = instrument_dfp(instrument.pan);
    write_text(&mut bytes[0x20..0x3A], &instrument.name);
    let mut sample_clipped = false;
    for note in 0..120 {
        let key = instrument.keys.get(note).copied().unwrap_or_default();
        let at = 0x40 + note * 2;
        bytes[at] = key.note.min(119);
        bytes[at + 1] = u8::try_from(key.sample.min(255)).unwrap_or(255);
        if key.sample > 255 {
            sample_clipped = true;
        }
    }
    if sample_clipped {
        warnings.push(format!(
            "Instrument {} maps a note to a sample above 255. IT stores 255.",
            index + 1
        ));
    }
    if instrument.volume_env.points.len() > 25
        || instrument.pan_env.points.len() > 25
        || instrument.pitch_env.points.len() > 25
    {
        warnings.push(format!(
            "Instrument {} has an envelope with more than 25 nodes. IT keeps 25.",
            index + 1
        ));
    }
    write_envelope(&mut bytes[0x130..0x130 + 81], &instrument.volume_env);
    write_envelope(&mut bytes[0x182..0x182 + 81], &instrument.pan_env);
    write_envelope(&mut bytes[0x1D4..0x1D4 + 81], &instrument.pitch_env);
    bytes
}

fn old_instrument(instrument: &Instrument) -> Vec<u8> {
    let mut bytes = vec![0u8; 0x120];
    bytes[0..4].copy_from_slice(b"IMPI");
    write_text(&mut bytes[0x14..0x2E], &instrument.name);
    for note in 0..120 {
        let key = instrument.keys.get(note).copied().unwrap_or_default();
        let at = 0x30 + note * 2;
        bytes[at] = key.note.min(119);
        bytes[at + 1] = u8::try_from(key.sample.min(255)).unwrap_or(255);
    }
    bytes
}

fn instrument_dfp(pan: Option<u8>) -> u8 {
    match pan {
        None => 0x80,
        Some(model) => linear_pan(model).0.min(64),
    }
}

fn write_envelope(dest: &mut [u8], envelope: &Envelope) {
    let count = envelope.points.len().min(25);
    let mut flags = 0u8;
    if envelope.enabled && count > 0 {
        flags |= 0x01;
    }
    if envelope.loop_on && count > 1 {
        flags |= 0x02;
    }
    if envelope.sustain && count > 0 {
        flags |= 0x04;
    }
    if envelope.filter {
        flags |= 0x80;
    }
    dest[0] = flags;
    dest[1] = count as u8;
    dest[2] = envelope.loop_start;
    dest[3] = envelope.loop_end;
    dest[4] = envelope.sustain_point;
    dest[5] = envelope.sustain_end;
    for (index, (tick, value)) in envelope.points.iter().take(25).enumerate() {
        let at = 6 + index * 3;
        dest[at] = *value;
        dest[at + 1..at + 3].copy_from_slice(&tick.to_le_bytes());
    }
}

fn sample_bytes(sample: &Sample, offset: u32, warnings: &mut Vec<String>) -> (Vec<u8>, Vec<u8>) {
    let clean8 = sample.pcm.iter().all(|frame| frame & 0x00FF == 0);
    let bits16 = sample.bits == 16 || !clean8;
    if sample.bits != 16 && !clean8 {
        warnings.push(format!(
            "Sample \"{}\" has 16-bit data and was stored as 16-bit.",
            sample.name
        ));
    }
    let data = if bits16 {
        let mut raw = Vec::with_capacity(sample.pcm.len() * 2);
        for frame in &sample.pcm {
            raw.extend_from_slice(&frame.to_le_bytes());
        }
        raw
    } else {
        sample.pcm.iter().map(|frame| (*frame >> 8) as u8).collect()
    };
    let mut header = vec![0u8; 0x50];
    header[0..4].copy_from_slice(b"IMPS");
    let gvl = sample.global_volume.min(64);
    header[0x11] = if gvl == 0 { 64 } else { gvl };
    header[0x13] = sample.volume.min(64);
    write_text(&mut header[0x14..0x2E], &sample.name);
    header[0x2E] = if bits16 { 0x01 | 0x02 } else { 0x01 };
    header[0x2F] = match sample.pan {
        Some(model) => linear_pan(model).0.min(64) | 0x80,
        None => 0,
    };
    let frames = data.len() / if bits16 { 2 } else { 1 };
    header[0x30..0x34].copy_from_slice(&(frames as u32).to_le_bytes());
    let (loop_on, ping) = loop_flags(sample.loop_kind, sample.loop_start, sample.loop_end, frames);
    let (sus_on, sus_ping) = loop_flags(
        sample.sustain_kind,
        sample.sustain_start,
        sample.sustain_end,
        frames,
    );
    if loop_on {
        header[0x34..0x38].copy_from_slice(&sample.loop_start.to_le_bytes());
        header[0x38..0x3C].copy_from_slice(&sample.loop_end.to_le_bytes());
    }
    let c5 = if sample.c5_speed == 0 {
        8363
    } else {
        sample.c5_speed
    };
    header[0x3C..0x40].copy_from_slice(&c5.to_le_bytes());
    if sus_on {
        header[0x40..0x44].copy_from_slice(&sample.sustain_start.to_le_bytes());
        header[0x44..0x48].copy_from_slice(&sample.sustain_end.to_le_bytes());
    }
    header[0x4C] = sample.vibrato.rate;
    header[0x4D] = sample.vibrato.depth;
    header[0x4E] = sample.vibrato.sweep;
    header[0x4F] = sample.vibrato.kind;
    let mut flags = 0u8;
    if !data.is_empty() {
        flags |= 0x01;
        let pointer = offset + 0x50;
        header[0x48..0x4C].copy_from_slice(&pointer.to_le_bytes());
    }
    if bits16 {
        flags |= 0x02;
    }
    if loop_on {
        flags |= 0x10;
    }
    if sus_on {
        flags |= 0x20;
    }
    if ping {
        flags |= 0x40;
    }
    if sus_ping {
        flags |= 0x80;
    }
    header[0x12] = flags;
    (header, data)
}

fn loop_flags(kind: LoopKind, start: u32, end: u32, frames: usize) -> (bool, bool) {
    if kind == LoopKind::None || end <= start || frames == 0 {
        return (false, false);
    }
    (true, kind == LoopKind::PingPong)
}

/// IT channel count is fixed at 64 in the header. Songs still only *use* the
/// channels that appear. [`channel_span`] reports the highest channel with a
/// cell or a mute, at least 1 and at most 64.
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

fn write_text(dest: &mut [u8], text: &str) {
    for (slot, ch) in dest.iter_mut().zip(text.chars()) {
        let value = u32::from(ch);
        if (0x20..0x7F).contains(&value) || (0xA0..=0xFF).contains(&value) {
            *slot = value as u8;
        }
    }
}

fn put_u16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u16_vec(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}
