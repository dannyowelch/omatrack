//! Playback engine: periods, effects, looping, and the showcase tune.

use omatrack::demo::showcase;
use omatrack::player::{
    pal_hz, sample_step, samples_per_tick, tuned_period, Interpolation, Playback, PlayerConfig,
    DEFAULT_SPEED, DEFAULT_TEMPO, PAL_CLOCK_HZ,
};
use omatrack::{wav_bytes, Cell, Module, Tag};

fn tone(period: u16, effect: u8, param: u8) -> Module {
    let mut module = Module::new(Tag::Mk);
    module.samples[0].volume = 64;
    module.samples[0]
        .set_data(vec![96, 96, (-96i8) as u8, (-96i8) as u8])
        .unwrap();
    module.samples[0].loop_start = 0;
    module.samples[0].loop_length = 2;
    module.patterns[0].rows[0][0] = Cell {
        sample: 1,
        period,
        effect,
        param,
    };
    module
}

fn config(rate: u32) -> PlayerConfig {
    PlayerConfig {
        sample_rate: rate,
        interpolation: Interpolation::Nearest,
        stereo_separation: 100,
    }
}

fn start(module: &Module, rate: u32) -> Playback {
    let mut playback = Playback::new(config(rate));
    playback.start(module, 0, 0);
    playback
}

fn tick(playback: &mut Playback, module: &Module) {
    let frames = samples_per_tick(playback.sample_rate(), playback.tempo()) as usize;
    let mut buffer = vec![0i16; frames * 2];
    let wrote = playback.render(module, &mut buffer);
    assert_eq!(wrote, frames, "a running song fills the tick");
}

fn play(module: &Module, rate: u32, ticks: usize) -> Playback {
    let mut playback = start(module, rate);
    for _ in 0..ticks {
        tick(&mut playback, module);
    }
    playback
}

#[test]
fn known_period_renders_the_pal_frequency_on_the_left() {
    let module = tone(428, 0, 0);
    let rate = 44_100;
    let mut playback = start(&module, rate);
    let mut pcm = vec![0i16; rate as usize * 2];
    let wrote = playback.render(&module, &mut pcm);
    assert_eq!(wrote, rate as usize);

    let mut rising = 0u32;
    let mut previous = 0i16;
    let mut right_energy = 0i64;
    let mut left_energy = 0i64;
    for frame in pcm.chunks_exact(2) {
        if previous < 0 && frame[0] >= 0 {
            rising += 1;
        }
        previous = frame[0];
        left_energy += i64::from(frame[0]).abs();
        right_energy += i64::from(frame[1]).abs();
    }
    let hz = f64::from(rising) * f64::from(rate) / f64::from(rate);
    let expected = pal_hz(428) / 4.0;
    assert!(
        (hz - expected).abs() < 2.0,
        "heard {hz} Hz, expected {expected}"
    );
    assert!(left_energy > 1_000_000, "left channel was silent");
    assert_eq!(right_energy, 0, "channel 0 is hard-left");
    assert!((expected - f64::from(PAL_CLOCK_HZ) / 428.0 / 4.0).abs() < 1e-6);
}

#[test]
fn finetune_shifts_the_period_before_the_note_plays() {
    let mut module = tone(428, 0, 0);
    module.samples[0].finetune_raw = 7;
    let playback = play(&module, 100, 1);
    let channel = playback.channel(0).unwrap();
    assert_eq!(channel.period, tuned_period(428, 7));
    assert_eq!(channel.period, 407);
    assert_eq!(channel.finetune, 7);
}

#[test]
fn hard_pan_puts_each_amiga_pair_on_one_side_and_separation_bleeds() {
    for (channel, left) in [(0, true), (1, false), (2, false), (3, true)] {
        let mut module = tone(428, 0, 0);
        module.patterns[0].rows[0][0] = Cell::empty();
        module.patterns[0].rows[0][channel] = Cell {
            sample: 1,
            period: 428,
            effect: 0,
            param: 0,
        };
        let mut playback = start(&module, 8_000);
        let mut buffer = vec![0i16; samples_per_tick(8_000, 125) as usize * 2];
        playback.render(&module, &mut buffer);
        let (l, r) = (buffer[0], buffer[1]);
        if left {
            assert!(
                l != 0 && r == 0,
                "channel {channel} should be left, got {l}/{r}"
            );
        } else {
            assert!(
                l == 0 && r != 0,
                "channel {channel} should be right, got {l}/{r}"
            );
        }
    }

    let module = tone(428, 0, 0);
    let mut narrow = Playback::new(PlayerConfig {
        sample_rate: 8_000,
        interpolation: Interpolation::Nearest,
        stereo_separation: 0,
    });
    narrow.start(&module, 0, 0);
    let mut buffer = vec![0i16; samples_per_tick(8_000, 125) as usize * 2];
    narrow.render(&module, &mut buffer);
    assert_eq!(buffer[0], buffer[1]);
    assert_ne!(buffer[0], 0);

    let mut half = Playback::new(PlayerConfig {
        sample_rate: 8_000,
        interpolation: Interpolation::Nearest,
        stereo_separation: 50,
    });
    half.start(&module, 0, 0);
    half.render(&module, &mut buffer);
    assert_eq!(buffer[1], buffer[0] / 2);
}

#[test]
fn mute_silences_a_channel_without_stopping_it() {
    let module = tone(428, 0xA, 0x01);
    let mut playback = start(&module, 100);
    playback.set_mute(0, true);
    tick(&mut playback, &module);
    let mut buffer = vec![0i16; samples_per_tick(100, 125) as usize * 2];
    // The tick above already rendered. Render another and inspect state.
    playback.render(&module, &mut buffer);
    assert!(buffer.iter().all(|sample| *sample == 0));
    let channel = playback.channel(0).unwrap();
    assert!(channel.muted);
    assert!(channel.active);
    assert!(channel.position > 0);
    assert!(channel.volume < 64, "volume slide still runs while muted");
}

#[test]
fn a_one_shot_stops_and_a_loop_wraps() {
    let mut once = tone(428, 0, 0);
    once.samples[0].loop_length = 1;
    let playback = play(&once, 44_100, 8);
    assert!(!playback.channel(0).unwrap().active);

    let looping = tone(428, 0, 0);
    let playback = play(&looping, 44_100, 40);
    let channel = playback.channel(0).unwrap();
    assert!(channel.active);
    assert!(channel.position < 4 << 16, "cursor left the 4-byte loop");
}

#[test]
fn linear_interpolation_is_not_just_the_nearest_byte() {
    let mut module = tone(214, 0, 0);
    module.samples[0].set_data(vec![0, 40, 80, 120]).unwrap();
    let frames = 256;
    let mut nearest = Playback::new(config(44_100));
    nearest.start(&module, 0, 0);
    let mut linear = Playback::new(PlayerConfig {
        sample_rate: 44_100,
        interpolation: Interpolation::Linear,
        stereo_separation: 100,
    });
    linear.start(&module, 0, 0);
    let mut a = vec![0i16; frames * 2];
    let mut b = vec![0i16; frames * 2];
    nearest.render(&module, &mut a);
    linear.render(&module, &mut b);
    assert_ne!(a, b);
    let levels = [0, 40 * 64, 80 * 64, 120 * 64];
    assert!(a.iter().step_by(2).all(|sample| levels.contains(sample)));
    assert!(b.iter().step_by(2).any(|sample| !levels.contains(sample)));
}

#[test]
fn slides_wait_for_the_second_tick_and_clamp() {
    let module = tone(428, 0x1, 0x02);
    let playback = play(&module, 100, 1);
    assert_eq!(playback.channel(0).unwrap().period, 428);
    assert_eq!(playback.row(), 0);
    let playback = play(&module, 100, 2);
    assert_eq!(playback.channel(0).unwrap().period, 426);
    let playback = play(&module, 100, 6);
    assert_eq!(playback.channel(0).unwrap().period, 418);
    assert_eq!(playback.row(), 1);

    let mut memory = tone(428, 0x1, 0x02);
    memory.patterns[0].rows[1][0] = Cell {
        sample: 0,
        period: 0,
        effect: 0x1,
        param: 0,
    };
    let playback = play(&memory, 100, 12);
    assert_eq!(playback.channel(0).unwrap().period, 408);

    let low = tone(120, 0x1, 0x20);
    let playback = play(&low, 100, 2);
    assert_eq!(playback.channel(0).unwrap().period, 113);

    let high = tone(856, 0x2, 0x10);
    let playback = play(&high, 100, 2);
    assert_eq!(playback.channel(0).unwrap().period, 856);
}

#[test]
fn glissando_snaps_the_played_period_to_a_semitone() {
    let mut module = tone(428, 0xE, 0x31);
    module.patterns[0].rows[1][0] = Cell {
        sample: 0,
        period: 0,
        effect: 0x1,
        param: 0x14,
    };
    let played = play(&module, 100, 8);
    let channel = played.channel(0).unwrap();
    assert_eq!(channel.period, 408);
    assert_eq!(channel.audible_period, 404);
}

#[test]
fn fine_slide_runs_once_on_the_first_tick() {
    let module = tone(428, 0xE, 0x11);
    let playback = play(&module, 100, 1);
    assert_eq!(playback.channel(0).unwrap().period, 427);
    let playback = play(&module, 100, 3);
    assert_eq!(playback.channel(0).unwrap().period, 427);
}

#[test]
fn arpeggio_cycles_semitones_after_the_first_tick() {
    let module = tone(428, 0x0, 0x47);
    assert_eq!(
        play(&module, 100, 1).channel(0).unwrap().audible_period,
        428
    );
    assert_eq!(
        play(&module, 100, 2).channel(0).unwrap().audible_period,
        339
    );
    assert_eq!(
        play(&module, 100, 3).channel(0).unwrap().audible_period,
        285
    );
    assert_eq!(
        play(&module, 100, 4).channel(0).unwrap().audible_period,
        428
    );
}

#[test]
fn vibrato_and_tremolo_modulate_without_replacing_the_base() {
    let vibrato = tone(428, 0x4, 0x48);
    let played = play(&vibrato, 100, 3);
    let channel = played.channel(0).unwrap();
    assert_eq!(channel.period, 428);
    assert_ne!(channel.audible_period, 428);
    assert_eq!(channel.volume, 64);

    let tremolo = tone(428, 0x7, 0x4F);
    let played = play(&tremolo, 100, 3);
    let channel = played.channel(0).unwrap();
    assert_eq!(channel.volume, 64);
    assert_ne!(channel.audible_volume, 64);
}

#[test]
fn tone_portamento_does_not_retrigger() {
    let mut module = tone(428, 0, 0);
    module.samples[0].loop_length = 1;
    module.samples[0].set_data(vec![96; 8_000]).unwrap();
    module.patterns[0].rows[1][0] = Cell {
        sample: 0,
        period: 214,
        effect: 0x3,
        param: 0x10,
    };
    let before = play(&module, 100, 6);
    let after = play(&module, 100, 7);
    assert!(after.channel(0).unwrap().position > before.channel(0).unwrap().position);
    assert_eq!(after.channel(0).unwrap().period, 428);
    let slid = play(&module, 100, 8);
    assert_eq!(slid.channel(0).unwrap().period, 412);
}

#[test]
fn volume_slide_memory_and_set_volume() {
    let set = tone(428, 0xC, 0x10);
    assert_eq!(play(&set, 100, 1).channel(0).unwrap().volume, 16);
    let clamped = tone(428, 0xC, 0x7F);
    assert_eq!(play(&clamped, 100, 1).channel(0).unwrap().volume, 64);

    let mut slide = tone(428, 0xA, 0x04);
    slide.patterns[0].rows[1][0] = Cell {
        sample: 0,
        period: 0,
        effect: 0xA,
        param: 0,
    };
    assert_eq!(play(&slide, 100, 6).channel(0).unwrap().volume, 44);
    assert_eq!(play(&slide, 100, 12).channel(0).unwrap().volume, 24);

    let mut porta_slide = tone(428, 0, 0);
    porta_slide.patterns[0].rows[1][0] = Cell {
        sample: 0,
        period: 214,
        effect: 0x5,
        param: 0x04,
    };
    // 5xy does not invent a portamento speed. Plant one on the way in.
    porta_slide.patterns[0].rows[0][0].effect = 0x3;
    porta_slide.patterns[0].rows[0][0].param = 0x08;
    let played = play(&porta_slide, 100, 8);
    let channel = played.channel(0).unwrap();
    assert!(
        channel.period < 428,
        "portamento ran, period {}",
        channel.period
    );
    assert!(
        channel.volume < 64,
        "volume slide ran, volume {}",
        channel.volume
    );
}

#[test]
fn sample_offset_is_param_times_256_bytes() {
    let mut module = tone(428, 0x9, 0x01);
    module.samples[0].loop_length = 1;
    module.samples[0].set_data(vec![0; 1024]).unwrap();
    let playback = play(&module, 100, 1);
    let step = sample_step(428, 100);
    let frames = u64::from(samples_per_tick(100, 125));
    let channel = playback.channel(0).unwrap();
    assert!(channel.active);
    assert_eq!(channel.position, (256 << 16) + step * frames);

    let mut past = tone(428, 0x9, 0x01);
    past.samples[0].loop_length = 1;
    past.samples[0].set_data(vec![0; 100]).unwrap();
    let playback = play(&past, 100, 1);
    assert!(!playback.channel(0).unwrap().active);
}

#[test]
fn speed_tempo_jump_and_break() {
    let mut module = tone(428, 0, 0);
    module.song_length = 3;
    module.order[1] = 0;
    module.order[2] = 0;
    module.patterns[0].rows[0][3] = Cell {
        sample: 0,
        period: 0,
        effect: 0xF,
        param: 0x03,
    };
    let playback = play(&module, 100, 1);
    assert_eq!(playback.speed(), 3);
    assert_eq!(playback.row(), 0);
    let playback = play(&module, 100, 3);
    assert_eq!(playback.row(), 1);

    module.patterns[0].rows[0][3].param = 0x7D;
    let playback = play(&module, 100, 1);
    assert_eq!(playback.tempo(), 125);
    assert_eq!(playback.speed(), 6);

    module.patterns[0].rows[0][0] = Cell {
        sample: 0,
        period: 0,
        effect: 0xB,
        param: 0x02,
    };
    module.patterns[0].rows[0][1] = Cell {
        sample: 0,
        period: 0,
        effect: 0xD,
        param: 0x16,
    };
    module.patterns[0].rows[0][3].param = 0x01;
    let playback = play(&module, 100, 1);
    assert_eq!(playback.order(), 2);
    assert_eq!(playback.row(), 16);

    // A later channel's position jump wins. Speed stays on channel 3's old
    // slot only until we overwrite it, so the speed command moves to channel 2.
    module.patterns[0].rows[0][2] = Cell {
        sample: 0,
        period: 0,
        effect: 0xF,
        param: 0x01,
    };
    module.patterns[0].rows[0][3] = Cell {
        sample: 0,
        period: 0,
        effect: 0xB,
        param: 0x01,
    };
    module.patterns[0].rows[0][1] = Cell::empty();
    let playback = play(&module, 100, 1);
    assert_eq!(playback.order(), 1);
    assert_eq!(playback.row(), 0);
}

#[test]
fn pattern_break_uses_decimal_digits_and_the_song_loops() {
    let mut module = tone(428, 0xD, 0x0A);
    module.song_length = 2;
    module.order[1] = 0;
    module.patterns[0].rows[0][3] = Cell {
        sample: 0,
        period: 0,
        effect: 0xF,
        param: 0x01,
    };
    let playback = play(&module, 100, 1);
    assert_eq!(playback.order(), 1);
    assert_eq!(playback.row(), 10);
    assert!(!playback.looped());

    let mut single = Module::new(Tag::Mk);
    single.patterns[0].rows[0][3] = Cell {
        sample: 0,
        period: 0,
        effect: 0xF,
        param: 0x01,
    };
    let mut playback = start(&single, 100);
    playback.set_stop_on_loop(true);
    for _ in 0..64 {
        tick(&mut playback, &single);
    }
    assert!(playback.looped());
    let mut buffer = vec![0i16; 8];
    assert_eq!(playback.render(&single, &mut buffer), 0);
}

#[test]
fn restart_clears_voices_pattern_loops_and_peaks() {
    let mut module = tone(428, 0xE, 0x60);
    module.restart = 3;
    module.song_length = 4;
    module.patterns[0].rows[0][2] = Cell {
        sample: 0,
        period: 0,
        effect: 0xF,
        param: 0x80,
    };
    module.patterns[0].rows[0][3] = Cell {
        sample: 0,
        period: 0,
        effect: 0xF,
        param: 0x01,
    };
    module.patterns[0].rows[1][0] = Cell {
        sample: 1,
        period: 381,
        effect: 0xE,
        param: 0x61,
    };

    let mut playback = start(&module, 8_000);
    playback.set_mute(1, true);
    playback.start(&module, 3, 20);
    assert_eq!(playback.order(), 3);
    assert_eq!(playback.row(), 20);
    assert!(playback.channel(1).unwrap().muted);
    playback.start(&module, 0, 0);
    assert_eq!(playback.order(), 0);
    assert_eq!(playback.row(), 0);
    for _ in 0..4 {
        tick(&mut playback, &module);
    }
    assert_eq!(playback.row(), 2, "the pattern loop has already been spent");
    assert_eq!(playback.tempo(), 128);
    assert_eq!(playback.speed(), 1);
    let held = playback.channel(0).unwrap();
    assert!(held.active);
    assert!(held.position > 0);
    assert_ne!(held.period, 0);
    assert!(playback.channel_peaks().iter().any(|peak| *peak > 0));

    playback.start(&module, 0, 0);
    assert_eq!(playback.order(), 0);
    assert_eq!(playback.row(), 0);
    assert_eq!(playback.tick(), 0);
    assert_eq!(playback.speed(), DEFAULT_SPEED);
    assert_eq!(playback.tempo(), DEFAULT_TEMPO);
    assert!(!playback.looped());
    assert!(!playback.is_halted());
    assert!(playback.is_playing());
    assert_eq!(playback.channel_peaks(), [0; 4]);
    let reset = playback.channel(0).unwrap();
    assert!(!reset.active);
    assert_eq!(reset.sample, 0);
    assert_eq!(reset.period, 0);
    assert_eq!(reset.volume, 0);
    assert_eq!(reset.position, 0);
    assert!(playback.channel(1).unwrap().muted);
    assert!(!playback.channel(0).unwrap().muted);

    // The spent E61 count must not stick. Two ticks from a fresh start jump
    // back to the loop point, the same as a player that never ran the loop.
    for _ in 0..2 {
        tick(&mut playback, &module);
    }
    assert_eq!(playback.row(), 0);
    assert_eq!(playback.order(), 0);
    assert_eq!(playback.channel(0).unwrap().period, 381);
    let fresh = play(&module, 8_000, 2);
    assert_eq!(playback.row(), fresh.row());
    assert_eq!(
        playback.channel(0).unwrap().position,
        fresh.channel(0).unwrap().position
    );

    module.song_length = 1;
    let mut until_end = start(&module, 8_000);
    until_end.set_stop_on_loop(true);
    for _ in 0..80 {
        if until_end.looped() {
            break;
        }
        tick(&mut until_end, &module);
    }
    assert!(until_end.looped());
    let mut silence = vec![9i16; 8];
    assert_eq!(until_end.render(&module, &mut silence), 0);
    assert_eq!(silence, [0, 0, 0, 0, 0, 0, 0, 0]);
    until_end.start(&module, 0, 0);
    assert!(!until_end.looped());
    assert_eq!(until_end.order(), 0);
    assert_eq!(until_end.row(), 0);
    tick(&mut until_end, &module);
    assert!(until_end.is_playing());
    assert_eq!(until_end.row(), 1);
    assert!(until_end.channel(0).unwrap().active);
}

#[test]
fn pattern_loop_plays_the_section_one_extra_time_per_count() {
    let mut module = tone(428, 0xE, 0x60);
    module.patterns[0].rows[0][3] = Cell {
        sample: 0,
        period: 0,
        effect: 0xF,
        param: 0x01,
    };
    module.patterns[0].rows[1][0] = Cell {
        sample: 1,
        period: 381,
        effect: 0xE,
        param: 0x61,
    };
    let first = play(&module, 100, 1);
    assert_eq!(first.row(), 1);
    assert_eq!(first.channel(0).unwrap().period, 428);
    let second = play(&module, 100, 2);
    assert_eq!(second.row(), 0, "E61 jumps back the first time");
    assert_eq!(second.channel(0).unwrap().period, 381);
    let third = play(&module, 100, 3);
    assert_eq!(third.row(), 1);
    let fourth = play(&module, 100, 4);
    assert_eq!(fourth.row(), 2, "the loop count is spent");
}

#[test]
fn retrigger_cut_delay_and_pattern_delay() {
    let mut retrig = tone(428, 0xE, 0x93);
    retrig.samples[0].loop_length = 1;
    retrig.samples[0].set_data(vec![96; 4000]).unwrap();
    let first = play(&retrig, 100, 1);
    let fourth = play(&retrig, 100, 4);
    assert_eq!(
        first.channel(0).unwrap().position,
        fourth.channel(0).unwrap().position
    );
    assert_ne!(
        play(&retrig, 100, 2).channel(0).unwrap().position,
        first.channel(0).unwrap().position
    );

    let cut = tone(428, 0xE, 0xC1);
    assert_eq!(play(&cut, 100, 1).channel(0).unwrap().volume, 64);
    assert_eq!(play(&cut, 100, 2).channel(0).unwrap().volume, 0);

    let delay = tone(428, 0xE, 0xD2);
    assert_eq!(play(&delay, 100, 1).channel(0).unwrap().period, 0);
    assert_eq!(play(&delay, 100, 2).channel(0).unwrap().period, 0);
    assert_eq!(play(&delay, 100, 3).channel(0).unwrap().period, 428);

    let mut held = tone(428, 0xE, 0xE1);
    held.patterns[0].rows[1][0] = Cell {
        sample: 1,
        period: 339,
        effect: 0,
        param: 0,
    };
    assert_eq!(play(&held, 100, 11).row(), 0);
    let at_boundary = play(&held, 100, 12);
    assert_eq!(at_boundary.row(), 1);
    assert_eq!(at_boundary.channel(0).unwrap().period, 428);
    assert_eq!(play(&held, 100, 13).channel(0).unwrap().period, 339);
}

#[test]
fn set_finetune_affects_the_next_note() {
    let mut module = tone(428, 0xE, 0x51);
    module.patterns[0].rows[0][3] = Cell {
        sample: 0,
        period: 0,
        effect: 0xF,
        param: 0x01,
    };
    module.patterns[0].rows[1][0] = Cell {
        sample: 0,
        period: 428,
        effect: 0,
        param: 0,
    };
    let first = play(&module, 100, 1);
    assert_eq!(first.channel(0).unwrap().period, 428);
    assert_eq!(first.channel(0).unwrap().finetune, 1);
    assert_eq!(play(&module, 100, 2).channel(0).unwrap().period, 425);
}

#[test]
fn halt_and_ignored_effects_are_explicit() {
    let halt = tone(428, 0xF, 0x00);
    let mut playback = start(&halt, 100);
    tick(&mut playback, &halt);
    assert!(playback.is_halted());
    let mut buffer = vec![9i16; 4];
    assert_eq!(playback.render(&halt, &mut buffer), 0);
    assert_eq!(buffer, [0, 0, 0, 0]);

    let ignored = tone(428, 0x8, 0x80);
    let playback = play(&ignored, 100, 2);
    assert_eq!(playback.channel(0).unwrap().period, 428);
    let funk = tone(428, 0xE, 0xF1);
    let playback = play(&funk, 100, 2);
    assert_eq!(playback.channel(0).unwrap().period, 428);
}

#[test]
fn showcase_plays_once_and_the_scale_is_in_the_left_channel() {
    let module = showcase();
    let rate = 44_100u32;
    let mut playback = Playback::new(PlayerConfig::default());
    playback.set_stop_on_loop(true);
    playback.start(&module, 0, 0);
    let mut pcm = Vec::new();
    let chunk = 2048;
    loop {
        let mut buffer = vec![0i16; chunk * 2];
        let wrote = playback.render(&module, &mut buffer);
        if wrote == 0 {
            break;
        }
        pcm.extend_from_slice(&buffer[..wrote * 2]);
    }
    let frames = pcm.len() / 2;
    // 64 rows of pattern 0, then rows 0..=16 of pattern 1, 6 ticks, 882 frames.
    assert_eq!(frames, 81 * 6 * 882);
    assert!(playback.looped());
    assert!(!playback.is_halted());

    let row = 6 * 882;
    assert!(
        band_power(&pcm, rate, 0, row, 517.95) > band_power(&pcm, rate, 0, row, 200.0) * 8.0,
        "C-1 should dominate the first row"
    );
    let second = 2 * row;
    assert!(
        band_power(&pcm, rate, second, row, 581.84)
            > band_power(&pcm, rate, second, row, 200.0) * 8.0,
        "D-1 should dominate row 2"
    );

    let bytes = wav_bytes(rate, &pcm[..64]).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), rate);
}

#[test]
fn channel_peaks_follow_mute_and_volume() {
    let module = tone(214, 0, 0);
    let mut playback = start(&module, 8_000);
    let frames = samples_per_tick(playback.sample_rate(), playback.tempo()) as usize;
    let mut pcm = vec![0i16; frames * 2];
    playback.render(&module, &mut pcm);
    let loud = playback.channel_peaks();
    assert!(loud[0] > 100, "{loud:?}");
    assert_eq!(loud[1], 0);
    assert_eq!(loud[2], 0);
    assert_eq!(loud[3], 0);

    playback.set_mute(0, true);
    playback.render(&module, &mut pcm);
    assert_eq!(playback.channel_peaks(), [0; 4]);

    let silenced = tone(214, 0x0C, 0x00);
    let mut playback = start(&silenced, 8_000);
    playback.render(&silenced, &mut pcm);
    assert_eq!(
        playback.channel_peaks(),
        [0; 4],
        "C00 clears the channel before it is mixed"
    );
}

#[test]
fn preview_plays_one_sample_through_the_mixer() {
    let module = tone(428, 0, 0);
    let pcm = omatrack::player::render_preview(&module, 1, 428, 0, 8_000, 2_000);
    assert_eq!(pcm.len(), 4_000);
    assert!(pcm.iter().any(|sample| *sample != 0));
    let silent = omatrack::player::render_preview(&module, 2, 428, 0, 8_000, 200);
    assert!(silent.iter().all(|sample| *sample == 0));
    let no_note = omatrack::player::render_preview(&module, 1, 0, 0, 8_000, 200);
    assert!(no_note.iter().all(|sample| *sample == 0));
}

/// Goertzel power of the left channel over `frames` starting at `start`.
fn band_power(pcm: &[i16], rate: u32, start: usize, frames: usize, freq: f64) -> f64 {
    let end = start + frames;
    let n = frames as f64;
    let k = (freq * n / f64::from(rate)).round();
    let w = 2.0 * std::f64::consts::PI * k / n;
    let coeff = 2.0 * w.cos();
    let mut s1 = 0.0;
    let mut s2 = 0.0;
    for frame in start..end {
        let x = f64::from(pcm[frame * 2]);
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}
