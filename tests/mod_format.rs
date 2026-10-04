//! Parser and writer tests against the public library API.
//!
//! Modules here are synthesized. Do not point these tests at copyrighted songs.
//! Freely licensed fixtures live in `tests/data` and are loaded by `sample_fixtures`.

use omatrack::{Cell, Module, Tag, HEADER_LEN, PATTERN_BYTES};

fn assert_same_bytes(left: &[u8], right: &[u8]) {
    if left == right {
        return;
    }
    let shared = left.len().min(right.len());
    for index in 0..shared {
        if left[index] != right[index] {
            panic!(
                "byte {index}: {:02X} != {:02X} (lengths {} and {})",
                left[index],
                right[index],
                left.len(),
                right.len()
            );
        }
    }
    panic!("lengths differ: {} vs {}", left.len(), right.len());
}

fn round_trip(module: &Module) -> Module {
    let bytes = module.to_bytes().expect("write");
    let parsed = Module::from_bytes(&bytes).expect("parse");
    assert_same_bytes(&parsed.to_bytes().expect("rewrite"), &bytes);
    assert_eq!(&parsed, module);
    parsed
}

#[test]
fn hand_built_mk_file_matches_the_spec_and_round_trips() {
    // Layout offsets are the 31-sample ProTracker header, not derived from the writer.
    let data_at = HEADER_LEN + PATTERN_BYTES;
    let mut bytes = vec![0u8; data_at + 4 + 3];

    bytes[0..2].copy_from_slice(b"HI");
    bytes[20..24].copy_from_slice(b"kick");
    bytes[42] = 0x00;
    bytes[43] = 0x02; // 2 words = 4 bytes
    bytes[44] = 0x08; // finetune -8
    bytes[45] = 64;
    bytes[48] = 0x00;
    bytes[49] = 0x01; // loop length 1 word: no loop

    // Instrument 31 sits in the last 30-byte header, ending at the song length.
    let last = 20 + 30 * 30;
    assert_eq!(last, 920);
    bytes[last..last + 4].copy_from_slice(b"last");
    bytes[last + 24] = 0x07; // finetune +7
    bytes[last + 25] = 40; // volume

    bytes[950] = 1;
    bytes[951] = 0x7F;
    bytes[1080..1084].copy_from_slice(b"M.K.");

    // sample 1, period 856 (C-1 = 0x0358), effect C, param 40.
    // byte0 = sample hi | period hi = 0x03
    // byte2 = sample lo << 4 | effect = 0x1C
    bytes[1084..1088].copy_from_slice(&[0x03, 0x58, 0x1C, 0x40]);

    // sample 16 (0x10), period 214 (C-3 = 0x00D6), effect A, param 0F.
    // Row 1 channel 1 starts five cells in.
    let row = 1usize;
    let channel = 1usize;
    let mid = HEADER_LEN + (row * 4 + channel) * 4;
    assert_eq!(mid, 1104);
    bytes[mid..mid + 4].copy_from_slice(&[0x10, 0xD6, 0x0A, 0x0F]);

    // sample 31 (0x1F), period 113 (B-3 = 0x0071), effect F, param 06.
    // Last cell of the pattern.
    let end_cell = HEADER_LEN + (63 * 4 + 3) * 4;
    assert_eq!(end_cell, data_at - 4);
    bytes[end_cell..end_cell + 4].copy_from_slice(&[0x10, 0x71, 0xFF, 0x06]);

    bytes[data_at..data_at + 4].copy_from_slice(&[0x80, 0x7F, 0x00, 0x01]);
    bytes[data_at + 4..].copy_from_slice(&[0xAA, 0xBB, 0xCC]);

    let module = Module::from_bytes(&bytes).expect("parse hand-built file");
    assert_eq!(module.display_title(), "HI");
    assert_eq!(module.tag, Tag::Mk);
    assert_eq!(module.song_length, 1);
    assert_eq!(module.restart, 0x7F);
    assert_eq!(module.patterns.len(), 1);
    assert_eq!(module.samples[0].display_name(), "kick");
    assert_eq!(module.samples[0].finetune(), -8);
    assert_eq!(module.samples[0].finetune_raw, 0x08);
    assert_eq!(module.samples[0].volume, 64);
    assert!(!module.samples[0].loops());
    assert_eq!(module.samples[0].data, vec![0x80, 0x7F, 0x00, 0x01]);
    assert_eq!(module.samples[30].display_name(), "last");
    assert_eq!(module.samples[30].finetune(), 7);
    assert_eq!(module.samples[30].volume, 40);
    assert!(module.samples[30].data.is_empty());

    let first = module.patterns[0].rows[0][0];
    assert_eq!(
        first,
        Cell {
            sample: 1,
            period: 856,
            effect: 0xC,
            param: 0x40,
        }
    );
    let mid_cell = module.patterns[0].rows[1][1];
    assert_eq!(
        mid_cell,
        Cell {
            sample: 16,
            period: 214,
            effect: 0xA,
            param: 0x0F,
        }
    );
    let last_cell = module.patterns[0].rows[63][3];
    assert_eq!(
        last_cell,
        Cell {
            sample: 31,
            period: 113,
            effect: 0xF,
            param: 0x06,
        }
    );
    assert_eq!(module.trailing, vec![0xAA, 0xBB, 0xCC]);
    assert_same_bytes(&module.to_bytes().expect("write"), &bytes);
}

#[test]
fn order_entries_past_the_song_length_reserve_patterns() {
    let mut module = Module::new(Tag::FourChannel);
    module.set_title("Orders").unwrap();
    module.song_length = 1;
    module.order[0] = 0;
    module.order[10] = 2;
    module.resize_patterns();
    assert_eq!(module.patterns.len(), 3);
    module.patterns[2].rows[7][2] = Cell {
        sample: 5,
        period: 428,
        effect: 0xD,
        param: 0x20,
    };
    let parsed = round_trip(&module);
    assert_eq!(parsed.tag, Tag::FourChannel);
    assert_eq!(parsed.song_length, 1);
    assert_eq!(parsed.patterns.len(), 3);
    assert_eq!(parsed.patterns[2].rows[7][2].period, 428);
    assert_eq!(parsed.order[10], 2);

    let mut short = module.to_bytes().unwrap();
    // Drop the two patterns the unused order slot asked for.
    short.truncate(HEADER_LEN + PATTERN_BYTES);
    let err = Module::from_bytes(&short).unwrap_err();
    assert!(err.to_string().contains("pattern data"), "{}", err);
}

#[test]
fn every_supported_tag_round_trips_and_others_are_rejected() {
    for tag in [Tag::Mk, Tag::Extended, Tag::Startrekker, Tag::FourChannel] {
        let mut module = Module::new(tag);
        module.samples[0].finetune_raw = 0x1F; // high bit preserved, nibble is -1
        module.samples[0].volume = 80; // above 64, still stored
        module.samples[0].loop_start = 0x0102;
        module.samples[0].loop_length = 0x0304;
        module.restart = 127;
        let parsed = round_trip(&module);
        assert_eq!(parsed.tag.as_bytes(), tag.as_bytes());
        assert_eq!(parsed.samples[0].finetune_raw, 0x1F);
        assert_eq!(parsed.samples[0].finetune(), -1);
        assert_eq!(parsed.samples[0].volume, 80);
        assert_eq!(parsed.samples[0].loop_start, 0x0102);
        assert_eq!(parsed.samples[0].loop_length, 0x0304);
    }

    let mut module = Module::new(Tag::Mk);
    module.order[0] = 64;
    module.resize_patterns();
    assert_eq!(module.patterns.len(), 65);
    module.patterns[64].rows[0][0].period = 214;
    let bytes = module.to_bytes().unwrap();
    assert_eq!(&bytes[1080..1084], b"M.K.");
    let parsed = Module::from_bytes(&bytes).unwrap();
    assert_eq!(parsed.patterns.len(), 65);
    assert_eq!(parsed.patterns[64].rows[0][0].period, 214);
    assert_eq!(parsed.tag, Tag::Mk);

    for tag in [b"8CHN", b"FLT8", b"CD81", b"M.K ", b"mk.M"] {
        let mut bytes = Module::new(Tag::Mk).to_bytes().unwrap();
        bytes[1080..1084].copy_from_slice(tag);
        let err = Module::from_bytes(&bytes).unwrap_err();
        assert!(
            err.to_string().contains("unrecognized module tag"),
            "{tag:?} -> {err}"
        );
    }
}

#[test]
fn malformed_and_truncated_files_are_errors() {
    let module = {
        let mut module = Module::new(Tag::Mk);
        module.samples[3].set_name("tone").unwrap();
        module.samples[3].set_data(vec![9, 8, 7, 6]).unwrap();
        module.patterns[0].rows[4][1] = Cell {
            sample: 4,
            period: 678,
            effect: 0x1,
            param: 0x02,
        };
        module
    };
    let bytes = module.to_bytes().unwrap();

    for len in 0..bytes.len() {
        assert!(
            Module::from_bytes(&bytes[..len]).is_err(),
            "prefix of {len} bytes was accepted"
        );
    }

    assert!(Module::from_bytes(&[])
        .unwrap_err()
        .to_string()
        .contains("truncated"));
    assert!(Module::from_bytes(&[0; 1083])
        .unwrap_err()
        .to_string()
        .contains("truncated"));

    let mut bad_len = bytes.clone();
    bad_len[950] = 0;
    assert!(Module::from_bytes(&bad_len)
        .unwrap_err()
        .to_string()
        .contains("song length 0"));
    bad_len[950] = 200;
    assert!(Module::from_bytes(&bad_len)
        .unwrap_err()
        .to_string()
        .contains("song length 200"));

    let mut huge_sample = bytes.clone();
    huge_sample[20 + 3 * 30 + 22] = 0x10; // instrument 4 length high byte
    huge_sample[20 + 3 * 30 + 23] = 0x00; // 0x1000 words
    let err = Module::from_bytes(&huge_sample).unwrap_err();
    assert!(err.to_string().contains("sample data"), "{err}");

    let garbage: Vec<u8> = (0..4000u32).map(|n| (n.wrapping_mul(17)) as u8).collect();
    assert!(Module::from_bytes(&garbage).is_err());

    let mut odd = module.clone();
    odd.samples[0].data = vec![1, 2, 3];
    assert!(odd.to_bytes().unwrap_err().to_string().contains("odd"));

    let mut period = module.clone();
    period.patterns[0].rows[0][0].period = 4096;
    assert!(period
        .to_bytes()
        .unwrap_err()
        .to_string()
        .contains("period"));

    let mut effect = module.clone();
    effect.patterns[0].rows[0][0].effect = 16;
    assert!(effect
        .to_bytes()
        .unwrap_err()
        .to_string()
        .contains("effect"));

    let mut short = module.clone();
    short.patterns.clear();
    let err = short.to_bytes().unwrap_err();
    assert!(err.to_string().contains("pattern count"), "{err}");

    let mut zero_len = module.clone();
    zero_len.song_length = 0;
    assert!(zero_len.to_bytes().is_err());
}

#[test]
fn random_modules_round_trip() {
    for seed in [1u32, 7, 42, 99, 0xC0FFEE, 0xFFFF_FFFF] {
        let module = random_module(seed);
        round_trip(&module);
    }
}

#[test]
fn load_and_save_round_trip_on_disk() {
    let module = random_module(0xA1B2_C3D4);
    let path = std::env::temp_dir().join(format!("omatrack-m1-{}.mod", std::process::id()));
    module.save(&path).expect("save");
    let loaded = Module::load(&path).expect("load");
    assert_same_bytes(
        &loaded.to_bytes().expect("rewrite"),
        &module.to_bytes().expect("write"),
    );
    let _ = std::fs::remove_file(&path);

    let missing = Module::load("/no/such/omatrack-module.mod").unwrap_err();
    assert!(missing.to_string().contains("failed to read"), "{missing}");
}

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    fn byte(&mut self) -> u8 {
        self.next() as u8
    }

    fn below(&mut self, max: usize) -> usize {
        (self.next() as usize) % max.max(1)
    }
}

fn random_module(seed: u32) -> Module {
    let mut rng = Rng(seed);
    let tags = [Tag::Mk, Tag::Extended, Tag::Startrekker, Tag::FourChannel];
    let mut module = Module::new(tags[rng.below(tags.len())]);
    for byte in &mut module.title {
        *byte = rng.byte();
    }
    module.song_length = (rng.byte() % 128) + 1;
    module.restart = rng.byte();
    for slot in &mut module.order {
        *slot = rng.byte() % 4;
    }
    module.resize_patterns();
    for sample in &mut module.samples {
        for byte in &mut sample.name {
            *byte = rng.byte();
        }
        sample.finetune_raw = rng.byte();
        sample.volume = rng.byte();
        sample.loop_start = rng.next() as u16;
        sample.loop_length = rng.next() as u16;
        let len = rng.below(9) * 2;
        sample.data = (0..len).map(|_| rng.byte()).collect();
    }
    for pattern in &mut module.patterns {
        for row in &mut pattern.rows {
            for cell in row.iter_mut() {
                cell.sample = rng.byte();
                cell.period = (rng.next() as u16) & 0x0FFF;
                cell.effect = rng.byte() & 0x0F;
                cell.param = rng.byte();
            }
        }
    }
    let trail = rng.below(9);
    module.trailing = (0..trail).map(|_| rng.byte()).collect();
    module
}
