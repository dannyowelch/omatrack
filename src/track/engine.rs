//! XM and IT replayer.
//!
//! The ProTracker mixer is not used here, so `.mod` output stays on its own
//! path. Timing matches that mixer: `sample_rate * 5 / (tempo * 2)` frames
//! per tick. XM and IT panning is the panning stored in the file.

use std::path::Path;

use super::pitch::{self, period_hz, shift_semitones, SEMITONE};
use super::song::{
    Cell, Envelope, Format, Instrument, LoopKind, NewNoteAction, Sample, Song, NOTE_CUT, NOTE_FADE,
    NOTE_OFF,
};
use crate::error::Error;
use crate::player::{samples_per_tick, Interpolation, PlayerConfig, RenderStats};
use crate::wav::write_wav;

const MAX_VOICES: usize = 96;
const FP: u32 = 16;
const FADE_FULL: i32 = 32768;

/// XM / IT playback state. The song is borrowed on each call.
#[derive(Debug, Clone)]
pub struct Playback {
    config: PlayerConfig,
    stop_on_loop: bool,
    started: bool,
    halted: bool,
    wrapped: bool,
    in_delay: bool,
    order: usize,
    row: usize,
    tick: u8,
    speed: u8,
    tempo: u8,
    global_volume: i32,
    global_volume_max: i32,
    samples_left: u32,
    pattern_delay: u8,
    pending_break: Option<u16>,
    pending_jump: Option<u16>,
    visited: [bool; 256],
    voices: Vec<Voice>,
    channels: usize,
    peaks: Vec<u16>,
}

#[derive(Debug, Clone)]
struct Voice {
    channel: usize,
    background: bool,
    active: bool,
    muted: bool,
    sample: Option<usize>,
    instrument: Option<usize>,
    note: u8,
    pos: u64,
    step: u64,
    forward: bool,
    volume: u8,
    channel_volume: u8,
    pan: u16,
    period: i32,
    porta_target: i32,
    c5: u32,
    linear: bool,
    key_off: bool,
    fade: i32,
    fade_rate: u16,
    vol_env: u16,
    pan_env: u16,
    pitch_env: u16,
    vol_env_on: bool,
    pan_env_on: bool,
    pitch_env_on: bool,
    vib_sweep: u16,
    auto_pos: u8,
    loop_row: u16,
    loop_count: u8,
    effect: u8,
    effect_param: u8,
    slide_up: u8,
    slide_down: u8,
    porta_speed: u8,
    vol_slide: u8,
    vibrato_speed: u8,
    vibrato_depth: u8,
    vibrato_pos: u8,
    vibrato_wave: u8,
    tremolo_speed: u8,
    tremolo_depth: u8,
    tremolo_pos: u8,
    tremolo_wave: u8,
    fine_vib_depth: u8,
    offset: u8,
    high_offset: u8,
    retrig: u8,
    glissando: bool,
    vol_column: u8,
    vol_column_on: bool,
    arp_semitone: i32,
    vib_delta: i32,
    trem_delta: i32,
    pending: Option<Pending>,
    tremor_param: u8,
    tremor_on: bool,
    pan_slide: u8,
    gvol_slide: u8,
    ch_vol_slide: u8,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    tick: u8,
    cell: Cell,
}

impl Voice {
    fn primary(channel: usize, pan: u16, channel_volume: u8, muted: bool) -> Self {
        Self {
            channel,
            background: false,
            active: false,
            muted,
            sample: None,
            instrument: None,
            note: 0,
            pos: 0,
            step: 0,
            forward: true,
            volume: 0,
            channel_volume,
            pan,
            period: 0,
            porta_target: 0,
            c5: 8363,
            linear: true,
            key_off: false,
            fade: FADE_FULL,
            fade_rate: 0,
            vol_env: 0,
            pan_env: 0,
            pitch_env: 0,
            vol_env_on: false,
            pan_env_on: false,
            pitch_env_on: false,
            vib_sweep: 0,
            auto_pos: 0,
            loop_row: 0,
            loop_count: 0,
            effect: 0,
            effect_param: 0,
            slide_up: 0,
            slide_down: 0,
            porta_speed: 0,
            vol_slide: 0,
            vibrato_speed: 0,
            vibrato_depth: 0,
            vibrato_pos: 0,
            vibrato_wave: 0,
            tremolo_speed: 0,
            tremolo_depth: 0,
            tremolo_pos: 0,
            tremolo_wave: 0,
            fine_vib_depth: 0,
            offset: 0,
            high_offset: 0,
            retrig: 0,
            glissando: false,
            vol_column: 0,
            vol_column_on: false,
            arp_semitone: 0,
            vib_delta: 0,
            trem_delta: 0,
            pending: None,
            tremor_param: 0,
            tremor_on: true,
            pan_slide: 0,
            gvol_slide: 0,
            ch_vol_slide: 0,
        }
    }
}

impl Playback {
    /// A player that has not started.
    pub fn new(config: PlayerConfig) -> Self {
        Self {
            config,
            stop_on_loop: false,
            started: false,
            halted: false,
            wrapped: false,
            in_delay: false,
            order: 0,
            row: 0,
            tick: 0,
            speed: 6,
            tempo: 125,
            global_volume: 64,
            global_volume_max: 64,
            samples_left: 0,
            pattern_delay: 0,
            pending_break: None,
            pending_jump: None,
            visited: [false; 256],
            voices: Vec::new(),
            channels: 0,
            peaks: Vec::new(),
        }
    }

    /// Stop [`Self::render`] when the order list repeats itself.
    pub fn set_stop_on_loop(&mut self, stop: bool) {
        self.stop_on_loop = stop;
    }

    /// Silence one pattern channel, or bring it back. Background notes on that
    /// channel follow the same mute.
    pub fn set_mute(&mut self, channel: usize, muted: bool) {
        for voice in &mut self.voices {
            if voice.channel == channel {
                voice.muted = muted;
            }
        }
    }

    /// Start at `order` / `row`. Channel mutes are the initial file mutes
    /// until [`Self::set_mute`] changes them.
    pub fn start(&mut self, song: &Song, order: usize, row: usize) {
        let channels = song.channel_count();
        let mutes: Vec<bool> = (0..channels)
            .map(|channel| song.initial_mute.get(channel).copied().unwrap_or(false))
            .collect();
        let preserved: Vec<bool> = if self.voices.len() == channels {
            self.voices.iter().map(|voice| voice.muted).collect()
        } else {
            mutes
        };
        self.voices = (0..channels)
            .map(|channel| {
                let pan = u16::from(song.initial_pan.get(channel).copied().unwrap_or(128));
                let ch_vol = song
                    .initial_channel_volume
                    .get(channel)
                    .copied()
                    .unwrap_or(64)
                    .min(64);
                Voice::primary(
                    channel,
                    pan,
                    ch_vol,
                    preserved.get(channel).copied().unwrap_or(false),
                )
            })
            .collect();
        self.channels = channels;
        self.peaks = vec![0; channels];
        self.order = order.min(song.order_len().saturating_sub(1));
        self.row = row;
        self.tick = 0;
        self.speed = song.initial_speed.max(1);
        self.tempo = song.initial_tempo.max(1);
        self.global_volume = i32::from(song.initial_global_volume);
        self.global_volume_max = i32::from(song.global_volume_max.max(1));
        self.samples_left = 0;
        self.pattern_delay = 0;
        self.pending_break = None;
        self.pending_jump = None;
        self.visited = [false; 256];
        self.visited[self.order.min(255)] = true;
        self.started = true;
        self.halted = false;
        self.wrapped = false;
        self.in_delay = false;
    }

    /// Fill `output` with interleaved stereo frames.
    ///
    /// The return value is how many frames came from the song. The rest of the
    /// buffer is silence because playback halted or the song looped.
    pub fn render(&mut self, song: &Song, output: &mut [i16]) -> usize {
        let frames = output.len() / 2;
        if frames == 0 {
            return 0;
        }
        self.peaks.fill(0);
        let mut filled = 0usize;
        while filled < frames {
            if self.samples_left == 0 {
                if !self.started || self.halted || (self.stop_on_loop && self.wrapped) {
                    output[filled * 2..].fill(0);
                    return filled;
                }
                self.begin_tick(song);
            }
            let n = (frames - filled).min(self.samples_left as usize);
            let end = (filled + n) * 2;
            self.mix(song, &mut output[filled * 2..end]);
            filled += n;
            self.samples_left -= u32::try_from(n).unwrap_or(u32::MAX);
        }
        filled
    }

    /// Peak `|sample × volume|` per pattern channel over the last render.
    ///
    /// The scale matches [`crate::player::CHANNEL_PEAK_SCALE`]: a full-scale
    /// 8-bit sample at volume 64 is 8192. 16-bit samples use the same range.
    pub fn channel_peaks(&self) -> &[u16] {
        &self.peaks
    }

    /// Order-list position.
    pub fn order(&self) -> usize {
        self.order
    }

    /// Row inside the current pattern.
    pub fn row(&self) -> usize {
        self.row
    }

    /// Ticks per row.
    pub fn speed(&self) -> u8 {
        self.speed
    }

    /// Tempo.
    pub fn tempo(&self) -> u8 {
        self.tempo
    }

    /// Pattern number at the current order, or 0 when the order does not name one.
    pub fn pattern_index(&self, song: &Song) -> usize {
        song.order_pattern(self.order).unwrap_or(0)
    }

    /// Playback stopped on a speed or tempo of 0, or an explicit halt.
    pub fn is_halted(&self) -> bool {
        self.halted
    }

    /// The order list wrapped or jumped somewhere it had already played.
    pub fn looped(&self) -> bool {
        self.wrapped
    }

    fn begin_tick(&mut self, song: &Song) {
        for voice in &mut self.voices {
            voice.arp_semitone = 0;
            voice.vib_delta = 0;
            voice.trem_delta = 0;
        }
        if self.tick == 0 && !self.in_delay {
            self.process_row(song);
        } else if self.tick != 0 {
            self.process_mid(song);
        }
        self.tick_envelopes(song);
        self.update_steps();
        self.advance_clock(song);
        self.samples_left = samples_per_tick(self.config.sample_rate, self.tempo).max(1);
    }

    fn process_row(&mut self, song: &Song) {
        let pattern = song.order_pattern(self.order);
        let channels = self.channels;
        for channel in 0..channels {
            let cell = pattern
                .and_then(|index| song.cell(index, self.row, channel))
                .unwrap_or_else(Cell::empty);
            self.fire_cell(song, channel, cell);
        }
    }

    fn fire_cell(&mut self, song: &Song, channel: usize, cell: Cell) {
        if let Some(delay) = note_delay(song.format, &cell) {
            if delay > 0 {
                if let Some(voice) = self.voices.get_mut(channel) {
                    voice.pending = Some(Pending { tick: delay, cell });
                    voice.effect = cell.effect;
                    voice.effect_param = cell.param;
                }
                return;
            }
        }
        self.apply_cell(song, channel, cell);
    }

    fn apply_cell(&mut self, song: &Song, channel: usize, cell: Cell) {
        let porta = is_porta(song.format, &cell);
        self.trigger_note(song, channel, &cell, porta);
        if let Some(voice) = self.voices.get_mut(channel) {
            voice.vol_column = cell.volume;
            voice.vol_column_on = cell.has_volume;
            apply_volume_column(song.format, voice, &cell, 0);
            voice.effect = cell.effect;
            voice.effect_param = cell.param;
        }
        self.effect_tick0(song, channel, &cell);
    }

    fn trigger_note(&mut self, song: &Song, channel: usize, cell: &Cell, porta: bool) {
        if cell.note == 0 && cell.instrument == 0 {
            return;
        }
        if cell.note == NOTE_OFF || cell.note == NOTE_FADE || cell.note == NOTE_CUT {
            if let Some(voice) = self.voices.get_mut(channel) {
                match cell.note {
                    NOTE_CUT => {
                        voice.volume = 0;
                        voice.active = false;
                    }
                    NOTE_FADE => {
                        voice.key_off = true;
                        if voice.fade_rate == 0 {
                            voice.fade_rate = 128;
                        }
                    }
                    _ => voice.key_off = true,
                }
            }
            return;
        }
        let instrument_index = if cell.instrument > 0 {
            Some(usize::from(cell.instrument - 1))
        } else {
            self.voices.get(channel).and_then(|voice| voice.instrument)
        };
        let Some(instrument_index) = instrument_index else {
            return;
        };
        let Some(instrument) = song.instruments.get(instrument_index) else {
            return;
        };
        if cell.note == 0 {
            if let Some(voice) = self.voices.get_mut(channel) {
                voice.instrument = Some(instrument_index);
                if let Some(sample_index) = voice.sample {
                    if let Some(sample) = song.samples.get(sample_index) {
                        voice.volume = sample.volume.min(64);
                    }
                }
            }
            return;
        }
        let semitone = cell.note - 1;
        let Some((sample_no, mapped)) = instrument.map_note(semitone) else {
            return;
        };
        let sample_index = usize::from(sample_no.saturating_sub(1));
        let Some(sample) = song.samples.get(sample_index) else {
            return;
        };
        self.duplicate_check(channel, instrument, sample_index, mapped);
        self.apply_nna(channel, instrument.nna);
        let period = period_of(song, sample, i32::from(mapped));
        let voice = &mut self.voices[channel];
        voice.instrument = Some(instrument_index);
        voice.sample = Some(sample_index);
        voice.note = mapped;
        voice.c5 = if sample.c5_speed == 0 {
            8363
        } else {
            sample.c5_speed
        };
        voice.linear = song.linear;
        voice.fade_rate = instrument.fadeout;
        voice.vol_env_on = instrument.volume_env.enabled;
        voice.pan_env_on = instrument.pan_env.enabled;
        voice.pitch_env_on = instrument.pitch_env.enabled && !instrument.pitch_env.filter;
        if let Some(pan) = instrument.pan.or(sample.pan) {
            voice.pan = u16::from(pan);
        }
        if porta && voice.period != 0 {
            voice.porta_target = period;
            voice.volume = sample.volume.min(64);
            return;
        }
        voice.period = period;
        voice.porta_target = period;
        voice.volume = sample.volume.min(64);
        voice.pos = 0;
        voice.forward = true;
        voice.active = !sample.pcm.is_empty();
        voice.key_off = false;
        voice.fade = FADE_FULL;
        voice.vol_env = 0;
        voice.pan_env = 0;
        voice.pitch_env = 0;
        voice.vib_sweep = 0;
        voice.auto_pos = 0;
        if voice.vibrato_wave & 4 == 0 {
            voice.vibrato_pos = 0;
        }
        if voice.tremolo_wave & 4 == 0 {
            voice.tremolo_pos = 0;
        }
    }

    fn duplicate_check(
        &mut self,
        channel: usize,
        instrument: &Instrument,
        sample_index: usize,
        note: u8,
    ) {
        if instrument.dct == 0 {
            return;
        }
        for voice in &mut self.voices {
            if voice.channel != channel || !voice.background || !voice.active {
                continue;
            }
            let dup = match instrument.dct {
                1 => voice.note == note,
                2 => voice.sample == Some(sample_index),
                3 => voice.instrument.is_some(),
                _ => false,
            };
            if !dup {
                continue;
            }
            match instrument.dca {
                1 => voice.key_off = true,
                2 => {
                    voice.key_off = true;
                    if voice.fade_rate == 0 {
                        voice.fade_rate = 256;
                    }
                }
                _ => {
                    voice.active = false;
                    voice.volume = 0;
                }
            }
        }
    }

    fn apply_nna(&mut self, channel: usize, nna: NewNoteAction) {
        if nna == NewNoteAction::Cut || self.voices.len() >= MAX_VOICES {
            return;
        }
        let Some(primary) = self.voices.get(channel) else {
            return;
        };
        if !primary.active {
            return;
        }
        let mut background = primary.clone();
        background.background = true;
        match nna {
            NewNoteAction::Off => background.key_off = true,
            NewNoteAction::Fade => {
                background.key_off = true;
                if background.fade_rate == 0 {
                    background.fade_rate = 256;
                }
            }
            _ => {}
        }
        self.voices.push(background);
    }

    fn effect_tick0(&mut self, song: &Song, channel: usize, cell: &Cell) {
        match song.format {
            Format::Xm => self.xm_tick0(channel, cell),
            Format::It => self.it_tick0(song, channel, cell),
        }
    }

    fn xm_tick0(&mut self, channel: usize, cell: &Cell) {
        let param = cell.param;
        match cell.effect {
            0x0B => self.pending_jump = Some(u16::from(param)),
            0x0D => self.pending_break = Some(bcd_row(param)),
            0x0E => self.xm_extended_tick0(channel, param),
            0x0F => self.set_speed_tempo(param),
            0x10 => self.global_volume = i32::from(param.min(64)),
            0x14 => {
                if let Some(voice) = self.voices.get_mut(channel) {
                    voice.key_off = true;
                }
            }
            0x15 => {
                if let Some(voice) = self.voices.get_mut(channel) {
                    voice.vol_env = u16::from(param);
                }
            }
            0x21 => self.xm_extra_fine(channel, param),
            effect => {
                let Some(voice) = self.voices.get_mut(channel) else {
                    return;
                };
                match effect {
                    0x01 => voice.slide_up = remember(param, voice.slide_up),
                    0x02 => voice.slide_down = remember(param, voice.slide_down),
                    0x03 => voice.porta_speed = remember(param, voice.porta_speed),
                    0x04 => remember_xy(param, &mut voice.vibrato_speed, &mut voice.vibrato_depth),
                    0x05 | 0x06 | 0x0A => voice.vol_slide = remember(param, voice.vol_slide),
                    0x07 => remember_xy(param, &mut voice.tremolo_speed, &mut voice.tremolo_depth),
                    0x08 => voice.pan = u16::from(param),
                    0x09 => {
                        voice.offset = remember(param, voice.offset);
                        if cell.note > 0 && cell.note < NOTE_FADE {
                            apply_offset(voice);
                        }
                    }
                    0x0C => voice.volume = param.min(64),
                    0x11 => voice.gvol_slide = remember(param, voice.gvol_slide),
                    0x19 => voice.pan_slide = remember(param, voice.pan_slide),
                    0x1B => voice.retrig = remember(param, voice.retrig),
                    0x1D => voice.tremor_param = remember(param, voice.tremor_param),
                    _ => {}
                }
            }
        }
    }

    fn xm_extended_tick0(&mut self, channel: usize, param: u8) {
        let nibble = param & 0x0F;
        match param >> 4 {
            0x6 => self.pattern_loop(channel, nibble),
            0xE => self.pattern_delay = nibble,
            command => {
                let Some(voice) = self.voices.get_mut(channel) else {
                    return;
                };
                match command {
                    0x1 => {
                        let amount = remember_nibble(nibble, &mut voice.slide_up);
                        voice.period = (voice.period - i32::from(amount) * 4).max(1);
                    }
                    0x2 => {
                        let amount = remember_nibble(nibble, &mut voice.slide_down);
                        voice.period += i32::from(amount) * 4;
                    }
                    0x3 => voice.glissando = nibble != 0,
                    0x4 => voice.vibrato_wave = nibble,
                    0x7 => voice.tremolo_wave = nibble,
                    0x9 => voice.retrig = remember_nibble(nibble, &mut voice.retrig),
                    0xA => voice.volume = voice.volume.saturating_add(nibble).min(64),
                    0xB => voice.volume = voice.volume.saturating_sub(nibble),
                    0xC if nibble == 0 => voice.volume = 0,
                    _ => {}
                }
            }
        }
    }

    fn xm_extra_fine(&mut self, channel: usize, param: u8) {
        let nibble = param & 0x0F;
        let Some(voice) = self.voices.get_mut(channel) else {
            return;
        };
        match param >> 4 {
            0x1 => voice.period = (voice.period - i32::from(nibble)).max(1),
            0x2 => voice.period += i32::from(nibble),
            _ => {}
        }
    }

    fn it_tick0(&mut self, song: &Song, channel: usize, cell: &Cell) {
        let param = cell.param;
        match cell.effect {
            1 => {
                if param > 0 {
                    self.speed = param;
                }
            }
            2 => self.pending_jump = Some(u16::from(param)),
            3 => self.pending_break = Some(bcd_row(param)),
            19 => self.it_extended_tick0(channel, param),
            20 => self.it_tempo(param),
            22 => self.global_volume = i32::from(param.min(128)),
            effect => {
                let linked = song.compatible_gxx;
                let Some(voice) = self.voices.get_mut(channel) else {
                    return;
                };
                match effect {
                    4 => voice.vol_slide = remember(param, voice.vol_slide),
                    5 => {
                        voice.slide_down = remember(param, voice.slide_down);
                        if linked {
                            voice.porta_speed = voice.slide_down;
                        }
                    }
                    6 => {
                        voice.slide_up = remember(param, voice.slide_up);
                        if linked {
                            voice.porta_speed = voice.slide_up;
                        }
                    }
                    7 => {
                        voice.porta_speed = remember(param, voice.porta_speed);
                        if linked && voice.porta_speed != 0 {
                            voice.slide_up = voice.porta_speed;
                        }
                    }
                    8 => remember_xy(param, &mut voice.vibrato_speed, &mut voice.vibrato_depth),
                    9 => voice.tremor_param = remember(param, voice.tremor_param),
                    10 => {}
                    11 => {
                        remember_xy(param, &mut voice.vibrato_speed, &mut voice.vibrato_depth);
                        voice.vol_slide = remember(param, voice.vol_slide);
                    }
                    12 => {
                        voice.porta_speed = remember(param, voice.porta_speed);
                        voice.vol_slide = remember(param, voice.vol_slide);
                    }
                    13 => voice.channel_volume = param.min(64),
                    14 => voice.ch_vol_slide = remember(param, voice.ch_vol_slide),
                    15 => {
                        voice.offset = remember(param, voice.offset);
                        if cell.note > 0 && cell.note < NOTE_FADE {
                            apply_offset(voice);
                        }
                    }
                    16 => voice.pan_slide = remember(param, voice.pan_slide),
                    17 => voice.retrig = remember(param, voice.retrig),
                    18 => remember_xy(param, &mut voice.tremolo_speed, &mut voice.tremolo_depth),
                    21 => remember_xy(param, &mut voice.vibrato_speed, &mut voice.fine_vib_depth),
                    23 => voice.gvol_slide = remember(param, voice.gvol_slide),
                    24 => voice.pan = u16::from(param),
                    _ => {}
                }
            }
        }
    }

    fn it_extended_tick0(&mut self, channel: usize, param: u8) {
        let nibble = param & 0x0F;
        match param >> 4 {
            0x6 => self.pattern_delay = nibble,
            0x8 => {
                if let Some(voice) = self.voices.get_mut(channel) {
                    voice.pan = u16::from(nibble) * 17;
                }
            }
            0xB => self.pattern_loop(channel, nibble),
            0xC if nibble == 0 => {
                if let Some(voice) = self.voices.get_mut(channel) {
                    voice.volume = 0;
                    voice.active = false;
                }
            }
            0xE => self.pattern_delay = nibble,
            0x7 => self.it_s7(channel, nibble),
            0x3 => {
                if let Some(voice) = self.voices.get_mut(channel) {
                    voice.vibrato_wave = nibble;
                }
            }
            0x4 => {
                if let Some(voice) = self.voices.get_mut(channel) {
                    voice.tremolo_wave = nibble;
                }
            }
            0xA => {
                if let Some(voice) = self.voices.get_mut(channel) {
                    voice.high_offset = nibble;
                }
            }
            _ => {}
        }
    }

    fn it_s7(&mut self, channel: usize, nibble: u8) {
        let Some(voice) = self.voices.get_mut(channel) else {
            return;
        };
        match nibble {
            0x0 | 0x3 => {
                voice.active = false;
                voice.volume = 0;
            }
            0x1 | 0x5 => voice.key_off = true,
            0x2 | 0x6 => {
                voice.key_off = true;
                if voice.fade_rate == 0 {
                    voice.fade_rate = 256;
                }
            }
            0x7 => voice.vol_env_on = false,
            0x8 => voice.vol_env_on = true,
            0x9 => voice.pan_env_on = false,
            0xA => voice.pan_env_on = true,
            0xB => voice.pitch_env_on = false,
            0xC => voice.pitch_env_on = true,
            _ => {}
        }
    }

    fn it_tempo(&mut self, param: u8) {
        match param {
            0x00 => {}
            0x01..=0x0F => self.tempo = self.tempo.saturating_sub(param).max(32),
            0x10..=0x1F => {
                self.tempo = self.tempo.saturating_add(param - 0x10);
            }
            other => self.tempo = other,
        }
    }

    fn pattern_loop(&mut self, channel: usize, nibble: u8) {
        let Some(voice) = self.voices.get_mut(channel) else {
            return;
        };
        if nibble == 0 {
            voice.loop_row = u16::try_from(self.row).unwrap_or(0);
            return;
        }
        if voice.loop_count == 0 {
            voice.loop_count = nibble;
            self.pending_break = Some(voice.loop_row);
            self.pending_jump = Some(u16::try_from(self.order).unwrap_or(0));
        } else {
            voice.loop_count -= 1;
            if voice.loop_count != 0 {
                self.pending_break = Some(voice.loop_row);
                self.pending_jump = Some(u16::try_from(self.order).unwrap_or(0));
            }
        }
    }

    fn set_speed_tempo(&mut self, param: u8) {
        if param == 0 {
            self.halted = true;
        } else if param < 0x20 {
            self.speed = param;
        } else {
            self.tempo = param;
        }
    }

    fn process_mid(&mut self, song: &Song) {
        let tick = self.tick;
        let channels = self.channels;
        for channel in 0..channels {
            if let Some(pending) = self.voices.get(channel).and_then(|voice| voice.pending) {
                if pending.tick == tick {
                    self.voices[channel].pending = None;
                    let mut cell = pending.cell;
                    // The delay itself must not arm another delay.
                    if matches!(song.format, Format::Xm) && cell.effect == 0x0E {
                        cell.effect = 0;
                        cell.param = 0;
                    }
                    if matches!(song.format, Format::It)
                        && cell.effect == 19
                        && cell.param >> 4 == 0xD
                    {
                        cell.effect = 0;
                        cell.param = 0;
                    }
                    self.apply_cell(song, channel, cell);
                }
            }
            let effect = self.voices.get(channel).map(|voice| voice.effect);
            let param = self.voices.get(channel).map(|voice| voice.effect_param);
            if let (Some(effect), Some(param)) = (effect, param) {
                if let Some(voice) = self.voices.get_mut(channel) {
                    let column = Cell {
                        volume: voice.vol_column,
                        has_volume: voice.vol_column_on,
                        ..Cell::empty()
                    };
                    apply_volume_column(song.format, voice, &column, tick);
                }
                self.effect_mid(song, channel, effect, param, tick);
            }
        }
        let gslide = self.voices.iter().take(channels).find_map(|voice| {
            let slides = match song.format {
                Format::Xm => voice.effect == 0x11,
                Format::It => voice.effect == 23,
            };
            slides.then_some(voice.gvol_slide)
        });
        if let Some(param) = gslide {
            self.global_volume = slide_value(self.global_volume, param, self.global_volume_max);
        }
    }

    fn effect_mid(&mut self, song: &Song, channel: usize, effect: u8, param: u8, tick: u8) {
        match song.format {
            Format::Xm => self.xm_mid(channel, effect, param, tick),
            Format::It => self.it_mid(channel, effect, param, tick),
        }
    }

    fn xm_mid(&mut self, channel: usize, effect: u8, param: u8, tick: u8) {
        let Some(voice) = self.voices.get_mut(channel) else {
            return;
        };
        match effect {
            0x00 if param != 0 => {
                let semis = match tick % 3 {
                    1 => i32::from(param >> 4),
                    2 => i32::from(param & 0x0F),
                    _ => 0,
                };
                voice.arp_semitone = semis;
            }
            0x01 => voice.period = (voice.period - i32::from(voice.slide_up) * 4).max(1),
            0x02 => voice.period += i32::from(voice.slide_down) * 4,
            0x03 => do_porta(voice),
            0x04 => do_vibrato(voice, false),
            0x05 => {
                do_porta(voice);
                voice.volume = slide_u8(voice.volume, voice.vol_slide, 64);
            }
            0x06 => {
                do_vibrato(voice, false);
                voice.volume = slide_u8(voice.volume, voice.vol_slide, 64);
            }
            0x07 => do_tremolo(voice),
            0x0A => voice.volume = slide_u8(voice.volume, voice.vol_slide, 64),
            0x0E => xm_extended_mid(voice, tick, param),
            0x19 => voice.pan = slide_pan(voice.pan, voice.pan_slide),
            0x1B => do_retrig(voice, tick, voice.retrig),
            0x1D => do_tremor(voice, tick),
            _ => {}
        }
    }

    fn it_mid(&mut self, channel: usize, effect: u8, param: u8, tick: u8) {
        let Some(voice) = self.voices.get_mut(channel) else {
            return;
        };
        match effect {
            4 => voice.volume = it_vol_slide(voice.volume, voice.vol_slide, tick),
            5 => voice.period += i32::from(voice.slide_down) * 4,
            6 => voice.period = (voice.period - i32::from(voice.slide_up) * 4).max(1),
            7 | 12 => do_porta(voice),
            8 | 11 => do_vibrato(voice, false),
            9 => do_tremor(voice, tick),
            10 if param != 0 => {
                let semis = match tick % 3 {
                    1 => i32::from(param >> 4),
                    2 => i32::from(param & 0x0F),
                    _ => 0,
                };
                voice.arp_semitone = semis;
            }
            13 => {}
            14 => {
                voice.channel_volume = it_vol_slide(voice.channel_volume, voice.ch_vol_slide, tick);
            }
            16 => voice.pan = slide_pan(voice.pan, voice.pan_slide),
            17 => do_retrig(voice, tick, voice.retrig),
            18 => do_tremolo(voice),
            19 => it_extended_mid(voice, tick, param),
            21 => do_vibrato(voice, true),
            _ => {}
        }
        if matches!(effect, 11 | 12) {
            voice.volume = it_vol_slide(voice.volume, voice.vol_slide, tick);
        }
    }

    fn tick_envelopes(&mut self, song: &Song) {
        for voice in &mut self.voices {
            let Some(index) = voice.instrument else {
                continue;
            };
            let Some(instrument) = song.instruments.get(index) else {
                continue;
            };
            if voice.vol_env_on {
                voice.vol_env =
                    advance_envelope(&instrument.volume_env, voice.vol_env, voice.key_off);
            }
            if voice.pan_env_on {
                voice.pan_env = advance_envelope(&instrument.pan_env, voice.pan_env, voice.key_off);
            }
            if voice.pitch_env_on && !instrument.pitch_env.filter {
                voice.pitch_env =
                    advance_envelope(&instrument.pitch_env, voice.pitch_env, voice.key_off);
            }
            if voice.key_off && voice.fade_rate > 0 {
                voice.fade = voice.fade.saturating_sub(i32::from(voice.fade_rate) * 2);
            }
            auto_vibrato(voice, instrument, song);
            if voice.tremor_param != 0 {
                // Tremor on/off is applied as a volume gate in the mixer via trem_delta.
            }
        }
    }

    fn update_steps(&mut self) {
        let rate = self.config.sample_rate.max(1);
        for voice in &mut self.voices {
            if !voice.active || voice.period <= 0 {
                voice.step = 0;
                continue;
            }
            let mut period = voice.period;
            if voice.arp_semitone != 0 {
                period = shift_semitones(period, voice.arp_semitone, voice.linear);
            }
            period = (period + voice.vib_delta).max(1);
            let hz = period_hz(period, voice.linear) * f64::from(voice.c5.max(1)) / 8363.0;
            if !hz.is_finite() || hz <= 0.0 {
                voice.step = 0;
                continue;
            }
            let step = hz / f64::from(rate) * f64::from(1u32 << FP);
            voice.step = step.round().clamp(0.0, u64::MAX as f64) as u64;
        }
    }

    fn advance_clock(&mut self, song: &Song) {
        if self.halted {
            return;
        }
        self.tick = self.tick.saturating_add(1);
        if self.tick < self.speed.max(1) {
            return;
        }
        self.tick = 0;
        if self.pattern_delay > 0 {
            self.pattern_delay -= 1;
            self.in_delay = true;
        } else {
            self.in_delay = false;
            self.advance_position(song);
        }
    }

    fn advance_position(&mut self, song: &Song) {
        let had_break = self.pending_break.is_some();
        let had_jump = self.pending_jump.is_some();
        let pattern = song.order_pattern(self.order);
        let rows = pattern
            .map(|index| song.row_count(index))
            .filter(|rows| *rows > 0)
            .unwrap_or(64);
        let mut next_row = self.row.saturating_add(1);
        let mut next_order = self.order;
        if let Some(row) = self.pending_break.take() {
            next_row = usize::from(row);
            if !had_jump {
                next_order = self.order.saturating_add(1);
            }
        }
        if let Some(order) = self.pending_jump.take() {
            next_order = usize::from(order);
            if !had_break {
                next_row = 0;
            }
        }
        if next_row >= rows {
            next_row = 0;
            next_order = next_order.saturating_add(1);
        }
        let len = song.order_len();
        // Skip IT "---" markers.
        let mut guard = 0;
        while song.format == Format::It
            && next_order < len
            && song.orders.get(next_order).copied() == Some(254)
            && guard < 256
        {
            next_order += 1;
            next_row = 0;
            guard += 1;
        }
        let mut wrapped_by_end = false;
        if next_order >= len || song.orders.get(next_order).copied() == Some(255) {
            let restart = usize::from(song.restart);
            next_order = if restart < len { restart } else { 0 };
            next_row = 0;
            wrapped_by_end = true;
        }
        let slot = next_order.min(255);
        let revisiting = wrapped_by_end
            || (next_order != self.order && self.visited[slot])
            || (had_jump && next_order == self.order && !had_break && next_row == 0);
        // A pattern loop jumps to the same order on purpose. `pattern_loop`
        // sets both jump and break; that is not a song-level loop.
        let pattern_loop = had_jump && had_break && next_order == self.order;
        if revisiting && !pattern_loop {
            self.wrapped = true;
        }
        self.visited[slot] = true;
        self.order = next_order.min(len.saturating_sub(1));
        self.row = next_row;
    }

    fn mix(&mut self, song: &Song, output: &mut [i16]) {
        let frames = output.len() / 2;
        let separation = i32::from(self.config.stereo_separation.min(100));
        let interpolation = self.config.interpolation;
        for frame in 0..frames {
            let mut left = 0i32;
            let mut right = 0i32;
            let mut frame_peaks = vec![0u32; self.channels];
            for voice in &mut self.voices {
                if !voice.active || voice.step == 0 || voice.muted {
                    continue;
                }
                let Some(sample_index) = voice.sample else {
                    continue;
                };
                let Some(sample) = song.samples.get(sample_index) else {
                    voice.active = false;
                    continue;
                };
                if sample.pcm.is_empty() {
                    voice.active = false;
                    continue;
                }
                let raw = read_sample(sample, voice, interpolation);
                advance_position(sample, voice);
                if !voice.tremor_on && voice.tremor_param != 0 {
                    continue;
                }
                let env = envelope_level(song, voice);
                let vol = (i32::from(voice.volume) + voice.trem_delta).clamp(0, 64);
                let ch = i32::from(voice.channel_volume.min(64));
                let g = self.global_volume.clamp(0, self.global_volume_max);
                let amp = i64::from(raw)
                    * i64::from(vol)
                    * i64::from(env)
                    * i64::from(ch)
                    * i64::from(g)
                    * i64::from(voice.fade.clamp(0, FADE_FULL))
                    / (64
                        * 64
                        * 64
                        * i64::from(self.global_volume_max.max(1))
                        * i64::from(FADE_FULL));
                let mag = (i64::from(raw).unsigned_abs() * (vol as u64) / 256) as u32;
                if let Some(slot) = frame_peaks.get_mut(voice.channel) {
                    *slot = (*slot).max(mag.min(u32::from(u16::MAX)));
                }
                let pan = voice_pan(song, voice);
                let (l_gain, r_gain) = pan_gains(pan, separation);
                left += (amp * i64::from(l_gain) / 256) as i32;
                right += (amp * i64::from(r_gain) / 256) as i32;
            }
            for (slot, peak) in self.peaks.iter_mut().zip(&frame_peaks) {
                if *peak > u32::from(*slot) {
                    *slot = *peak as u16;
                }
            }
            let base = frame * 2;
            let head = (self.channels as i32).clamp(4, 32);
            output[base] = clamp_i16(left * 4 / head);
            output[base + 1] = clamp_i16(right * 4 / head);
        }
        self.voices
            .retain(|voice| !voice.background || voice.active);
    }
}

fn note_delay(format: Format, cell: &Cell) -> Option<u8> {
    match format {
        Format::Xm if cell.effect == 0x0E && cell.param >> 4 == 0xD => Some(cell.param & 0x0F),
        Format::It if cell.effect == 19 && cell.param >> 4 == 0xD => Some(cell.param & 0x0F),
        _ => None,
    }
}

fn is_porta(format: Format, cell: &Cell) -> bool {
    match format {
        Format::Xm => {
            matches!(cell.effect, 0x03 | 0x05) || (cell.has_volume && cell.volume >= 0xF0)
        }
        Format::It => {
            matches!(cell.effect, 7 | 12) || (cell.has_volume && (193..=202).contains(&cell.volume))
        }
    }
}

fn period_of(song: &Song, sample: &Sample, semitone: i32) -> i32 {
    let mut note = semitone + i32::from(sample.relative_note);
    if song.format == Format::It {
        // IT C-5 is the 8363 Hz reference. The XM tables use C-4 for that.
        note -= 12;
    }
    let finetune = i32::from(sample.finetune);
    if song.linear {
        pitch::linear_period(note, finetune).max(1)
    } else {
        pitch::amiga_period(note + 1, finetune).max(1)
    }
}

fn apply_offset(voice: &mut Voice) {
    let frames = u64::from(voice.offset) * 256 + (u64::from(voice.high_offset) << 16);
    voice.pos = frames << FP;
}

fn apply_volume_column(format: Format, voice: &mut Voice, cell: &Cell, tick: u8) {
    if !cell.has_volume {
        return;
    }
    match format {
        Format::Xm => xm_volume_column(voice, cell.volume, tick),
        Format::It => it_volume_column(voice, cell.volume, tick),
    }
}

fn xm_volume_column(voice: &mut Voice, value: u8, tick: u8) {
    let nibble = value & 0x0F;
    match value {
        0x10..=0x50 if tick == 0 => voice.volume = value - 0x10,
        0x60..=0x6F => {
            if nibble != 0 {
                voice.vol_slide = nibble;
            }
            if tick > 0 {
                voice.volume = voice.volume.saturating_sub(voice.vol_slide & 0x0F);
            }
        }
        0x70..=0x7F => {
            if nibble != 0 {
                voice.vol_slide = nibble << 4;
            }
            if tick > 0 {
                voice.volume = voice.volume.saturating_add(voice.vol_slide >> 4).min(64);
            }
        }
        0x80..=0x8F if tick == 0 => voice.volume = voice.volume.saturating_sub(nibble),
        0x90..=0x9F if tick == 0 => voice.volume = voice.volume.saturating_add(nibble).min(64),
        0xA0..=0xAF if tick == 0 && nibble != 0 => voice.vibrato_speed = nibble,
        0xB0..=0xBF => {
            if nibble != 0 {
                voice.vibrato_depth = nibble;
            }
            if tick > 0 {
                do_vibrato(voice, false);
            }
        }
        0xC0..=0xCF if tick == 0 => voice.pan = u16::from(nibble) * 17,
        0xD0..=0xDF if tick > 0 => {
            voice.pan = voice.pan.saturating_sub(u16::from(nibble) * 4);
        }
        0xE0..=0xEF if tick > 0 => {
            voice.pan = (voice.pan + u16::from(nibble) * 4).min(255);
        }
        0xF0..=0xFF => {
            if nibble != 0 {
                voice.porta_speed = nibble * 16;
            }
            if tick > 0 {
                do_porta(voice);
            }
        }
        _ => {}
    }
}

fn it_volume_column(voice: &mut Voice, value: u8, tick: u8) {
    match value {
        0..=64 if tick == 0 => voice.volume = value,
        65..=74 if tick == 0 => voice.volume = voice.volume.saturating_add(value - 65).min(64),
        75..=84 if tick == 0 => voice.volume = voice.volume.saturating_sub(value - 75),
        85..=94 if tick > 0 => voice.volume = voice.volume.saturating_add(value - 85).min(64),
        95..=104 if tick > 0 => voice.volume = voice.volume.saturating_sub(value - 95),
        105..=114 if tick > 0 => voice.period += i32::from(value - 105) * 4,
        115..=124 if tick > 0 => {
            voice.period = (voice.period - i32::from(value - 115) * 4).max(1);
        }
        128..=192 if tick == 0 => {
            voice.pan = u16::from(value - 128) * 255 / 64;
        }
        193..=202 => {
            let speed = value - 193;
            if speed != 0 {
                voice.porta_speed = speed * 16;
            }
            if tick > 0 {
                do_porta(voice);
            }
        }
        203..=212 => {
            let depth = value - 203;
            if depth != 0 {
                voice.vibrato_depth = depth;
            }
            if tick > 0 {
                do_vibrato(voice, false);
            }
        }
        _ => {}
    }
}

fn do_porta(voice: &mut Voice) {
    if voice.porta_target == 0 || voice.porta_speed == 0 {
        return;
    }
    let step = i32::from(voice.porta_speed) * 4;
    voice.period = match voice.period.cmp(&voice.porta_target) {
        std::cmp::Ordering::Less => (voice.period + step).min(voice.porta_target),
        std::cmp::Ordering::Greater => (voice.period - step).max(voice.porta_target),
        std::cmp::Ordering::Equal => voice.period,
    };
    if voice.glissando {
        let snapped = (voice.period + SEMITONE / 2) / SEMITONE * SEMITONE;
        if (snapped - voice.porta_target).abs() < SEMITONE {
            voice.period = voice.porta_target;
        }
    }
}

fn do_vibrato(voice: &mut Voice, fine: bool) {
    let depth = if fine {
        i32::from(voice.fine_vib_depth)
    } else {
        i32::from(voice.vibrato_depth)
    };
    voice.vibrato_pos = voice.vibrato_pos.wrapping_add(voice.vibrato_speed);
    let wave = lfo(voice.vibrato_pos, voice.vibrato_wave);
    voice.vib_delta = wave * depth / 32;
}

fn do_tremolo(voice: &mut Voice) {
    voice.tremolo_pos = voice.tremolo_pos.wrapping_add(voice.tremolo_speed);
    let wave = lfo(voice.tremolo_pos, voice.tremolo_wave);
    voice.trem_delta = wave * i32::from(voice.tremolo_depth) / 64;
}

fn do_retrig(voice: &mut Voice, tick: u8, param: u8) {
    let interval = param & 0x0F;
    if interval == 0 || tick % interval != 0 {
        return;
    }
    voice.pos = 0;
    voice.forward = true;
    voice.active = voice.sample.is_some();
    let cmd = param >> 4;
    voice.volume = match cmd {
        0 => voice.volume,
        1 => voice.volume.saturating_sub(1),
        2 => voice.volume.saturating_sub(2),
        3 => voice.volume.saturating_sub(4),
        4 => voice.volume.saturating_sub(8),
        5 => voice.volume.saturating_sub(16),
        6 => (u16::from(voice.volume) * 2 / 3) as u8,
        7 => voice.volume / 2,
        9 => voice.volume.saturating_add(1).min(64),
        0xA => voice.volume.saturating_add(2).min(64),
        0xB => voice.volume.saturating_add(4).min(64),
        0xC => voice.volume.saturating_add(8).min(64),
        0xD => voice.volume.saturating_add(16).min(64),
        0xE => (u16::from(voice.volume) * 3 / 2).min(64) as u8,
        0xF => voice.volume.saturating_mul(2).min(64),
        _ => voice.volume,
    };
}

fn do_tremor(voice: &mut Voice, tick: u8) {
    let on = (voice.tremor_param >> 4) + 1;
    let off = (voice.tremor_param & 0x0F) + 1;
    let cycle = on + off;
    if cycle == 0 {
        return;
    }
    voice.tremor_on = tick % cycle < on;
}

fn xm_extended_mid(voice: &mut Voice, tick: u8, param: u8) {
    let nibble = param & 0x0F;
    match param >> 4 {
        0x9 => do_retrig(voice, tick, nibble),
        0xC if tick == nibble => {
            voice.volume = 0;
            voice.active = false;
        }
        _ => {}
    }
}

fn it_extended_mid(voice: &mut Voice, tick: u8, param: u8) {
    let nibble = param & 0x0F;
    if param >> 4 == 0xC && tick == nibble {
        voice.volume = 0;
        voice.active = false;
    }
}

fn auto_vibrato(voice: &mut Voice, instrument: &Instrument, song: &Song) {
    let vib = if song.format == Format::It {
        voice
            .sample
            .and_then(|index| song.samples.get(index))
            .map(|sample| sample.vibrato)
            .unwrap_or(instrument.vibrato)
    } else {
        instrument.vibrato
    };
    if vib.depth == 0 || vib.rate == 0 {
        return;
    }
    if vib.sweep == 0 {
        voice.vib_sweep = u16::from(vib.depth) << 8;
    } else {
        voice.vib_sweep = voice
            .vib_sweep
            .saturating_add(u16::from(vib.sweep))
            .min(u16::from(vib.depth) << 8);
    }
    voice.auto_pos = voice.auto_pos.wrapping_add(vib.rate);
    let depth = i32::from(voice.vib_sweep >> 8);
    let wave = lfo(voice.auto_pos, vib.kind);
    voice.vib_delta += wave * depth / 64;
}

fn lfo(pos: u8, wave: u8) -> i32 {
    let pos = pos & 63;
    match wave & 3 {
        1 => {
            // Ramp down.
            32 - i32::from(pos)
        }
        2 => {
            if pos < 32 {
                48
            } else {
                -48
            }
        }
        3 => {
            let mixed = pos.wrapping_mul(17).wrapping_add(pos << 1);
            i32::from(mixed & 63) - 32
        }
        _ => sine(pos),
    }
}

fn sine(pos: u8) -> i32 {
    const TABLE: [i8; 16] = [
        0, 12, 25, 36, 46, 54, 60, 63, 64, 63, 60, 54, 46, 36, 25, 12,
    ];
    let quarter = usize::from(pos / 16);
    let index = usize::from(pos % 16);
    let mag = i32::from(TABLE[index]);
    match quarter {
        0 => mag,
        1 => i32::from(TABLE[15 - index]),
        2 => -mag,
        _ => -i32::from(TABLE[15 - index]),
    }
}

fn advance_envelope(env: &Envelope, pos: u16, key_off: bool) -> u16 {
    if !env.enabled || env.points.is_empty() {
        return pos;
    }
    if env.sustain && !key_off {
        let end = node_tick(env, env.sustain_end);
        let start = node_tick(env, env.sustain_point);
        if pos >= end {
            if end > start {
                return start + (pos - start) % (end - start).max(1);
            }
            return end;
        }
    }
    let mut next = pos.saturating_add(1);
    if env.loop_on {
        let start = node_tick(env, env.loop_start);
        let end = node_tick(env, env.loop_end);
        if next > end && end > start {
            next = start + (next - start) % (end - start).max(1);
        }
    }
    next
}

fn node_tick(env: &Envelope, index: u8) -> u16 {
    env.points
        .get(usize::from(index))
        .map(|point| point.0)
        .unwrap_or(0)
}

fn envelope_level(song: &Song, voice: &Voice) -> i32 {
    let Some(index) = voice.instrument else {
        return 64;
    };
    let Some(instrument) = song.instruments.get(index) else {
        return 64;
    };
    if !voice.vol_env_on || !instrument.volume_env.enabled {
        return 64;
    }
    i32::from(sample_envelope(&instrument.volume_env, voice.vol_env))
}

fn sample_envelope(env: &Envelope, tick: u16) -> u8 {
    if env.points.is_empty() {
        return 64;
    }
    if tick <= env.points[0].0 {
        return env.points[0].1;
    }
    for pair in env.points.windows(2) {
        if tick <= pair[1].0 {
            let span = i32::from(pair[1].0 - pair[0].0);
            if span <= 0 {
                return pair[1].1;
            }
            let along = i32::from(tick - pair[0].0);
            let value =
                i32::from(pair[0].1) + (i32::from(pair[1].1) - i32::from(pair[0].1)) * along / span;
            return value.clamp(0, 64) as u8;
        }
    }
    env.points.last().map(|point| point.1).unwrap_or(64)
}

fn voice_pan(song: &Song, voice: &Voice) -> u16 {
    let mut pan = i32::from(voice.pan);
    if voice.pan_env_on {
        if let Some(instrument) = voice
            .instrument
            .and_then(|index| song.instruments.get(index))
        {
            if instrument.pan_env.enabled {
                let value = i32::from(sample_envelope(&instrument.pan_env, voice.pan_env));
                pan += (value - 32) * 2;
            }
        }
    }
    pan.clamp(0, 255) as u16
}

fn pan_gains(pan: u16, separation: i32) -> (i32, i32) {
    let pan = i32::from(pan);
    let left = 256 - pan;
    let right = pan;
    let left = 128 + (left - 128) * separation / 100;
    let right = 128 + (right - 128) * separation / 100;
    (left.clamp(0, 256), right.clamp(0, 256))
}

fn read_sample(sample: &Sample, voice: &Voice, interpolation: Interpolation) -> i32 {
    let len = sample.pcm.len();
    if len == 0 {
        return 0;
    }
    let idx = (voice.pos >> FP) as usize;
    let frac = (voice.pos & 0xFFFF) as i32;
    let at = |index: usize| -> i32 {
        if index < len {
            i32::from(sample.pcm[index])
        } else {
            0
        }
    };
    let a = at(idx.min(len - 1));
    if interpolation == Interpolation::Nearest || frac == 0 {
        return a;
    }
    let b = at(idx.saturating_add(1).min(len - 1));
    let a = i64::from(a);
    let b = i64::from(b);
    (a + (b - a) * i64::from(frac) / 65536) as i32
}

fn advance_position(sample: &Sample, voice: &mut Voice) {
    let len = sample.pcm.len() as u64;
    if len == 0 {
        voice.active = false;
        return;
    }
    let (start, end, kind) = active_loop(sample, voice.key_off);
    let start_fp = u64::from(start) << FP;
    let end_fp = u64::from(end) << FP;
    if kind == LoopKind::None {
        if voice.forward {
            voice.pos = voice.pos.saturating_add(voice.step);
        }
        if voice.pos >> FP >= len {
            voice.active = false;
        }
        return;
    }
    let loop_len = end_fp.saturating_sub(start_fp).max(1 << FP);
    if kind == LoopKind::PingPong {
        if voice.forward {
            voice.pos = voice.pos.saturating_add(voice.step);
            if voice.pos >= end_fp {
                let over = voice.pos - end_fp;
                voice.pos = end_fp.saturating_sub(over.min(loop_len));
                voice.forward = false;
            }
        } else if voice.pos > voice.step {
            voice.pos -= voice.step;
            if voice.pos <= start_fp {
                let over = start_fp.saturating_sub(voice.pos);
                voice.pos = start_fp + over.min(loop_len);
                voice.forward = true;
            }
        } else {
            voice.forward = true;
            voice.pos = start_fp;
        }
        return;
    }
    voice.pos = voice.pos.saturating_add(voice.step);
    if voice.pos >= end_fp {
        voice.pos = start_fp + (voice.pos - end_fp) % loop_len;
    }
}

fn active_loop(sample: &Sample, key_off: bool) -> (u32, u32, LoopKind) {
    if !key_off
        && sample.sustain_kind != LoopKind::None
        && sample.sustain_end > sample.sustain_start
    {
        return (
            sample.sustain_start,
            sample.sustain_end,
            sample.sustain_kind,
        );
    }
    if sample.loop_kind != LoopKind::None && sample.loop_end > sample.loop_start {
        return (sample.loop_start, sample.loop_end, sample.loop_kind);
    }
    (0, 0, LoopKind::None)
}

fn remember(param: u8, previous: u8) -> u8 {
    if param == 0 {
        previous
    } else {
        param
    }
}

fn remember_nibble(nibble: u8, memory: &mut u8) -> u8 {
    if nibble == 0 {
        *memory
    } else {
        *memory = nibble;
        nibble
    }
}

fn remember_xy(param: u8, xmem: &mut u8, ymem: &mut u8) {
    let x = param >> 4;
    let y = param & 0x0F;
    if x != 0 {
        *xmem = x;
    }
    if y != 0 {
        *ymem = y;
    }
}

fn slide_u8(value: u8, param: u8, max: u8) -> u8 {
    if param & 0xF0 != 0 {
        value.saturating_add(param >> 4).min(max)
    } else {
        value.saturating_sub(param & 0x0F)
    }
}

fn it_vol_slide(value: u8, param: u8, tick: u8) -> u8 {
    let up = param >> 4;
    let down = param & 0x0F;
    if down == 0x0F && up != 0 {
        if tick == 0 {
            return value.saturating_add(up).min(64);
        }
        return value;
    }
    if up == 0x0F && down != 0 {
        if tick == 0 {
            return value.saturating_sub(down);
        }
        return value;
    }
    if tick == 0 {
        return value;
    }
    slide_u8(value, param, 64)
}

fn slide_value(value: i32, param: u8, max: i32) -> i32 {
    let next = if param & 0xF0 != 0 {
        value + i32::from(param >> 4)
    } else {
        value - i32::from(param & 0x0F)
    };
    next.clamp(0, max)
}

fn slide_pan(pan: u16, param: u8) -> u16 {
    let next = if param & 0xF0 != 0 {
        i32::from(pan) + i32::from(param >> 4) * 4
    } else {
        i32::from(pan) - i32::from(param & 0x0F) * 4
    };
    next.clamp(0, 255) as u16
}

fn bcd_row(param: u8) -> u16 {
    u16::from((param >> 4) * 10 + (param & 0x0F))
}

fn clamp_i16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// Play `song` from the top into a 16-bit stereo WAV file.
///
/// Rendering stops when the song loops, when playback halts, or after
/// `max_frames`, whichever comes first.
pub fn render_to_wav(
    song: &Song,
    path: &Path,
    config: PlayerConfig,
    max_frames: usize,
) -> Result<RenderStats, Error> {
    let mut playback = Playback::new(config);
    playback.set_stop_on_loop(true);
    playback.start(song, 0, 0);
    let mut pcm = Vec::new();
    let chunk = 2048usize;
    while pcm.len() / 2 < max_frames {
        let frames = chunk.min(max_frames - pcm.len() / 2);
        let mut buffer = vec![0i16; frames * 2];
        let wrote = playback.render(song, &mut buffer);
        if wrote == 0 {
            break;
        }
        pcm.extend_from_slice(&buffer[..wrote * 2]);
    }
    let frames = pcm.len() / 2;
    write_wav(path, config.sample_rate.max(1), &pcm)?;
    Ok(RenderStats {
        frames,
        sample_rate: config.sample_rate.max(1),
        looped: playback.looped(),
        halted: playback.is_halted(),
    })
}
