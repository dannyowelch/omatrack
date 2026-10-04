//! Linux playback through [cpal](https://docs.rs/cpal).
//!
//! The default host is ALSA. PipeWire and PulseAudio both expose an ALSA
//! device (`pipewire` / `pulse`, usually installed as the `default` PCM), so
//! one backend covers the three servers Omarchy machines actually run. The
//! callback only pulls frames from [`Playback`](crate::player::Playback); it
//! does not own the song clock.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

use crate::error::Error;
use crate::module::Module;
use crate::player::{Playback, PlayerConfig};
use crate::viz::{VizAccum, VizBus, VizSnapshot};

/// Mix on the UI thread and publish analyzer windows without opening a device.
///
/// Set to `1`, `true`, or `yes`. The tracker still redraws on its normal
/// timer; only the sound-device open is skipped. For machines with no ALSA,
/// PipeWire, or PulseAudio output, and for capturing the live spectrum.
pub const SOFTWARE_AUDIO_ENV: &str = "OMATRACK_SOFTWARE_AUDIO";

/// `true` when [`SOFTWARE_AUDIO_ENV`] requests the device-free mixer.
pub fn software_audio_enabled() -> bool {
    match std::env::var(SOFTWARE_AUDIO_ENV) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        ),
        Err(_) => false,
    }
}

/// Shown when the host cannot see an output device.
pub const NO_DEVICE: &str = "No audio output device. PipeWire, PulseAudio, and ALSA did not expose an output; check that a sound server is running and a device is connected";

/// Where the song is, as of the last time the audio thread ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    /// Order-list position.
    pub order: usize,
    /// Row inside that pattern.
    pub row: usize,
    /// Pattern number at that order position.
    pub pattern: usize,
    /// Ticks per row.
    pub speed: u8,
    /// CIA tempo.
    pub tempo: u8,
    /// The stream is pulling frames.
    pub active: bool,
}

struct Shared {
    module: Module,
    playback: Playback,
    active: bool,
    /// The callback is mixing the song, not a note preview.
    song: bool,
    /// Frames of preview still to mix. `None` while a song is playing.
    preview_left: Option<u32>,
}

/// A live output stream. Opening the device is deferred until [`Self::start`].
pub struct AudioOutput {
    stream: Option<Stream>,
    shared: Arc<Mutex<Shared>>,
    error: Arc<Mutex<Option<String>>>,
    /// Sample rate of the open stream, when there is one.
    rate: Option<u32>,
    /// Interpolation, separation, and the rate to request from the device.
    preferences: PlayerConfig,
    /// Latest mix window for the spectrum. The callback only stores atomics.
    viz: Arc<VizBus>,
    /// Mix on this thread instead of a cpal callback. See [`SOFTWARE_AUDIO_ENV`].
    software: bool,
    /// Rolling window for [`Self::software`] publishes. The callback keeps its own.
    accum: VizAccum,
    /// Wall clock of the last software mix, so each pump covers the gap.
    last_pump: Instant,
}

impl AudioOutput {
    /// An output that is not playing and has not opened a device yet.
    pub fn new() -> Self {
        Self {
            stream: None,
            shared: Arc::new(Mutex::new(Shared {
                module: Module::default(),
                playback: Playback::new(PlayerConfig::default()),
                active: false,
                song: false,
                preview_left: None,
            })),
            error: Arc::new(Mutex::new(None)),
            rate: None,
            preferences: PlayerConfig::default(),
            viz: Arc::new(VizBus::new()),
            software: false,
            accum: VizAccum::new(),
            last_pump: Instant::now(),
        }
    }

    /// Latest analysis window, if the callback has published one.
    ///
    /// This copies atomics and does not lock the mixer, so a slow UI frame
    /// cannot stall the callback.
    pub fn visualization(&self) -> Option<VizSnapshot> {
        self.viz.load()
    }

    /// Forget the last analysis window.
    ///
    /// Channel meters and the spectrum read this slot. Clearing it keeps a
    /// rewind from holding the previous mix until the next publish.
    pub fn clear_visualization(&self) {
        self.viz.clear();
    }

    /// Mixer settings from the config file. The device rate still wins when it
    /// cannot play [`PlayerConfig::sample_rate`].
    pub fn set_preferences(&mut self, config: PlayerConfig) {
        self.preferences = config;
    }

    fn mix_config(&self, sample_rate: u32) -> PlayerConfig {
        PlayerConfig {
            sample_rate,
            interpolation: self.preferences.interpolation,
            stereo_separation: self.preferences.stereo_separation,
        }
    }

    /// Open the default output device and play `module` from `order` / `row`.
    ///
    /// Fails with a message that names PipeWire, PulseAudio, and ALSA when no
    /// device can be opened. An existing stream is stopped first.
    pub fn start(
        &mut self,
        module: &Module,
        order: usize,
        row: usize,
        muted: [bool; 4],
    ) -> Result<(), Error> {
        self.stop();
        if software_audio_enabled() {
            return self.start_software(module, order, row, muted);
        }
        let opened = open_device(self.preferences.sample_rate)?;
        let config = self.mix_config(opened.config.sample_rate);
        self.rate = Some(opened.config.sample_rate);
        let module = module.clone();
        let mut playback = Playback::new(config);
        for (channel, mute) in muted.into_iter().enumerate() {
            playback.set_mute(channel, mute);
        }
        playback.start(&module, order, row);
        self.play_opened(opened, module, playback)
    }

    fn play_opened(
        &mut self,
        opened: OpenedDevice,
        module: Module,
        playback: Playback,
    ) -> Result<(), Error> {
        {
            let mut shared = lock_shared(&self.shared);
            shared.module = module;
            shared.playback = playback;
            shared.active = true;
            shared.song = true;
            shared.preview_left = None;
        }
        if let Ok(mut slot) = self.error.lock() {
            *slot = None;
        }
        match build_stream(
            &opened,
            Arc::clone(&self.shared),
            Arc::clone(&self.error),
            Arc::clone(&self.viz),
        ) {
            Ok(stream) => {
                stream.play().map_err(|err| {
                    self.deactivate();
                    Error::Audio(format!(
                        "could not start playback on \"{}\": {err}",
                        opened.name
                    ))
                })?;
                self.stream = Some(stream);
                Ok(())
            }
            Err(err) => {
                self.deactivate();
                Err(err)
            }
        }
    }

    /// Drop the stream and stop pulling frames. The last position is kept.
    pub fn stop(&mut self) {
        self.deactivate();
        self.stream = None;
        self.rate = None;
        self.software = false;
        self.accum = VizAccum::new();
    }

    /// Render the audio that elapsed since the previous pump.
    ///
    /// No-op unless playback was started under [`SOFTWARE_AUDIO_ENV`]. The UI
    /// calls this once per frame, before it reads the analyzer, so the
    /// spectrum sees the same mix the callback would have published.
    pub fn pump_software(&mut self) {
        if !self.software {
            return;
        }
        let active = self
            .shared
            .lock()
            .map(|shared| shared.active)
            .unwrap_or(false);
        if !active {
            return;
        }
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_pump).as_secs_f32();
        self.last_pump = now;
        if !dt.is_finite() || dt < 0.005 {
            return;
        }
        let dt = dt.min(0.10);
        let rate = self.rate.unwrap_or(44_100).max(1);
        let frames = ((dt * rate as f32).round() as usize).clamp(1, (rate as usize) / 4);
        let mut scratch = vec![0i16; frames * 2];
        let peaks = render_scratch(&self.shared, frames, &mut scratch);
        self.accum.push(&scratch, peaks, &self.viz, rate);
    }

    fn start_software(
        &mut self,
        module: &Module,
        order: usize,
        row: usize,
        muted: [bool; 4],
    ) -> Result<(), Error> {
        let rate = self.preferences.sample_rate.max(1);
        let config = self.mix_config(rate);
        let module = module.clone();
        let mut playback = Playback::new(config);
        for (channel, mute) in muted.into_iter().enumerate() {
            playback.set_mute(channel, mute);
        }
        playback.start(&module, order, row);
        {
            let mut shared = lock_shared(&self.shared);
            shared.module = module;
            shared.playback = playback;
            shared.active = true;
            shared.song = true;
            shared.preview_left = None;
        }
        if let Ok(mut slot) = self.error.lock() {
            *slot = None;
        }
        self.rate = Some(rate);
        self.software = true;
        self.accum = VizAccum::new();
        self.last_pump = Instant::now();
        Ok(())
    }

    /// Play one note through the mixer without moving the song.
    ///
    /// Ignored while a song is already playing. The scratch module uses
    /// `module`'s sample data and the device's sample rate.
    pub fn preview(
        &mut self,
        module: &Module,
        sample: u8,
        period: u16,
        channel: usize,
    ) -> Result<(), Error> {
        {
            let shared = lock_shared(&self.shared);
            if shared.song && shared.active {
                return Ok(());
            }
        }
        let scratch = crate::player::preview_module(module, sample, period, channel);
        if self.stream.is_none() {
            let opened = open_device(self.preferences.sample_rate)?;
            self.rate = Some(opened.config.sample_rate);
            let stream = build_stream(
                &opened,
                Arc::clone(&self.shared),
                Arc::clone(&self.error),
                Arc::clone(&self.viz),
            )?;
            stream.play().map_err(|err| {
                self.deactivate();
                self.rate = None;
                Error::Audio(format!(
                    "could not start preview on \"{}\": {err}",
                    opened.name
                ))
            })?;
            self.stream = Some(stream);
        }
        let rate = self
            .rate
            .unwrap_or(crate::player::DEFAULT_SAMPLE_RATE)
            .max(1);
        let config = self.mix_config(rate);
        let mut playback = Playback::new(config);
        playback.start(&scratch, 0, 0);
        let frames = (rate / 5).max(1);
        let mut shared = lock_shared(&self.shared);
        shared.module = scratch;
        shared.playback = playback;
        shared.song = false;
        shared.active = true;
        shared.preview_left = Some(frames);
        Ok(())
    }

    /// Audition instrument `slot` (0-based) at `period` through the same mixer as [`Self::preview`].
    ///
    /// The note is centered, and a sample whose volume is 0 is heard at 64. Playback
    /// holds for about four seconds, which covers a one-shot and a few loops, then
    /// the stream closes. Ignored while the song is playing.
    pub fn audition(&mut self, module: &Module, slot: usize, period: u16) -> Result<(), Error> {
        let Some(sample_no) = u8::try_from(slot.saturating_add(1))
            .ok()
            .filter(|n| *n <= 31)
        else {
            return Ok(());
        };
        {
            let shared = lock_shared(&self.shared);
            if shared.song && shared.active {
                return Ok(());
            }
        }
        let mut scratch = crate::player::preview_module(module, sample_no, period, 0);
        if let Some(sample) = scratch.samples.get_mut(slot) {
            if sample.volume == 0 && !sample.data.is_empty() {
                sample.volume = 64;
            }
        }
        if self.stream.is_none() {
            let opened = open_device(self.preferences.sample_rate)?;
            self.rate = Some(opened.config.sample_rate);
            let stream = build_stream(
                &opened,
                Arc::clone(&self.shared),
                Arc::clone(&self.error),
                Arc::clone(&self.viz),
            )?;
            stream.play().map_err(|err| {
                self.deactivate();
                self.rate = None;
                Error::Audio(format!(
                    "could not start preview on \"{}\": {err}",
                    opened.name
                ))
            })?;
            self.stream = Some(stream);
        }
        let rate = self
            .rate
            .unwrap_or(crate::player::DEFAULT_SAMPLE_RATE)
            .max(1);
        let mut config = self.mix_config(rate);
        config.stereo_separation = 0;
        let mut playback = Playback::new(config);
        playback.start(&scratch, 0, 0);
        let frames = rate.saturating_mul(4).max(1);
        let mut shared = lock_shared(&self.shared);
        shared.module = scratch;
        shared.playback = playback;
        shared.song = false;
        shared.active = true;
        shared.preview_left = Some(frames);
        Ok(())
    }

    /// Replace the song the callback is mixing. Preview is left alone.
    pub fn replace_module(&self, module: &Module) {
        let mut shared = lock_shared(&self.shared);
        if shared.song {
            shared.module = module.clone();
        }
    }

    /// Close a preview stream after it has finished.
    pub fn stop_if_preview_done(&mut self) {
        let done = {
            let shared = lock_shared(&self.shared);
            self.stream.is_some() && !shared.song && !shared.active
        };
        if done {
            self.stop();
        }
    }

    /// Silence one channel, or bring it back.
    pub fn set_mute(&self, channel: usize, muted: bool) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.playback.set_mute(channel, muted);
        }
    }

    /// Last position the callback published. `None` if the lock is poisoned.
    pub fn snapshot(&self) -> Option<Snapshot> {
        let shared = self.shared.lock().ok()?;
        Some(Snapshot {
            order: shared.playback.order(),
            row: shared.playback.row(),
            pattern: shared.playback.pattern_index(&shared.module),
            speed: shared.playback.speed(),
            tempo: shared.playback.tempo(),
            active: shared.active,
        })
    }

    /// Take a stream error reported by the audio thread, if there is one.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|mut slot| slot.take())
    }

    fn deactivate(&self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.active = false;
            shared.song = false;
            shared.preview_left = None;
        }
    }
}

impl Default for AudioOutput {
    fn default() -> Self {
        Self::new()
    }
}

struct OpenedDevice {
    device: cpal::Device,
    config: StreamConfig,
    format: SampleFormat,
    name: String,
}

fn playable(format: SampleFormat) -> bool {
    matches!(
        format,
        SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
    )
}

fn open_device(preferred_rate: u32) -> Result<OpenedDevice, Error> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| Error::Audio(NO_DEVICE.to_string()))?;
    let name = device
        .description()
        .map(|desc| desc.name().to_string())
        .unwrap_or_else(|_| "default output".to_string());
    let default = device.default_output_config().map_err(|err| {
        Error::Audio(format!(
            "\"{name}\" has no playable output ({err}). PipeWire, PulseAudio, and ALSA all use this device list"
        ))
    })?;
    let supported = choose_output(&device, preferred_rate).unwrap_or(default);
    let format = supported.sample_format();
    if !playable(format) {
        return Err(Error::Audio(format!(
            "\"{name}\" uses {format:?}, which omatrack cannot play. Expected f32, i16, or u16"
        )));
    }
    let config: StreamConfig = supported.into();
    Ok(OpenedDevice {
        device,
        config,
        format,
        name,
    })
}

fn choose_output(
    device: &cpal::Device,
    preferred_rate: u32,
) -> Option<cpal::SupportedStreamConfig> {
    if preferred_rate == 0 {
        return None;
    }
    let ranges = device.supported_output_configs().ok()?;
    for range in ranges {
        if !playable(range.sample_format()) {
            continue;
        }
        let min = range.min_sample_rate();
        let max = range.max_sample_rate();
        if preferred_rate >= min && preferred_rate <= max {
            return Some(range.with_sample_rate(preferred_rate));
        }
    }
    None
}

fn build_stream(
    opened: &OpenedDevice,
    shared: Arc<Mutex<Shared>>,
    error: Arc<Mutex<Option<String>>>,
    viz: Arc<VizBus>,
) -> Result<Stream, Error> {
    let channels = opened.config.channels as usize;
    let err_fn = {
        let error = Arc::clone(&error);
        let shared = Arc::clone(&shared);
        move |err: cpal::StreamError| {
            if let Ok(mut slot) = error.try_lock() {
                *slot = Some(format!("audio stream failed: {err}"));
            }
            if let Ok(mut shared) = shared.try_lock() {
                shared.active = false;
            }
        }
    };
    let name = opened.name.clone();
    let built = match opened.format {
        SampleFormat::F32 => build_typed::<f32>(
            &opened.device,
            &opened.config,
            channels,
            shared,
            err_fn,
            viz,
        ),
        SampleFormat::I16 => build_typed::<i16>(
            &opened.device,
            &opened.config,
            channels,
            shared,
            err_fn,
            viz,
        ),
        SampleFormat::U16 => build_typed::<u16>(
            &opened.device,
            &opened.config,
            channels,
            shared,
            err_fn,
            viz,
        ),
        other => {
            return Err(Error::Audio(format!(
                "the output device \"{name}\" uses {other:?}, which omatrack cannot play"
            )));
        }
    };
    built.map_err(|err| {
        Error::Audio(format!(
            "could not open the audio device \"{name}\": {err}. If PipeWire or PulseAudio is running, its ALSA device should appear as the default output"
        ))
    })
}

fn build_typed<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    shared: Arc<Mutex<Shared>>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
    viz: Arc<VizBus>,
) -> Result<Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + SampleConvert,
{
    let mut scratch = Vec::<i16>::new();
    let mut accum = VizAccum::new();
    let sample_rate = config.sample_rate;
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let frames = if channels == 0 {
                0
            } else {
                data.len() / channels
            };
            let peaks = render_scratch(&shared, frames, &mut scratch);
            for frame in 0..frames {
                let left = scratch[frame * 2];
                let right = scratch[frame * 2 + 1];
                let base = frame * channels;
                write_frame(&mut data[base..base + channels], left, right);
            }
            let end = frames.saturating_mul(2).min(scratch.len());
            accum.push(&scratch[..end], peaks, &viz, sample_rate);
        },
        err_fn,
        None,
    )
}

fn render_scratch(shared: &Mutex<Shared>, frames: usize, scratch: &mut Vec<i16>) -> [u16; 4] {
    scratch.resize(frames * 2, 0);
    let Ok(mut shared) = shared.lock() else {
        scratch.fill(0);
        return [0; 4];
    };
    if frames == 0 || !shared.active {
        scratch.fill(0);
        return [0; 4];
    }
    if shared.song {
        let Shared {
            module, playback, ..
        } = &mut *shared;
        playback.render(module, scratch);
        return playback.channel_peaks();
    }
    let left = shared.preview_left.unwrap_or(0);
    if left == 0 {
        scratch.fill(0);
        shared.active = false;
        return [0; 4];
    }
    let n = frames.min(usize::try_from(left).unwrap_or(frames));
    scratch[n * 2..].fill(0);
    let peaks = {
        let Shared {
            module, playback, ..
        } = &mut *shared;
        playback.render(module, &mut scratch[..n * 2]);
        playback.channel_peaks()
    };
    let left = left.saturating_sub(u32::try_from(n).unwrap_or(left));
    shared.preview_left = Some(left);
    if left == 0 {
        shared.active = false;
    }
    peaks
}

fn write_frame<T: SampleConvert>(frame: &mut [T], left: i16, right: i16) {
    if frame.is_empty() {
        return;
    }
    if frame.len() == 1 {
        let mono = (i32::from(left) + i32::from(right)) / 2;
        frame[0] = T::from_i16(mono.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
        return;
    }
    frame[0] = T::from_i16(left);
    frame[1] = T::from_i16(right);
    for channel in frame.iter_mut().skip(2) {
        *channel = T::from_i16(0);
    }
}

/// Conversion from the mixer's `i16` into the device format.
trait SampleConvert: Copy + Send + 'static {
    fn from_i16(sample: i16) -> Self;
}

impl SampleConvert for f32 {
    fn from_i16(sample: i16) -> Self {
        f32::from(sample) / 32768.0
    }
}

impl SampleConvert for i16 {
    fn from_i16(sample: i16) -> Self {
        sample
    }
}

impl SampleConvert for u16 {
    fn from_i16(sample: i16) -> Self {
        (i32::from(sample) + 32768) as u16
    }
}

fn lock_shared(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_device_message_names_the_servers() {
        let text = Error::Audio(NO_DEVICE.to_string()).to_string();
        assert!(text.contains("PipeWire"), "{text}");
        assert!(text.contains("PulseAudio"), "{text}");
        assert!(text.contains("ALSA"), "{text}");
    }

    #[test]
    fn the_default_device_either_opens_or_names_the_failure() {
        let module = crate::demo::showcase();
        let mut output = AudioOutput::new();
        match output.start(&module, 0, 0, [false; 4]) {
            Ok(()) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
                let snapshot = output.snapshot().expect("snapshot");
                assert!(snapshot.active);
                output.stop();
            }
            Err(err) => {
                let text = err.to_string();
                assert!(
                    text.contains("device") || text.contains("ALSA") || text.contains("PipeWire"),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn i16_converts_to_float_and_offset_binary() {
        assert_eq!(f32::from_i16(0), 0.0);
        assert!((f32::from_i16(i16::MAX) - (32767.0 / 32768.0)).abs() < 1e-6);
        assert_eq!(u16::from_i16(0), 32768);
        assert_eq!(u16::from_i16(i16::MIN), 0);
    }
}
