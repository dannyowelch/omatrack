//! XM and IT parsers, malformed input, and render checks.
//!
//! The modules built here are original test songs, not third-party files.

use omatrack::player::{PlayerConfig, DEFAULT_SAMPLE_RATE};
use omatrack::track::{self, Format};
use omatrack::{open_bytes, Opened};

#[test]
fn synthetic_modules_round_trip() {
    for bytes in [
        minimal_xm(4),
        minimal_xm(8),
        minimal_it(false),
        minimal_it(true),
    ] {
        assert_round_trip(&bytes);
    }
}

#[test]
fn cc0_fixtures_round_trip() {
    for path in [
        "tests/data/blue_intermission_congusbongus_CC0.xm",
        "tests/data/jingle_bells_drmccoy_CC0.it",
    ] {
        assert_round_trip(&read_fixture(path));
    }
    for path in [
        "tests/data/long_rows_synthetic.xm",
        "tests/data/long_rows_synthetic.it",
    ] {
        let bytes = std::fs::read(path).unwrap_or_else(|err| panic!("{path}: {err}"));
        assert_round_trip(&bytes);
    }
}

#[test]
fn saved_headers_match_the_format_and_playback_stays_close() {
    let xm_bytes = read_fixture("tests/data/blue_intermission_congusbongus_CC0.xm");
    let it_bytes = read_fixture("tests/data/jingle_bells_drmccoy_CC0.it");
    for (bytes, magic) in [
        (&xm_bytes, &b"Extended Module: "[..]),
        (&it_bytes, &b"IMPM"[..]),
    ] {
        let song = load_track(bytes);
        let original = render_pcm(&song, 1.0);
        let (written, warnings) = omatrack::track::save_song(&song).expect("write");
        assert!(
            written.starts_with(magic),
            "header {:?}",
            &written[..magic.len()]
        );
        assert!(
            warnings.iter().all(|line| !line.contains("not written")),
            "unreferenced-pattern warning on a native save: {warnings:?}"
        );
        let again = load_track(&written);
        let replay = render_pcm(&again, 1.0);
        let frames = original.len().min(replay.len());
        assert!(frames > 8_000, "frames {frames}");
        let diff = original
            .iter()
            .zip(&replay)
            .map(|(left, right)| left.abs_diff(*right))
            .max()
            .unwrap_or(0);
        assert_eq!(diff, 0, "render diverged by {diff}");
    }
    assert_eq!(xm_bytes[37], 0x1A);
    let (xm_written, _) = omatrack::track::save_song(&load_track(&xm_bytes)).unwrap();
    assert_eq!(xm_written[37], 0x1A);
    let version = u16::from_le_bytes([xm_written[58], xm_written[59]]);
    assert_eq!(version, 0x0104);
    let (it_written, _) = omatrack::track::save_song(&load_track(&it_bytes)).unwrap();
    assert!(it_written.windows(4).any(|window| window == b"IMPI"));
    assert!(it_written.windows(4).any(|window| window == b"IMPS"));
}

#[test]
fn an_edit_and_an_unreferenced_pattern_survive_a_save() {
    let mut song = load_track(&minimal_xm(4));
    song.patterns[0].rows[3][1].note = 37;
    song.patterns[0].rows[3][1].instrument = 1;
    song.patterns[0].rows[3][1].volume = 0x32;
    song.patterns[0].rows[3][1].has_volume = true;
    song.patterns.push(omatrack::track::Pattern::empty(64, 4));
    let extra = song.patterns.len() - 1;
    song.patterns[extra].rows[0][0].note = 49;
    let (written, warnings) = omatrack::track::save_song(&song).unwrap();
    assert!(
        warnings.iter().all(|line| !line.contains("not written")),
        "{warnings:?}"
    );
    let again = load_track(&written);
    assert_eq!(again.patterns[0].rows[3][1].note, 37);
    assert_eq!(again.patterns[0].rows[3][1].instrument, 1);
    assert_eq!(again.patterns[0].rows[3][1].volume, 0x32);
    assert!(again.patterns[0].rows[3][1].has_volume);
    assert_eq!(again.patterns[extra].rows[0][0].note, 49);
    assert!(again
        .orders
        .iter()
        .all(|order| usize::from(*order) != extra));
}

#[test]
fn converting_to_mod_warns_and_leaves_the_song_argument_intact() {
    let song = load_track(&minimal_xm(8));
    let saved =
        omatrack::track::save_song_as(&song, omatrack::track::SaveFormat::Mod).expect("convert");
    assert!(
        saved.warnings.iter().any(|line| line.contains("channel")),
        "{:?}",
        saved.warnings
    );
    assert_eq!(song.channels, 8);
    match omatrack::open_bytes(&saved.bytes).unwrap() {
        omatrack::Opened::Mod(module) => {
            assert!(!module.patterns.is_empty());
            assert_eq!(module.patterns[0].rows[0].len(), 4);
        }
        omatrack::Opened::Track(_) => panic!("converted file was not a module"),
    }
    let _ = omatrack::track::Playback::new(PlayerConfig::default());
    let converted = match omatrack::open_bytes(&saved.bytes).unwrap() {
        omatrack::Opened::Mod(module) => module,
        omatrack::Opened::Track(_) => unreachable!(),
    };
    let mut playback = omatrack::player::Playback::new(PlayerConfig::default());
    playback.start(&converted, 0, 0);
    let mut buf = vec![0i16; 4_000];
    playback.render(&converted, &mut buf);
}

#[test]
fn a_module_saved_as_xm_loads_and_renders() {
    let bytes = std::fs::read("tests/data/fx-poly1_k0wax_CC0.mod").unwrap();
    let module = match omatrack::open_bytes(&bytes).unwrap() {
        omatrack::Opened::Mod(module) => module,
        omatrack::Opened::Track(_) => panic!("fixture is a module"),
    };
    let saved = omatrack::track::save_module(&module, omatrack::track::SaveFormat::Xm).expect("xm");
    assert!(saved.bytes.starts_with(b"Extended Module: "));
    let song = load_track(&saved.bytes);
    assert_eq!(song.format, Format::Xm);
    assert_eq!(song.channels, 4);
    let stats = render_stats(&song, 1.0);
    assert!(stats.frames > 1_000, "{}", stats.frames);
}

fn render_pcm(song: &omatrack::Song, seconds: f64) -> Vec<i16> {
    let mut playback = track::Playback::new(PlayerConfig::default());
    playback.set_stop_on_loop(true);
    playback.start(song, 0, 0);
    let max = (seconds * f64::from(DEFAULT_SAMPLE_RATE)) as usize;
    let mut pcm = Vec::new();
    while pcm.len() / 2 < max {
        let mut buf = vec![0i16; 2048 * 2];
        let wrote = playback.render(song, &mut buf);
        if wrote == 0 {
            break;
        }
        pcm.extend_from_slice(&buf[..wrote * 2]);
    }
    pcm
}

fn assert_round_trip(bytes: &[u8]) {
    let song = load_track(bytes);
    let (written, _warnings) = omatrack::track::save_song(&song).expect("write");
    let again = load_track(&written);
    assert_songs_equal(&song, &again);
}

fn assert_songs_equal(left: &omatrack::Song, right: &omatrack::Song) {
    assert_eq!(left.title, right.title, "title");
    assert_eq!(left.tracker, right.tracker, "tracker");
    assert_eq!(left.format, right.format, "format");
    assert_eq!(left.channels, right.channels, "channels");
    assert_eq!(left.orders, right.orders, "orders");
    assert_eq!(left.restart, right.restart, "restart");
    assert_eq!(left.linear, right.linear, "linear");
    assert_eq!(left.initial_speed, right.initial_speed, "speed");
    assert_eq!(left.initial_tempo, right.initial_tempo, "tempo");
    assert_eq!(
        left.initial_global_volume, right.initial_global_volume,
        "gvol"
    );
    assert_eq!(left.global_volume_max, right.global_volume_max, "gvol max");
    assert_eq!(left.initial_pan, right.initial_pan, "pan");
    assert_eq!(
        left.initial_channel_volume, right.initial_channel_volume,
        "cvol"
    );
    assert_eq!(left.initial_mute, right.initial_mute, "mute");
    assert_eq!(left.instrument_mode, right.instrument_mode, "ins mode");
    assert_eq!(left.old_effects, right.old_effects, "old fx");
    assert_eq!(left.compatible_gxx, right.compatible_gxx, "gxx");
    assert_eq!(left.compat, right.compat, "compat");
    assert_eq!(left.patterns.len(), right.patterns.len(), "patterns");
    for (index, (a, b)) in left.patterns.iter().zip(right.patterns.iter()).enumerate() {
        assert_eq!(a.rows.len(), b.rows.len(), "pattern {index} rows");
        for (row_index, (ar, br)) in a.rows.iter().zip(b.rows.iter()).enumerate() {
            assert_eq!(
                ar.len(),
                br.len(),
                "pattern {index} row {row_index} channels"
            );
            for (channel, (ac, bc)) in ar.iter().zip(br.iter()).enumerate() {
                assert_eq!(ac, bc, "pattern {index} row {row_index} ch {channel}");
            }
        }
    }
    assert_eq!(
        left.instruments.len(),
        right.instruments.len(),
        "instruments"
    );
    for (index, (a, b)) in left.instruments.iter().zip(&right.instruments).enumerate() {
        assert_eq!(a.name, b.name, "instrument {index} name");
        assert_eq!(a.keys, b.keys, "instrument {index} keys");
        assert_eq!(a.volume_env, b.volume_env, "instrument {index} vol env");
        assert_eq!(a.pan_env, b.pan_env, "instrument {index} pan env");
        assert_eq!(a.pitch_env, b.pitch_env, "instrument {index} pitch env");
        assert_eq!(a.fadeout, b.fadeout, "instrument {index} fade");
        assert_eq!(a.vibrato, b.vibrato, "instrument {index} vib");
        assert_eq!(a.nna, b.nna, "instrument {index} nna");
        assert_eq!(a.dct, b.dct, "instrument {index} dct");
        assert_eq!(a.dca, b.dca, "instrument {index} dca");
        assert_eq!(a.global_volume, b.global_volume, "instrument {index} gvol");
        assert_eq!(a.pan, b.pan, "instrument {index} pan");
        assert_eq!(a.owned_samples, b.owned_samples, "instrument {index} owned");
    }
    assert_eq!(left.samples.len(), right.samples.len(), "samples");
    for (index, (a, b)) in left.samples.iter().zip(&right.samples).enumerate() {
        assert_eq!(a.name, b.name, "sample {index} name");
        assert_eq!(a.bits, b.bits, "sample {index} bits");
        assert_eq!(a.volume, b.volume, "sample {index} volume");
        assert_eq!(a.global_volume, b.global_volume, "sample {index} gvol");
        assert_eq!(a.pan, b.pan, "sample {index} pan");
        assert_eq!(a.finetune, b.finetune, "sample {index} fine");
        assert_eq!(a.relative_note, b.relative_note, "sample {index} rel");
        assert_eq!(a.c5_speed, b.c5_speed, "sample {index} c5");
        assert_eq!(a.loop_start, b.loop_start, "sample {index} loop start");
        assert_eq!(a.loop_end, b.loop_end, "sample {index} loop end");
        assert_eq!(a.loop_kind, b.loop_kind, "sample {index} loop");
        assert_eq!(a.sustain_start, b.sustain_start, "sample {index} sus start");
        assert_eq!(a.sustain_end, b.sustain_end, "sample {index} sus end");
        assert_eq!(a.sustain_kind, b.sustain_kind, "sample {index} sus");
        assert_eq!(a.vibrato, b.vibrato, "sample {index} vib");
        assert_eq!(a.pcm.len(), b.pcm.len(), "sample {index} pcm len");
        if let Some(at) = a.pcm.iter().zip(&b.pcm).position(|(l, r)| l != r) {
            panic!(
                "sample {index} pcm differs at {at}: {} vs {}",
                a.pcm[at], b.pcm[at]
            );
        }
    }
}

#[test]
fn cc0_fixtures_load_and_make_sound() {
    let cases = [
        (
            "tests/data/blue_intermission_congusbongus_CC0.xm",
            Format::Xm,
            6,
        ),
        ("tests/data/jingle_bells_drmccoy_CC0.it", Format::It, 8),
    ];
    for (path, format, channels) in cases {
        let bytes = read_fixture(path);
        let song = load_track(&bytes);
        assert_eq!(song.format, format, "{path}");
        assert_eq!(song.channels, channels, "{path} title {}", song.title);
        assert!(
            song.samples
                .iter()
                .all(|sample| !sample.name.chars().any(|ch| ch.is_control())),
            "{path} sample names contain controls: {:?}",
            song.samples
                .iter()
                .map(|sample| &sample.name)
                .filter(|name| name.chars().any(|ch| ch.is_control()))
                .collect::<Vec<_>>()
        );
        let stats = render_stats(&song, 2.0);
        assert!(stats.peak > 1000, "{path} peak {}", stats.peak);
        assert!(stats.frames > 20_000, "{path} frames {}", stats.frames);
    }
}

#[test]
fn truncated_and_corrupt_files_do_not_panic() {
    let xm = minimal_xm(4);
    let it = minimal_it(false);
    let compressed = minimal_it(true);
    for bytes in [&xm, &it, &compressed] {
        for len in 0..bytes.len() {
            let _ = open_bytes(&bytes[..len]);
        }
        let mut corrupt = bytes.clone();
        for index in (8..corrupt.len()).step_by(17) {
            corrupt[index] ^= 0x5A;
            let _ = open_bytes(&corrupt);
        }
    }
    assert!(open_bytes(b"Extended Module: ").is_err());
    assert!(open_bytes(b"IMPM").is_err());
    assert!(open_bytes(&[0x1A; 1084]).is_err());
}

#[test]
fn xm_loads_four_channels_and_renders_audible_audio() {
    let song = load_track(&minimal_xm(4));
    assert_eq!(song.format, Format::Xm);
    assert_eq!(song.channels, 4);
    assert_eq!(song.cell(0, 0, 0).map(|cell| cell.note), Some(49));
    assert!(!song.samples.is_empty());
    assert!(song.samples[0].pcm.iter().any(|s| *s != 0));
    let stats = render_stats(&song, 8.0);
    assert!(stats.peak > 1000, "peak {}", stats.peak);
    assert!(stats.frames > 10_000, "frames {}", stats.frames);
    let seconds = stats.frames as f64 / f64::from(DEFAULT_SAMPLE_RATE);
    assert!(
        (0.5..30.0).contains(&seconds),
        "duration {seconds:.2}s ({} frames)",
        stats.frames
    );
}

#[test]
fn xm_with_eight_channels_reports_each_meter() {
    let song = load_track(&minimal_xm(8));
    assert_eq!(song.channels, 8);
    let mut playback = track::Playback::new(PlayerConfig::default());
    playback.set_stop_on_loop(true);
    playback.start(&song, 0, 0);
    let mut buf = vec![0i16; 44_100 * 2];
    let wrote = playback.render(&song, &mut buf);
    assert!(wrote > 1000, "wrote {wrote}");
    let peaks = playback.channel_peaks();
    assert_eq!(peaks.len(), 8);
    let hot = peaks.iter().filter(|peak| **peak > 100).count();
    assert!(hot >= 4, "peaks {peaks:?}");
}

#[test]
fn it_loads_and_renders_including_it214_samples() {
    let plain = load_track(&minimal_it(false));
    assert_eq!(plain.format, Format::It);
    assert!(plain.channels >= 1 && plain.channels <= 64);
    assert!(plain.samples[0].pcm.iter().any(|s| *s != 0));
    let plain_stats = render_stats(&plain, 4.0);
    assert!(plain_stats.peak > 400, "peak {}", plain_stats.peak);

    let packed = load_track(&minimal_it(true));
    assert!(
        packed.samples[0].pcm.iter().any(|s| s.abs() > 1000),
        "decompressed {:?}",
        &packed.samples[0].pcm[..packed.samples[0].pcm.len().min(8)]
    );
    let packed_stats = render_stats(&packed, 4.0);
    assert!(
        packed_stats.peak > 200,
        "compressed peak {}",
        packed_stats.peak
    );
}

#[test]
fn a_pingpong_xm_sample_keeps_sounding() {
    let mut bytes = minimal_xm(1);
    // Sample flag is inside the 40-byte header, which follows the 263-byte
    // instrument header. The flag byte is offset 14 of that header.
    // Locate it by scanning for the sample name we wrote, then step back.
    let name = b"square";
    let at = bytes
        .windows(name.len())
        .rposition(|window| window == name)
        .expect("sample name");
    // name starts at byte 18 of the sample header, flags are at byte 14.
    let flags = at - 4;
    bytes[flags] = 2; // ping-pong, 8-bit
    let song = load_track(&bytes);
    let stats = render_stats(&song, 2.0);
    assert!(stats.peak > 500, "peak {}", stats.peak);
    assert!(stats.tail > 200, "tail {}", stats.tail);
}

struct Stats {
    frames: usize,
    peak: u16,
    tail: u16,
}

/// The CC0 modules are stored as the original bytes and as ASCII base64 so a
/// text-only upload still carries them. When both exist they must match.
fn read_fixture(path: &str) -> Vec<u8> {
    let encoded = std::fs::read_to_string(format!("{path}.b64"))
        .unwrap_or_else(|err| panic!("{path}.b64: {err}"));
    let decoded = b64_decode(&encoded);
    if let Ok(bytes) = std::fs::read(path) {
        assert_eq!(bytes, decoded, "{path} does not match {path}.b64");
    }
    decoded
}

fn b64_decode(text: &str) -> Vec<u8> {
    fn val(byte: u8) -> u8 {
        match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => 0,
        }
    }
    let bytes: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            break;
        }
        let a = val(chunk[0]);
        let b = val(chunk[1]);
        out.push((a << 2) | (b >> 4));
        if chunk.len() > 2 && chunk[2] != b'=' {
            let c = val(chunk[2]);
            out.push((b << 4) | (c >> 2));
            if chunk.len() > 3 && chunk[3] != b'=' {
                let d = val(chunk[3]);
                out.push((c << 6) | d);
            }
        }
    }
    out
}

fn load_track(bytes: &[u8]) -> omatrack::Song {
    match open_bytes(bytes) {
        Ok(Opened::Track(song)) => song,
        Ok(Opened::Mod(_)) => panic!("expected xm or it, parsed a mod"),
        Err(err) => panic!("load failed: {err}"),
    }
}

fn render_stats(song: &omatrack::Song, seconds: f64) -> Stats {
    let mut playback = track::Playback::new(PlayerConfig::default());
    playback.set_stop_on_loop(true);
    playback.start(song, 0, 0);
    let max = (seconds * f64::from(DEFAULT_SAMPLE_RATE)) as usize;
    let mut pcm = Vec::new();
    while pcm.len() / 2 < max {
        let mut buf = vec![0i16; 2048 * 2];
        let wrote = playback.render(song, &mut buf);
        if wrote == 0 {
            break;
        }
        pcm.extend_from_slice(&buf[..wrote * 2]);
    }
    let peak = pcm
        .iter()
        .map(|sample| sample.unsigned_abs())
        .max()
        .unwrap_or(0);
    let tail_from = pcm.len().saturating_sub(4_000);
    let tail = pcm[tail_from..]
        .iter()
        .map(|sample| sample.unsigned_abs())
        .max()
        .unwrap_or(0);
    Stats {
        frames: pcm.len() / 2,
        peak,
        tail,
    }
}

fn minimal_xm(channels: u16) -> Vec<u8> {
    let channels = channels.clamp(1, 32);
    let mut out = Vec::new();
    out.extend_from_slice(b"Extended Module: ");
    out.extend_from_slice(b"Tiny XM             ");
    out.push(0x1A);
    out.extend_from_slice(b"omatrack-test       ");
    push_u16(&mut out, 0x0104);
    push_u32(&mut out, 276);
    push_u16(&mut out, 1);
    push_u16(&mut out, 0);
    push_u16(&mut out, channels);
    push_u16(&mut out, 1);
    push_u16(&mut out, 1);
    push_u16(&mut out, 1);
    push_u16(&mut out, 6);
    push_u16(&mut out, 125);
    out.extend_from_slice(&[0u8; 256]);

    let mut packed = Vec::new();
    for row in 0..64 {
        for channel in 0..channels {
            if row == 0 {
                packed.push(0x83);
                packed.push(49);
                packed.push(1);
            } else {
                packed.push(0x80);
            }
            let _ = channel;
        }
    }
    push_u32(&mut out, 9);
    out.push(0);
    push_u16(&mut out, 64);
    push_u16(&mut out, packed.len() as u16);
    out.extend_from_slice(&packed);

    let mut header = vec![0u8; 263];
    header[0..4].copy_from_slice(&263u32.to_le_bytes());
    header[4..10].copy_from_slice(b"square");
    header[27] = 1;
    header[29..33].copy_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&header);

    let mut sample = vec![0u8; 40];
    sample[0..4].copy_from_slice(&32u32.to_le_bytes());
    sample[8..12].copy_from_slice(&32u32.to_le_bytes());
    sample[12] = 64;
    sample[14] = 1;
    sample[15] = 128;
    sample[18..24].copy_from_slice(b"square");
    out.extend_from_slice(&sample);
    let mut acc = 0i8;
    for i in 0..32 {
        let target: i8 = if i % 2 == 0 { 100 } else { -100 };
        let delta = target.wrapping_sub(acc);
        acc = target;
        out.push(delta as u8);
    }
    out
}

fn minimal_it(compressed: bool) -> Vec<u8> {
    let mut out = vec![0u8; 0xC0];
    out[0..4].copy_from_slice(b"IMPM");
    out[4..10].copy_from_slice(b"TinyIT");
    out[0x20] = 1;
    out[0x22] = 1;
    out[0x24] = 1;
    out[0x26] = 1;
    out[0x2A] = 0x14;
    out[0x2B] = 0x02;
    let flags: u16 = 0x01 | 0x04 | 0x08;
    out[0x2C] = flags as u8;
    out[0x2D] = (flags >> 8) as u8;
    out[0x30] = 128;
    out[0x32] = 6;
    out[0x33] = 125;
    for channel in 0..64 {
        out[0x40 + channel] = 32;
        out[0x80 + channel] = 64;
    }
    out.push(0);
    let ins_at = out.len();
    push_u32(&mut out, 0);
    let smp_at = out.len();
    push_u32(&mut out, 0);
    let pat_at = out.len();
    push_u32(&mut out, 0);

    let ins_off = out.len() as u32;
    write_u32(&mut out, ins_at, ins_off);
    let mut inst = vec![0u8; 0x1D4 + 0x52];
    inst[0..4].copy_from_slice(b"IMPI");
    inst[0x20..0x26].copy_from_slice(b"Lead  ");
    inst[0x18] = 128;
    inst[0x19] = 0x80;
    for note in 0..120 {
        let pair = 0x40 + note * 2;
        inst[pair] = note as u8;
        inst[pair + 1] = 1;
    }
    out.extend_from_slice(&inst);

    let smp_off = out.len() as u32;
    write_u32(&mut out, smp_at, smp_off);
    let pointer_at = out.len() + 0x48;
    let mut smp = vec![0u8; 0x50];
    smp[0..4].copy_from_slice(b"IMPS");
    smp[0x11] = 64;
    smp[0x13] = 64;
    smp[0x14..0x1A].copy_from_slice(b"square");
    let length: u32 = 8;
    smp[0x30..0x34].copy_from_slice(&length.to_le_bytes());
    smp[0x38..0x3C].copy_from_slice(&length.to_le_bytes());
    smp[0x3C..0x40].copy_from_slice(&8363u32.to_le_bytes());
    let mut flags = 0x01 | 0x10;
    if compressed {
        flags |= 0x08;
    }
    smp[0x12] = flags;
    smp[0x2E] = 0x01;
    out.extend_from_slice(&smp);
    let data_off = out.len() as u32;
    write_u32(&mut out, pointer_at, data_off);
    if compressed {
        let deltas = [40, 0, -20, 0, 20, 0, -10, 0];
        out.extend_from_slice(&pack_it8(&deltas));
    } else {
        for i in 0..8 {
            let target: i8 = if i % 2 == 0 { 80 } else { -80 };
            out.push(target as u8);
        }
    }

    let pat_off = out.len() as u32;
    write_u32(&mut out, pat_at, pat_off);
    let mut packed = vec![0x81, 0x03, 60, 1, 0];
    packed.extend(std::iter::repeat_n(0, 63));
    push_u16(&mut out, packed.len() as u16);
    push_u16(&mut out, 64);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&packed);
    out
}

fn pack_it8(deltas: &[i32]) -> Vec<u8> {
    // Width 9: bit 8 clear, the low 8 bits are the signed delta. Bits are
    // stored low-bit-of-byte first, and that first bit is the high bit of
    // the value.
    let mut bits = Vec::new();
    let mut acc = 0u32;
    let mut nbits = 0u32;
    let flush = |acc: &mut u32, nbits: &mut u32, bits: &mut Vec<u8>| {
        while *nbits >= 8 {
            bits.push((*acc & 0xFF) as u8);
            *acc >>= 8;
            *nbits -= 8;
        }
    };
    for delta in deltas {
        let value = (*delta as u8) as u32; // bit 8 stays clear
        for shift in (0..9).rev() {
            let bit = (value >> shift) & 1;
            acc |= bit << nbits;
            nbits += 1;
            flush(&mut acc, &mut nbits, &mut bits);
        }
    }
    if nbits > 0 {
        bits.push((acc & 0xFF) as u8);
    }
    let mut out = Vec::new();
    out.extend_from_slice(&(bits.len() as u16).to_le_bytes());
    out.extend_from_slice(&bits);
    out
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
