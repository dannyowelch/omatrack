//! Smoothed levels the UI draws. The FFT runs here, on the thread that calls
//! [`VizState::tick`], never inside the audio callback.

use super::agc::AutoGain;
use super::ballistics::{smooth_spectrum, Ballistics, Meter, PeakMark};
use super::bands::{db_unit, display_shift, spectrum_magnitudes};
use super::bus::VizSnapshot;
use super::column::sample_series;
use super::WINDOW;
use crate::player::CHANNEL_PEAK_SCALE;

/// How long a missing or repeated window may sit before levels ease toward
/// silence. Shorter gaps keep the last complete analysis and only advance
/// the ballistics clock.
const STALE_AFTER: f32 = 0.35;

/// Bottom of the channel-meter curve. 0 dBFS is the top of the meter.
/// A loud channel around −12..−6 dBFS lands near 60–80%.
pub const METER_FLOOR_DB: f32 = -30.0;

/// How many log bars the analyzer keeps. The panel samples these into columns.
pub const BARS: usize = 48;

/// Spectrum, meters, and the latest stereo window.
#[derive(Debug, Clone)]
pub struct VizState {
    bars: Vec<Meter>,
    meters: [Meter; 4],
    stereo: Vec<f32>,
    phase: f32,
    last_gen: u64,
    /// Seconds since a new analysis window arrived.
    stale: f32,
    agc: AutoGain,
    /// Tilted linear band magnitudes from the last complete window.
    magnitudes: Vec<f32>,
    /// Loudest of [`Self::magnitudes`]. The normalizer keeps slewing toward it
    /// on frames that do not carry a new window.
    last_mag_peak: f32,
    /// Channel-meter targets from the last complete window, `0..=1`.
    meter_input: [f32; 4],
    /// Blended bar targets from the last push, one per analyzer bar.
    bar_targets: Vec<f32>,
    /// Smoothed bar heights from the same push, kept for column sampling.
    bar_levels: Vec<f32>,
    /// Peak hold for each displayed spectrum column. Empty until the panel
    /// reports a width. Indexed by column, never shared between columns.
    column_peaks: Vec<PeakMark>,
    column_count: usize,
    /// Seconds of ballistics waiting for the first column width.
    pending_dt: f32,
    /// Gain already folded into [`Self::column_peaks`] and the bar peaks.
    applied_gain: f32,
}

impl Default for VizState {
    fn default() -> Self {
        Self {
            bars: vec![Meter::default(); BARS],
            meters: [Meter::default(); 4],
            stereo: Vec::new(),
            phase: 0.0,
            last_gen: 0,
            stale: 0.0,
            agc: AutoGain::default(),
            magnitudes: Vec::new(),
            last_mag_peak: 0.0,
            meter_input: [0.0; 4],
            bar_targets: Vec::new(),
            bar_levels: Vec::new(),
            column_peaks: Vec::new(),
            column_count: 0,
            pending_dt: 0.0,
            applied_gain: 1.0,
        }
    }
}

impl VizState {
    /// A dark analyzer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one UI frame.
    ///
    /// `snapshot` is the latest mix window, or `None` when the callback has
    /// not published yet (including a torn read the bus refused). A snapshot
    /// whose generation has not changed does not recompute the FFT and does
    /// not clear the meters: the last complete window stays the target and
    /// the ballistics advance by `dt`. After [`STALE_AFTER`] seconds with no
    /// new window, levels ease toward silence instead of jumping there.
    /// `dt` is seconds since the previous call.
    pub fn tick(&mut self, snapshot: Option<&VizSnapshot>, dt: f32) {
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 0.25)
        } else {
            0.0
        };
        let fresh =
            snapshot.filter(|snap| snap.gen != 0 && snap.gen != self.last_gen && snap.rate > 0);
        if let Some(snap) = fresh {
            self.last_gen = snap.gen;
            self.stale = 0.0;
            self.ingest(snap, dt);
        } else if self.last_gen != 0 {
            self.stale += dt;
            if self.stale >= STALE_AFTER {
                self.release(dt);
            } else {
                self.coast(dt);
            }
        }
        let energy = self.meters.iter().map(|meter| meter.level).sum::<f32>() / 4.0;
        self.phase = (self.phase + dt * (0.7 + energy * 2.2)) % std::f32::consts::TAU;
    }

    fn ingest(&mut self, snap: &VizSnapshot, dt: f32) {
        self.magnitudes = spectrum_magnitudes(&snap.stereo, snap.rate, BARS);
        self.last_mag_peak = self.magnitudes.iter().copied().fold(0.0f32, f32::max);
        let _ = self.agc.update(self.last_mag_peak, dt);
        self.push_bars(dt);
        for (slot, peak) in self.meter_input.iter_mut().zip(snap.peaks) {
            *slot = channel_unit(peak);
        }
        self.push_meters(dt);
        self.stereo.clear();
        let frames = (snap.stereo.len() / 2).min(WINDOW);
        self.stereo.reserve(frames * 2);
        for index in 0..frames {
            self.stereo
                .push(f32::from(snap.stereo[index * 2]) / 32768.0);
            self.stereo
                .push(f32::from(snap.stereo[index * 2 + 1]) / 32768.0);
        }
    }

    /// Keep the last window. Gain and bar height still move with `dt`.
    fn coast(&mut self, dt: f32) {
        let _ = self.agc.update(self.last_mag_peak, dt);
        self.push_bars(dt);
        self.push_meters(dt);
    }

    fn push_bars(&mut self, dt: f32) {
        let gain = self.agc.gain();
        let levels: Vec<f32> = self
            .magnitudes
            .iter()
            .map(|level| db_unit(*level * gain))
            .collect();
        self.apply_bar_targets(&levels, dt);
    }

    fn push_meters(&mut self, dt: f32) {
        let ballistics = Ballistics::meter();
        for (meter, input) in self.meters.iter_mut().zip(self.meter_input) {
            meter.update(input, dt, ballistics);
        }
    }

    fn release(&mut self, dt: f32) {
        // Below the gate the normalizer holds, so a gap does not crank the gain.
        let _ = self.agc.update(0.0, dt);
        let silent = vec![0.0; self.bars.len()];
        self.apply_bar_targets(&silent, dt);
        let meter_ballistics = Ballistics::meter();
        for meter in &mut self.meters {
            meter.update(0.0, dt, meter_ballistics);
        }
    }

    /// Smooth `raw` into the bars, then advance one peak per displayed column.
    ///
    /// Neighbor blending happens here, on the bar targets. The column marks
    /// are updated afterwards from those smoothed values, each from its own
    /// sample, so a spike cannot be copied onto the next column's cap.
    fn apply_bar_targets(&mut self, raw: &[f32], dt: f32) {
        self.rescale_held_peaks();
        self.bar_targets = smooth_spectrum(&mut self.bars, raw, dt);
        self.bar_levels = self.bars.iter().map(|meter| meter.level).collect();
        self.advance_columns(dt);
    }

    /// Keep held marks in the same display units as the bars when gain moves.
    fn rescale_held_peaks(&mut self) {
        let gain = self.agc.gain();
        let shift = display_shift(self.applied_gain, gain);
        self.applied_gain = gain;
        if shift.abs() < 1.0e-6 {
            return;
        }
        for meter in &mut self.bars {
            meter.rescale_peak(shift);
        }
        for mark in &mut self.column_peaks {
            mark.rescale(shift);
        }
    }

    fn advance_columns(&mut self, dt: f32) {
        if self.column_count == 0 {
            self.pending_dt = (self.pending_dt + dt).min(1.0);
            return;
        }
        if self.column_peaks.len() != self.column_count {
            self.column_peaks
                .resize(self.column_count, PeakMark::default());
        }
        let columns = self.column_count;
        let ballistics = Ballistics::spectrum();
        for index in 0..columns {
            let target = sample_series(&self.bar_targets, columns, index);
            let shown = sample_series(&self.bar_levels, columns, index);
            let mark = &mut self.column_peaks[index];
            mark.update(target, dt, ballistics);
            if mark.value < shown {
                mark.value = shown;
            }
        }
    }

    /// How many spectrum columns the panel is painting.
    ///
    /// Peak state is per column. The first call replays any ballistics time
    /// that arrived before the width was known. Later calls resample the
    /// existing marks when the terminal width changes.
    pub fn set_column_count(&mut self, columns: usize) {
        if columns == self.column_count {
            return;
        }
        if columns == 0 {
            self.column_peaks.clear();
            self.column_count = 0;
            return;
        }
        let first = self.column_count == 0;
        self.column_peaks = resample_peaks(&self.column_peaks, columns);
        self.column_count = columns;
        if first {
            let dt = self.pending_dt;
            self.pending_dt = 0.0;
            if dt > 0.0 && !self.bar_targets.is_empty() {
                self.advance_columns(dt);
            }
        }
    }

    /// Smoothed height of display column `index`, `0..=1`.
    pub fn column_level(&self, index: usize) -> f32 {
        sample_series(&self.bar_levels, self.column_count, index)
    }

    /// Peak-hold mark for display column `index`.
    pub fn column_peak(&self, index: usize) -> f32 {
        self.column_peaks
            .get(index)
            .map(|mark| mark.value)
            .unwrap_or(0.0)
    }

    /// Smoothed bar height, `0..=1`.
    pub fn bar(&self, index: usize) -> f32 {
        self.bars.get(index).map(|meter| meter.level).unwrap_or(0.0)
    }

    /// Peak-hold mark for that bar.
    pub fn bar_peak(&self, index: usize) -> f32 {
        self.bars.get(index).map(|meter| meter.peak).unwrap_or(0.0)
    }

    /// Number of spectrum bars.
    pub fn bar_count(&self) -> usize {
        self.bars.len()
    }

    /// Smoothed channel level and its peak-hold mark.
    pub fn meter(&self, index: usize) -> (f32, f32) {
        self.meters
            .get(index)
            .map(|meter| (meter.level, meter.peak))
            .unwrap_or((0.0, 0.0))
    }

    /// Latest stereo window, interleaved, about `-1..=1`.
    pub fn stereo(&self) -> &[f32] {
        &self.stereo
    }

    /// Rotation for the scope shapes, in radians.
    pub fn phase(&self) -> f32 {
        self.phase
    }
}

/// Nearest mark at the new width. An empty history starts at zero. A resize
/// copies the nearest old mark once; the next frame tracks each column alone.
fn resample_peaks(old: &[PeakMark], new_len: usize) -> Vec<PeakMark> {
    if new_len == 0 {
        return Vec::new();
    }
    if old.is_empty() {
        return vec![PeakMark::default(); new_len];
    }
    (0..new_len)
        .map(|index| {
            let slot = if new_len == 1 {
                0
            } else {
                let pos = index as f32 * (old.len() as f32 - 1.0) / (new_len as f32 - 1.0);
                (pos.round() as usize).min(old.len() - 1)
            };
            old[slot]
        })
        .collect()
}

/// Map a linear peak in `0..=1` through [`METER_FLOOR_DB`].
///
/// Full scale stays at the top. A typical loud channel (roughly a quarter to
/// a half of [`CHANNEL_PEAK_SCALE`]) lands around 60–80% instead of looking
/// half empty. Silence and non-finite values are 0.
pub fn meter_curve(linear: f32) -> f32 {
    if !linear.is_finite() || linear <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * linear.clamp(0.0, 1.0).max(1.0e-8).log10();
    ((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0)
}

/// Map a mixer peak onto `0..=1` with [`meter_curve`].
pub fn channel_unit(peak: u16) -> f32 {
    meter_curve(f32::from(peak) / f32::from(CHANNEL_PEAK_SCALE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_snapshot(hz: f32, peaks: [u16; 4], gen: u64) -> VizSnapshot {
        let mut stereo = [0i16; WINDOW * 2];
        for index in 0..WINDOW {
            let sample = ((std::f32::consts::TAU * hz * index as f32) / 44_100.0).sin();
            let value = (sample * 18_000.0) as i16;
            stereo[index * 2] = value;
            stereo[index * 2 + 1] = value / 3;
        }
        VizSnapshot {
            stereo,
            peaks,
            rate: 44_100,
            gen,
        }
    }

    #[test]
    fn a_tone_raises_a_bar_and_the_matching_meter() {
        let mut state = VizState::new();
        let snap = sine_snapshot(440.0, [7000, 0, 100, 0], 1);
        state.tick(Some(&snap), 0.05);
        let mut peak_bar = 0usize;
        let mut peak_level = 0.0f32;
        for index in 0..state.bar_count() {
            let level = state.bar(index);
            if level > peak_level {
                peak_level = level;
                peak_bar = index;
            }
        }
        assert!(peak_level > 0.4, "bar {peak_bar} = {peak_level}");
        assert!(state.bar(0) + 0.15 < peak_level);
        let (level, peak) = state.meter(0);
        assert!(level > 0.7, "{level}");
        assert!(peak >= level);
        assert_eq!(state.meter(1).0, 0.0);
        assert!(state.meter(2).0 < 0.05, "{:?}", state.meter(2));
        assert!(!state.stereo().is_empty());
    }

    #[test]
    fn silence_after_a_gap_lets_the_meter_fall() {
        let mut state = VizState::new();
        let snap = sine_snapshot(220.0, [8000, 0, 0, 0], 1);
        state.tick(Some(&snap), 0.05);
        let before = state.meter(0).0;
        state.tick(Some(&snap), 0.05);
        assert!(
            state.meter(0).0 + 0.02 >= before,
            "same generation dropped the meter {} -> {}",
            before,
            state.meter(0).0
        );
        state.tick(None, 0.10);
        state.tick(None, 0.20);
        assert!(state.meter(0).0 < before, "{}", state.meter(0).0);
    }

    #[test]
    fn a_duplicate_or_torn_snapshot_does_not_reset_the_spectrum() {
        let mut state = VizState::new();
        let mut snap = sine_snapshot(440.0, [8000, 0, 0, 0], 1);
        state.tick(Some(&snap), 0.05);
        let level = max_bar(&state);
        let peak = max_peak(&state);
        assert!(
            level > 0.3 && peak + 0.001 >= level,
            "level {level} peak {peak}"
        );
        for _ in 0..4 {
            state.tick(Some(&snap), 0.02);
        }
        assert!(
            max_bar(&state) > level * 0.75,
            "duplicate window drained the bar to {}",
            max_bar(&state)
        );
        assert!(
            max_peak(&state) + 0.02 >= peak,
            "peak hold cleared {} -> {}",
            peak,
            max_peak(&state)
        );
        state.tick(None, 0.02);
        assert!(
            max_bar(&state) > level * 0.6,
            "a torn read wiped the bar to {}",
            max_bar(&state)
        );

        snap.gen = 2;
        snap.stereo = [0; WINDOW * 2];
        snap.peaks = [0; 4];
        state.tick(Some(&snap), 0.02);
        assert!(
            max_peak(&state) + 0.02 >= peak * 0.9,
            "a new quiet window snapped the cap off {}",
            max_peak(&state)
        );
    }

    #[test]
    fn a_quiet_tone_rises_with_the_normalizer_instead_of_jumping() {
        let mut state = VizState::new();
        let mut snap = sine_snapshot(440.0, [0, 0, 0, 0], 1);
        for index in 0..WINDOW {
            let sample = ((std::f32::consts::TAU * 440.0 * index as f32) / 44_100.0).sin();
            let value = (sample * 1_600.0) as i16;
            snap.stereo[index * 2] = value;
            snap.stereo[index * 2 + 1] = value;
        }
        state.tick(Some(&snap), 0.05);
        let early = max_bar(&state);
        for generation in 2..50 {
            snap.gen = generation;
            state.tick(Some(&snap), 0.05);
        }
        let later = max_bar(&state);
        assert!(early < 0.8, "first frame already filled the panel: {early}");
        assert!(later > early + 0.1, "early {early}, later {later}");
        assert!(later > 0.75, "quiet tone never came up: {later}");
    }

    #[test]
    fn meter_curve_lifts_a_loud_channel_into_the_upper_half() {
        let half = meter_curve(0.5);
        let quarter = meter_curve(0.25);
        assert!((0.70..=0.90).contains(&half), "{half}");
        assert!((0.55..=0.75).contains(&quarter), "{quarter}");
        assert_eq!(meter_curve(1.0), 1.0);
        assert_eq!(meter_curve(0.0), 0.0);
        assert_eq!(channel_unit(0), 0.0);
        // 100 / 8192 is about −38 dB, under the floor.
        assert!(channel_unit(100) < 0.05, "{}", channel_unit(100));
    }

    #[test]
    fn a_gain_increase_does_not_promote_a_quieter_column_target() {
        let mut state = VizState::new();
        state.set_column_count(3);
        state.applied_gain = 1.0;
        state.agc.set_display_gain(1.0);
        for mark in &mut state.column_peaks {
            mark.update(0.70, 0.0, Ballistics::spectrum());
        }
        // +3.6 dB of gain is +0.10 on the 36 dB display. The new target is
        // only +0.05, so the held magnitude is still above it.
        state.agc.set_display_gain(10f32.powf(3.6 / 20.0));
        state.apply_bar_targets(&[0.75, 0.75, 0.75], 0.05);
        assert!(
            state.column_peak(0) > 0.78,
            "quieter target replaced the held mark: {}",
            state.column_peak(0)
        );
        assert!(
            (state.column_peak(1) - state.column_peak(0)).abs() < 1.0e-4,
            "columns diverged under the same target"
        );
        assert!(state.column_peak(0) + 1.0e-4 >= state.column_level(0));
    }

    #[test]
    fn a_spike_in_one_bar_does_not_copy_its_cap_onto_a_distant_column() {
        let mut state = VizState::new();
        state.set_column_count(5);
        state.apply_bar_targets(&[0.0, 0.0, 1.0, 0.0, 0.0], 0.05);
        assert!(
            state.column_peak(2) > 0.60,
            "center {}",
            state.column_peak(2)
        );
        assert!(
            state.column_peak(0) < 0.05,
            "edge took the spike: {}",
            state.column_peak(0)
        );
        assert!(
            state.column_peak(1) < 0.30,
            "neighbor cap cloned the spike: {}",
            state.column_peak(1)
        );
        assert!(state.column_peak(1) + 1.0e-3 >= state.column_level(1));
        let held = state.column_peak(2);
        for _ in 0..8 {
            state.apply_bar_targets(&[0.0; 5], 0.10);
        }
        assert!(
            state.column_peak(2) + 0.03 >= held,
            "cap fell during the hold: {held} -> {}",
            state.column_peak(2)
        );
        for _ in 0..8 {
            state.apply_bar_targets(&[0.0; 5], 0.10);
        }
        assert!(
            state.column_peak(2) < held - 0.15,
            "cap never fell: {}",
            state.column_peak(2)
        );
        assert!(state.column_peak(2) + 1.0e-3 >= state.column_level(2));
        assert!(state.column_peak(0) < 0.05);
    }

    #[test]
    fn a_tone_holds_its_column_then_the_cap_falls_through_silence() {
        let mut state = VizState::new();
        state.set_column_count(BARS);
        let mut snap = sine_snapshot(440.0, [4000, 0, 0, 0], 1);
        state.tick(Some(&snap), 0.05);
        let (loud, _) = (0..state.column_count)
            .map(|index| (index, state.column_level(index)))
            .max_by(|left, right| left.1.partial_cmp(&right.1).unwrap())
            .unwrap();
        let quiet = if loud < BARS / 2 { BARS - 1 } else { 0 };
        assert!(
            state.column_peak(loud) > state.column_peak(quiet) + 0.2,
            "loud {} quiet {}",
            state.column_peak(loud),
            state.column_peak(quiet)
        );
        let held = state.column_peak(loud);
        snap.stereo = [0; WINDOW * 2];
        snap.peaks = [0; 4];
        for step in 0..14 {
            snap.gen = 2 + step;
            state.tick(Some(&snap), 0.05);
            assert!(
                state.column_peak(loud) + 1.0e-3 >= state.column_level(loud),
                "cap under the bar"
            );
        }
        assert!(
            state.column_peak(loud) + 0.05 >= held.min(0.95),
            "fell inside the hold: {held} -> {}",
            state.column_peak(loud)
        );
        let mid = state.column_peak(loud);
        for step in 0..16 {
            snap.gen = 40 + step;
            state.tick(Some(&snap), 0.05);
        }
        assert!(
            state.column_peak(loud) < mid - 0.08,
            "silence did not drop the cap: {mid} -> {}",
            state.column_peak(loud)
        );
        assert!(state.column_peak(quiet) < state.column_peak(loud).max(0.2));
    }

    fn max_bar(state: &VizState) -> f32 {
        (0..state.bar_count())
            .map(|index| state.bar(index))
            .fold(0.0f32, f32::max)
    }

    fn max_peak(state: &VizState) -> f32 {
        (0..state.bar_count())
            .map(|index| state.bar_peak(index))
            .fold(0.0f32, f32::max)
    }
}
