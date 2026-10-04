//! The original tune used to try the tracker.
//!
//! `cargo run --example write_showcase -- song.mod` writes [`showcase`]. It is
//! a C major scale, not a borrowed song. The square sample loops so playback
//! holds each note for the whole row.

use crate::module::{Cell, Module, Tag};

/// Two-pattern scale with a speed command, a volume slide, and a pattern break.
pub fn showcase() -> Module {
    let mut module = Module::new(Tag::Mk);
    module
        .set_title("Omatrack")
        .expect("title fits the 20-byte field");
    module.song_length = 2;
    module.restart = 0;
    module.order[0] = 0;
    module.order[1] = 1;
    module.resize_patterns();

    module.samples[0]
        .set_name("square")
        .expect("sample name fits");
    module.samples[0].volume = 64;
    module.samples[0]
        .set_data(square_wave(64, 8))
        .expect("even length");
    // Loop the whole square so a note sustains instead of blipping once.
    module.samples[0].loop_start = 0;
    module.samples[0].loop_length = 32;

    module.samples[1]
        .set_name("tick")
        .expect("sample name fits");
    module.samples[1].volume = 48;
    module.samples[1].finetune_raw = 0x0F; // -1
    module.samples[1].set_data(decay(32)).expect("even length");

    // C major scale on channel 1, a low C on channel 2, speed on channel 4.
    let scale = [856, 762, 678, 640, 570, 508, 453, 428];
    for (step, period) in scale.iter().copied().enumerate() {
        let row = step * 2;
        module.patterns[0].rows[row][0] = Cell {
            sample: 1,
            period,
            effect: 0,
            param: 0,
        };
        if step % 2 == 0 {
            module.patterns[0].rows[row][1] = Cell {
                sample: 2,
                period: 856,
                effect: 0xC,
                param: 0x30,
            };
        }
    }
    module.patterns[0].rows[0][3] = Cell {
        sample: 0,
        period: 0,
        effect: 0xF,
        param: 0x06,
    };
    module.patterns[1].rows[0][0] = Cell {
        sample: 1,
        period: 428,
        effect: 0xA,
        param: 0x0F,
    };
    module.patterns[1].rows[16][0] = Cell {
        sample: 1,
        period: 214,
        effect: 0xD,
        param: 0x00,
    };
    module
}

fn square_wave(len: usize, period: usize) -> Vec<u8> {
    let mut data = vec![0u8; len];
    for (index, byte) in data.iter_mut().enumerate() {
        let high = (index % period) < (period / 2);
        *byte = if high { 96 } else { (-96i8) as u8 };
    }
    data
}

fn decay(len: usize) -> Vec<u8> {
    let mut data = vec![0u8; len];
    for (index, byte) in data.iter_mut().enumerate() {
        let amp = 100i16 - (index as i16 * 3);
        let sample = if index % 2 == 0 { amp } else { -amp / 2 };
        *byte = sample.clamp(-128, 127) as u8;
    }
    data
}
