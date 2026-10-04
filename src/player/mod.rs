//! Four-channel ProTracker replayer.
//!
//! [`Playback::render`] is the whole mixer: give it a [`Module`](crate::Module)
//! and a buffer of interleaved stereo `i16` frames and it fills them. No audio
//! device is involved, so the same function drives unit tests, WAV export, and
//! the live callback.
//!
//! Timing follows the PAL CIA tempo. Speed is ticks per row (default 6) and
//! tempo is beats per minute (default 125, which is 50 ticks per second):
//!
//! ```text
//! frames per tick = sample_rate * 5 / (tempo * 2)
//! sample rate     = 3_546_895 / period
//! ```
//!
//! Slides, vibrato, arpeggio, tremolo, volume slides, retrigger, and note cut
//! run on every tick except the first. Notes, offsets, jumps, breaks, fine
//! slides, and speed/tempo run on the first tick only. A zero parameter recalls
//! the last nonzero one for slides, portamento, vibrato, tremolo, volume
//! slides, retrigger, sample offset, and the fine slides.
//!
//! # Not implemented
//!
//! These bytes are read and ignored. The song stays in time.
//!
//! | Effect | Why |
//! | --- | --- |
//! | `8xx` | Not a ProTracker command. Panning stays the Amiga hard pan. |
//! | `E0x` | Amiga LED filter. There is no analogue filter here. |
//! | `E8x` | Unused in ProTracker (some later trackers pan with it). |
//! | `EFx` | Invert loop. It rewrites sample bytes while they play. |
//!
//! `F00` stops the replayer. ProTracker instead freezes the tick clock; the
//! audible result is the same.

mod mix;
mod tables;

use std::path::Path;

use crate::error::Error;
use crate::module::{Cell, Module, CHANNELS, ORDER_LEN, ROWS};
use crate::wav::write_wav;

pub use tables::{
    pal_hz, sample_step, samples_per_tick, tuned_period, FINETUNE_PERIODS, MAX_PERIOD, MIN_PERIOD,
    PAL_CLOCK_HZ,
};

use tables::{break_row, lfo, nearest_period, semitone_period, signed_finetune};

/// Output rate used by [`PlayerConfig::default`] and by `--render`.
pub const DEFAULT_SAMPLE_RATE: u32 = 44_100;
/// `|sample byte| × volume` at which a channel meter reads full.
///
/// A stored sample is signed 8-bit, so −128 at volume 64 is 8192. [`Playback::channel_peaks`]
/// uses this scale. Muted channels and a volume of 0 stay at 0.
pub const CHANNEL_PEAK_SCALE: u16 = 8192;
/// Ticks per row until an `Fxx` command changes it.
pub const DEFAULT_SPEED: u8 = 6;
/// CIA tempo until an `Fxx` command changes it.
pub const DEFAULT_TEMPO: u8 = 125;

/// How a sample cursor picks bytes between two neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Interpolation {
    /// The byte currently under the cursor.
    Nearest,
    /// Straight line between this byte and the next, including across a loop.
    #[default]
    Linear,
}

/// Knobs that do not belong to the module file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerConfig {
    /// Output frames per second.
    pub sample_rate: u32,
    /// Sample interpolation.
    pub interpolation: Interpolation,
    /// `0` is mono, `100` is hard Amiga panning (channels 0 and 3 left, 1 and 2 right).
    pub stereo_separation: u8,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            interpolation: Interpolation::Linear,
            stereo_separation: 100,
        }
    }
}

/// What one render-to-disk pass produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderStats {
    /// Stereo frames written.
    pub frames: usize,
    /// Rate stored in the WAV header.
    pub sample_rate: u32,
    /// The song jumped back to an order it had already entered.
    pub looped: bool,
    /// An `F00` command halted playback.
    pub halted: bool,
}

/// One channel, for tests and the transport readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelState {
    /// Instrument number, `0` if the channel has never been given one.
    pub sample: u8,
    /// Period used by slides and portamento, before vibrato.
    pub period: u16,
    /// Period actually mixed on the last tick, after arpeggio, vibrato, and glissando.
    pub audible_period: u16,
    /// Volume stored by `Cxx` and volume slides, `0..=64`.
    pub volume: u8,
    /// Volume mixed on the last tick, after tremolo.
    pub audible_volume: u8,
    /// Sample cursor in 16.16 fixed point. The integer part is a byte index.
    pub position: u64,
    /// Signed finetune currently latched on the channel.
    pub finetune: i8,
    /// The channel still has sample data left to play.
    pub active: bool,
    /// Mix silence, but keep running effects and the sample cursor.
    pub muted: bool,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    tick: u8,
    sample: u8,
    period: u16,
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    sample: u8,
    position: u64,
    period: u16,
    porta_target: u16,
    volume: u8,
    finetune: u8,
    active: bool,
    muted: bool,
    effect: u8,
    effect_param: u8,
    slide_up: u8,
    slide_down: u8,
    porta_speed: u8,
    vibrato_speed: u8,
    vibrato_depth: u8,
    vibrato_pos: u8,
    vibrato_wave: u8,
    tremolo_speed: u8,
    tremolo_depth: u8,
    tremolo_pos: u8,
    tremolo_wave: u8,
    vol_slide: u8,
    offset: u8,
    retrigger: u8,
    fine_up: u8,
    fine_down: u8,
    fine_vol_up: u8,
    fine_vol_down: u8,
    glissando: bool,
    pending: Option<Pending>,
    arp_period: Option<u16>,
    vib_delta: i16,
    trem_delta: i16,
    audible_period: u16,
    audible_volume: u8,
}

impl Voice {
    fn new() -> Self {
        Self {
            sample: 0,
            position: 0,
            period: 0,
            porta_target: 0,
            volume: 0,
            finetune: 0,
            active: false,
            muted: false,
            effect: 0,
            effect_param: 0,
            slide_up: 0,
            slide_down: 0,
            porta_speed: 0,
            vibrato_speed: 0,
            vibrato_depth: 0,
            vibrato_pos: 0,
            vibrato_wave: 0,
            tremolo_speed: 0,
            tremolo_depth: 0,
            tremolo_pos: 0,
            tremolo_wave: 0,
            vol_slide: 0,
            offset: 0,
            retrigger: 0,
            fine_up: 0,
            fine_down: 0,
            fine_vol_up: 0,
            fine_vol_down: 0,
            glissando: false,
            pending: None,
            arp_period: None,
            vib_delta: 0,
            trem_delta: 0,
            audible_period: 0,
            audible_volume: 0,
        }
    }

    fn state(&self) -> ChannelState {
        ChannelState {
            sample: self.sample,
            period: self.period,
            audible_period: self.audible_period,
            volume: self.volume,
            audible_volume: self.audible_volume,
            position: self.position,
            finetune: signed_finetune(self.finetune),
            active: self.active,
            muted: self.muted,
        }
    }
}

/// Song position and the four voices.
///
/// The module is borrowed on each call so the editor can keep owning it.
#[derive(Debug)]
pub struct Playback {
    config: PlayerConfig,
    stop_on_loop: bool,
    started: bool,
    halted: bool,
    wrapped: bool,
    order: usize,
    row: usize,
    tick: u8,
    speed: u8,
    tempo: u8,
    samples_left: u32,
    in_delay: bool,
    pattern_delay: u8,
    loop_start: u8,
    loop_count: u8,
    pending_loop: Option<u8>,
    pending_break: Option<u8>,
    pending_jump: Option<u8>,
    visited: [bool; ORDER_LEN],
    voices: [Voice; CHANNELS],
    /// Peak `|sample × volume|` of each channel over the last [`Self::render`].
    frame_peaks: [u16; CHANNELS],
}

impl Playback {
    /// A stopped player. Call [`Self::start`] before [`Self::render`].
    pub fn new(config: PlayerConfig) -> Self {
        Self {
            config,
            stop_on_loop: false,
            started: false,
            halted: false,
            wrapped: false,
            order: 0,
            row: 0,
            tick: 0,
            speed: DEFAULT_SPEED,
            tempo: DEFAULT_TEMPO,
            samples_left: 0,
            in_delay: false,
            pattern_delay: 0,
            loop_start: 0,
            loop_count: 0,
            pending_loop: None,
            pending_break: None,
            pending_jump: None,
            visited: [false; ORDER_LEN],
            voices: std::array::from_fn(|_| Voice::new()),
            frame_peaks: [0; CHANNELS],
        }
    }

    /// Begin at `order` / `row`.
    ///
    /// Voice memory, sample cursors, effect memory, pattern-loop counters,
    /// pattern delay, and the song-loop visit map are cleared, and channel
    /// peaks drop to zero. A note that was still sounding does not continue,
    /// and a pattern loop that was already running does not jump again until
    /// the song reaches that command from the new position. Mutes and
    /// [`Self::set_stop_on_loop`] are kept, so a song that had already looped
    /// can play again. Passing order 0 and row 0 restarts at the top of the
    /// song, which is not the module's restart byte.
    pub fn start(&mut self, module: &Module, order: usize, row: usize) {
        let muted = self.voices.map(|voice| voice.muted);
        let config = self.config;
        let stop_on_loop = self.stop_on_loop;
        *self = Self::new(config);
        self.stop_on_loop = stop_on_loop;
        for (voice, mute) in self.voices.iter_mut().zip(muted) {
            voice.muted = mute;
        }
        let len = song_len(module);
        self.order = order.min(len - 1);
        self.row = row.min(ROWS - 1);
        self.visited[self.order] = true;
        self.started = true;
    }

    /// Stop scheduling new ticks once the song loops. Live playback leaves this off.
    pub fn set_stop_on_loop(&mut self, stop: bool) {
        self.stop_on_loop = stop;
    }

    /// Silence a channel without stopping its effects.
    pub fn set_mute(&mut self, channel: usize, muted: bool) {
        if let Some(voice) = self.voices.get_mut(channel) {
            voice.muted = muted;
        }
    }

    /// Flip mute and return the new value.
    pub fn toggle_mute(&mut self, channel: usize) -> bool {
        if let Some(voice) = self.voices.get_mut(channel) {
            voice.muted = !voice.muted;
            voice.muted
        } else {
            false
        }
    }

    /// Fill `output` with interleaved stereo frames.
    ///
    /// `output.len() / 2` frames are always written. The return value is how
    /// many of those frames came from the song; the rest are silence because
    /// playback halted or, when [`Self::set_stop_on_loop`] is set, because the
    /// song looped. An odd trailing sample is ignored.
    pub fn render(&mut self, module: &Module, output: &mut [i16]) -> usize {
        self.frame_peaks = [0; CHANNELS];
        let frames = output.len() / 2;
        if frames == 0 {
            return 0;
        }
        let mut filled = 0usize;
        while filled < frames {
            if self.samples_left == 0 {
                if !self.started || self.halted || (self.stop_on_loop && self.wrapped) {
                    output[filled * 2..].fill(0);
                    return filled;
                }
                self.begin_tick(module);
            }
            let n = (frames - filled).min(self.samples_left as usize);
            let end = (filled + n) * 2;
            let peaks = mix::mix_frames(
                module,
                &mut self.voices,
                self.config.sample_rate.max(1),
                self.config.interpolation,
                self.config.stereo_separation,
                &mut output[filled * 2..end],
            );
            for (slot, peak) in self.frame_peaks.iter_mut().zip(peaks) {
                if peak > *slot {
                    *slot = peak;
                }
            }
            filled += n;
            self.samples_left -= u32::try_from(n).unwrap_or(u32::MAX);
        }
        filled
    }

    /// Peak `|sample × volume|` of each channel over the last [`Self::render`].
    ///
    /// Index 0 is channel 1. The value is 0 when that channel is muted, its
    /// audible volume is 0, or it did not produce a sample in the buffer.
    /// [`CHANNEL_PEAK_SCALE`] is a full-scale byte at volume 64.
    pub fn channel_peaks(&self) -> [u16; CHANNELS] {
        self.frame_peaks
    }

    /// Order-list position, `0 .. song length`.
    pub fn order(&self) -> usize {
        self.order
    }

    /// Row inside the current pattern, `0..64`.
    pub fn row(&self) -> usize {
        self.row
    }

    /// Tick inside the current row.
    pub fn tick(&self) -> u8 {
        self.tick
    }

    /// Ticks per row.
    pub fn speed(&self) -> u8 {
        self.speed
    }

    /// CIA tempo.
    pub fn tempo(&self) -> u8 {
        self.tempo
    }

    /// Output rate this player was configured with.
    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    /// Pattern number stored at the current order position.
    pub fn pattern_index(&self, module: &Module) -> usize {
        let len = song_len(module);
        let pos = self.order.min(len - 1);
        usize::from(module.order[pos])
    }

    /// `start` has been called and `F00` has not.
    pub fn is_playing(&self) -> bool {
        self.started && !self.halted
    }

    /// An `F00` command stopped the clock.
    pub fn is_halted(&self) -> bool {
        self.halted
    }

    /// Playback has already visited the order it just entered.
    pub fn looped(&self) -> bool {
        self.wrapped
    }

    /// Channel `index`, if it is one of the four.
    pub fn channel(&self, index: usize) -> Option<ChannelState> {
        self.voices.get(index).map(Voice::state)
    }

    fn begin_tick(&mut self, module: &Module) {
        let frames = samples_per_tick(self.config.sample_rate, self.tempo);
        for voice in &mut self.voices {
            voice.arp_period = None;
            voice.vib_delta = 0;
            voice.trem_delta = 0;
        }
        if self.tick == 0 && !self.in_delay {
            self.process_row(module);
        } else if self.tick != 0 {
            self.process_mid(module);
        }
        for voice in &mut self.voices {
            voice.audible_period = audible_period(voice);
            voice.audible_volume = audible_volume(voice);
        }
        self.advance_clock(module);
        self.samples_left = frames;
    }

    fn process_row(&mut self, module: &Module) {
        let pattern = self.pattern_index(module);
        for channel in 0..CHANNELS {
            let cell = module
                .patterns
                .get(pattern)
                .map(|pattern| pattern.rows[self.row][channel])
                .unwrap_or_else(Cell::empty);
            self.process_channel(module, channel, cell);
        }
    }

    fn process_channel(&mut self, module: &Module, channel: usize, cell: Cell) {
        let delay = cell.effect == 0x0E && (cell.param >> 4) == 0x0D && (cell.param & 0x0F) != 0;
        {
            let voice = &mut self.voices[channel];
            voice.effect = cell.effect;
            voice.effect_param = cell.param;
            if delay {
                voice.pending = Some(Pending {
                    tick: cell.param & 0x0F,
                    sample: cell.sample,
                    period: cell.period,
                });
                return;
            }
            voice.pending = None;
            trigger(voice, module, cell.sample, cell.period, cell.effect);
        }
        effect_tick0(self, channel, cell, module);
    }

    fn process_mid(&mut self, module: &Module) {
        let tick = self.tick;
        for channel in 0..CHANNELS {
            let voice = &mut self.voices[channel];
            if let Some(pending) = voice.pending {
                if tick == pending.tick {
                    voice.pending = None;
                    trigger(voice, module, pending.sample, pending.period, 0);
                }
            }
            let effect = voice.effect;
            let param = voice.effect_param;
            match effect {
                0x0 if param != 0 => do_arpeggio(voice, tick, param),
                0x1 => {
                    let speed = voice.slide_up;
                    voice.period = slide_period(voice.period, -i16::from(speed));
                }
                0x2 => {
                    let speed = voice.slide_down;
                    voice.period = slide_period(voice.period, i16::from(speed));
                }
                0x3 => do_tone_porta(voice),
                0x4 => do_vibrato(voice),
                0x5 => {
                    do_tone_porta(voice);
                    let slide = voice.vol_slide;
                    apply_volume_slide(voice, slide);
                }
                0x6 => {
                    do_vibrato(voice);
                    let slide = voice.vol_slide;
                    apply_volume_slide(voice, slide);
                }
                0x7 => do_tremolo(voice),
                0xA => {
                    let slide = voice.vol_slide;
                    apply_volume_slide(voice, slide);
                }
                0xE => extended_mid(voice, tick, param),
                _ => {}
            }
        }
    }

    fn advance_clock(&mut self, module: &Module) {
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
            self.advance_position(module);
        }
    }

    fn advance_position(&mut self, module: &Module) {
        let had_break = self.pending_break.is_some();
        let had_jump = self.pending_jump.is_some();
        if !had_break && !had_jump {
            if let Some(loop_row) = self.pending_loop.take() {
                self.row = usize::from(loop_row).min(ROWS - 1);
                return;
            }
        }
        self.pending_loop = None;

        let mut next_row = self.row.saturating_add(1);
        let mut next_order = self.order;
        if let Some(row) = self.pending_break.take() {
            next_row = usize::from(row);
            next_order = self.order.saturating_add(1);
        }
        if let Some(order) = self.pending_jump.take() {
            next_order = usize::from(order);
            if !had_break {
                next_row = 0;
            }
        }
        if next_row >= ROWS {
            next_row = 0;
            next_order = next_order.saturating_add(1);
        }

        let len = song_len(module);
        let mut wrapped_by_end = false;
        if next_order >= len {
            next_order = restart_order(module, len);
            next_row = 0;
            wrapped_by_end = true;
        }
        let revisiting = wrapped_by_end
            || (next_order != self.order && self.visited[next_order])
            || (had_jump && next_order == self.order);
        if revisiting {
            self.wrapped = true;
        }
        self.visited[next_order] = true;
        self.order = next_order;
        self.row = next_row;
    }
}

fn effect_tick0(playback: &mut Playback, channel: usize, cell: Cell, module: &Module) {
    let param = cell.param;
    match cell.effect {
        0xB => playback.pending_jump = Some(param),
        0xD => playback.pending_break = Some(break_row(param)),
        0xE => extended_tick0(playback, channel, param),
        0xF => {
            if param == 0 {
                playback.halted = true;
            } else if param < 0x20 {
                playback.speed = param;
            } else {
                playback.tempo = param;
            }
        }
        effect => {
            let voice = &mut playback.voices[channel];
            match effect {
                0x1 => voice.slide_up = remember(param, voice.slide_up),
                0x2 => voice.slide_down = remember(param, voice.slide_down),
                0x3 => voice.porta_speed = remember(param, voice.porta_speed),
                0x4 => remember_xy(param, &mut voice.vibrato_speed, &mut voice.vibrato_depth),
                0x5 | 0x6 | 0xA => voice.vol_slide = remember(param, voice.vol_slide),
                0x7 => remember_xy(param, &mut voice.tremolo_speed, &mut voice.tremolo_depth),
                0x9 => {
                    voice.offset = remember(param, voice.offset);
                    if cell.period != 0 {
                        apply_offset(voice, module);
                    }
                }
                0xC => voice.volume = param.min(64),
                _ => {}
            }
        }
    }
}

fn extended_tick0(playback: &mut Playback, channel: usize, param: u8) {
    let nibble = param & 0x0F;
    match param >> 4 {
        0x6 => {
            if nibble == 0 {
                playback.loop_start = u8::try_from(playback.row).unwrap_or(0);
            } else if playback.loop_count == 0 {
                playback.loop_count = nibble;
                playback.pending_loop = Some(playback.loop_start);
            } else {
                playback.loop_count -= 1;
                if playback.loop_count != 0 {
                    playback.pending_loop = Some(playback.loop_start);
                }
            }
        }
        0xE => playback.pattern_delay = nibble,
        command => {
            let voice = &mut playback.voices[channel];
            match command {
                0x1 => {
                    let amount = remember_nibble(nibble, &mut voice.fine_up);
                    voice.period = slide_period(voice.period, -i16::from(amount));
                }
                0x2 => {
                    let amount = remember_nibble(nibble, &mut voice.fine_down);
                    voice.period = slide_period(voice.period, i16::from(amount));
                }
                0x3 => voice.glissando = nibble != 0,
                0x4 => voice.vibrato_wave = nibble,
                0x5 => voice.finetune = nibble,
                0x7 => voice.tremolo_wave = nibble,
                0x9 => voice.retrigger = remember_nibble(nibble, &mut voice.retrigger),
                0xA => {
                    let amount = remember_nibble(nibble, &mut voice.fine_vol_up);
                    voice.volume = voice.volume.saturating_add(amount).min(64);
                }
                0xB => {
                    let amount = remember_nibble(nibble, &mut voice.fine_vol_down);
                    voice.volume = voice.volume.saturating_sub(amount);
                }
                0xC if nibble == 0 => voice.volume = 0,
                // E0x filter, E8x, EFx invert loop: ignored.
                _ => {}
            }
        }
    }
}

fn extended_mid(voice: &mut Voice, tick: u8, param: u8) {
    let nibble = param & 0x0F;
    match param >> 4 {
        0x9 => {
            let interval = voice.retrigger;
            if interval != 0 && tick % interval == 0 {
                retrigger(voice);
            }
        }
        0xC if tick == nibble => voice.volume = 0,
        _ => {}
    }
}

fn trigger(voice: &mut Voice, module: &Module, sample: u8, period: u16, effect: u8) {
    let porta = matches!(effect, 0x3 | 0x5);
    if (1..=31).contains(&sample) {
        let instrument = &module.samples[usize::from(sample) - 1];
        voice.sample = sample;
        voice.finetune = instrument.finetune_raw & 0x0F;
        voice.volume = instrument.volume.min(64);
    }
    if period == 0 {
        return;
    }
    let tuned = tuned_period(period, voice.finetune);
    if porta && voice.period != 0 {
        voice.porta_target = tuned;
        return;
    }
    voice.period = tuned;
    if porta {
        voice.porta_target = tuned;
    }
    voice.position = 0;
    voice.active = voice.sample != 0;
    if voice.vibrato_wave & 4 == 0 {
        voice.vibrato_pos = 0;
    }
    if voice.tremolo_wave & 4 == 0 {
        voice.tremolo_pos = 0;
    }
}

fn apply_offset(voice: &mut Voice, module: &Module) {
    let bytes = u64::from(voice.offset) * 256;
    voice.position = bytes << tables::FP_SHIFT;
    let Some(index) = usize::from(voice.sample).checked_sub(1) else {
        voice.active = false;
        return;
    };
    let Some(sample) = module.samples.get(index) else {
        voice.active = false;
        return;
    };
    let end = if sample.loops() {
        let start = usize::from(sample.loop_start) * 2;
        start
            .saturating_add(usize::from(sample.loop_length) * 2)
            .min(sample.data.len())
    } else {
        sample.data.len()
    };
    voice.active = (bytes as usize) < end && voice.sample != 0;
}

fn retrigger(voice: &mut Voice) {
    if voice.sample == 0 {
        return;
    }
    voice.position = 0;
    voice.active = true;
}

fn do_arpeggio(voice: &mut Voice, tick: u8, param: u8) {
    let semitones = match tick % 3 {
        1 => param >> 4,
        2 => param & 0x0F,
        _ => 0,
    };
    if semitones == 0 || voice.period == 0 {
        voice.arp_period = None;
    } else {
        voice.arp_period = Some(semitone_period(voice.period, voice.finetune, semitones));
    }
}

fn do_vibrato(voice: &mut Voice) {
    let raw = lfo(voice.vibrato_pos, voice.vibrato_wave);
    let delta = i32::from(raw) * i32::from(voice.vibrato_depth) / 128;
    voice.vib_delta = i16::try_from(delta).unwrap_or(0);
    voice.vibrato_pos = voice.vibrato_pos.wrapping_add(voice.vibrato_speed) & 63;
}

fn do_tremolo(voice: &mut Voice) {
    let raw = lfo(voice.tremolo_pos, voice.tremolo_wave);
    let delta = i32::from(raw) * i32::from(voice.tremolo_depth) / 64;
    voice.trem_delta = i16::try_from(delta.clamp(-128, 127)).unwrap_or(0);
    voice.tremolo_pos = voice.tremolo_pos.wrapping_add(voice.tremolo_speed) & 63;
}

fn do_tone_porta(voice: &mut Voice) {
    if voice.porta_target == 0 || voice.porta_speed == 0 || voice.period == 0 {
        return;
    }
    let speed = u16::from(voice.porta_speed);
    voice.period = match voice.period.cmp(&voice.porta_target) {
        std::cmp::Ordering::Less => voice.period.saturating_add(speed).min(voice.porta_target),
        std::cmp::Ordering::Greater => voice.period.saturating_sub(speed).max(voice.porta_target),
        std::cmp::Ordering::Equal => voice.period,
    };
    voice.period = voice.period.clamp(MIN_PERIOD, MAX_PERIOD);
}

fn apply_volume_slide(voice: &mut Voice, param: u8) {
    let up = param >> 4;
    let down = param & 0x0F;
    if up != 0 {
        voice.volume = voice.volume.saturating_add(up).min(64);
    } else if down != 0 {
        voice.volume = voice.volume.saturating_sub(down);
    }
}

fn audible_period(voice: &Voice) -> u16 {
    let base = voice.arp_period.unwrap_or(voice.period);
    if base == 0 {
        return 0;
    }
    let mixed = i32::from(base) + i32::from(voice.vib_delta);
    let mixed = mixed.clamp(1, 4095) as u16;
    if voice.glissando {
        nearest_period(mixed, voice.finetune)
    } else {
        mixed
    }
}

fn audible_volume(voice: &Voice) -> u8 {
    let mixed = i32::from(voice.volume) + i32::from(voice.trem_delta);
    mixed.clamp(0, 64) as u8
}

fn slide_period(period: u16, delta: i16) -> u16 {
    if period == 0 {
        return 0;
    }
    let next = i32::from(period) + i32::from(delta);
    next.clamp(i32::from(MIN_PERIOD), i32::from(MAX_PERIOD)) as u16
}

fn remember(param: u8, previous: u8) -> u8 {
    if param == 0 {
        previous
    } else {
        param
    }
}

fn remember_nibble(nibble: u8, slot: &mut u8) -> u8 {
    if nibble != 0 {
        *slot = nibble;
    }
    *slot
}

fn remember_xy(param: u8, x_slot: &mut u8, y_slot: &mut u8) {
    let x = param >> 4;
    let y = param & 0x0F;
    if x != 0 {
        *x_slot = x;
    }
    if y != 0 {
        *y_slot = y;
    }
}

fn song_len(module: &Module) -> usize {
    usize::from(module.song_length).clamp(1, ORDER_LEN)
}

fn restart_order(module: &Module, len: usize) -> usize {
    let restart = usize::from(module.restart);
    if restart < len {
        restart
    } else {
        0
    }
}

/// One note on a scratch module, using `module`'s sample data.
///
/// The pattern is a single cell. Playback renders it with the same mixer as a
/// song, which is what edit-mode preview hears.
pub fn preview_module(module: &Module, sample: u8, period: u16, channel: usize) -> Module {
    let mut scratch = Module::new(module.tag);
    if (1..=31).contains(&sample) {
        let index = usize::from(sample) - 1;
        scratch.samples[index] = module.samples[index].clone();
    }
    if period > 0 && (1..=31).contains(&sample) {
        let channel = channel.min(CHANNELS - 1);
        scratch.patterns[0].rows[0][channel] = Cell {
            sample,
            period,
            effect: 0,
            param: 0,
        };
    }
    scratch
}

/// Render `frames` of [`preview_module`] at `sample_rate`.
///
/// An empty instrument, a zero period, or a zero frame count is silence.
pub fn render_preview(
    module: &Module,
    sample: u8,
    period: u16,
    channel: usize,
    sample_rate: u32,
    frames: usize,
) -> Vec<i16> {
    let mut out = vec![0i16; frames.saturating_mul(2)];
    if frames == 0 {
        return out;
    }
    let scratch = preview_module(module, sample, period, channel);
    let config = PlayerConfig {
        sample_rate: sample_rate.max(1),
        ..PlayerConfig::default()
    };
    let mut playback = Playback::new(config);
    playback.start(&scratch, 0, 0);
    playback.render(&scratch, &mut out);
    out
}

/// Play `module` from the top into a 16-bit stereo WAV file.
///
/// Rendering stops when the song loops, when `F00` halts it, or after
/// `max_frames` frames, whichever comes first.
pub fn render_to_wav(
    module: &Module,
    path: &Path,
    config: PlayerConfig,
    max_frames: usize,
) -> Result<RenderStats, Error> {
    let mut playback = Playback::new(config);
    playback.set_stop_on_loop(true);
    playback.start(module, 0, 0);
    let mut pcm = Vec::new();
    let chunk = 2048usize;
    while pcm.len() / 2 < max_frames {
        let frames = chunk.min(max_frames - pcm.len() / 2);
        let mut buffer = vec![0i16; frames * 2];
        let wrote = playback.render(module, &mut buffer);
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
