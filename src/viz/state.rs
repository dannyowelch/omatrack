//! Smoothed levels the UI draws. The FFT runs here, on the thread that calls
//! [`VizState::tick`], never inside the audio callback.

use super::agc::AutoGain;
use super::ballistics::{approach, smooth_spectrum, Ballistics, Meter};
use super::bands::{db_unit, spectrum_magnitudes};
use super::bus::VizSnapshot;
use super::column::column_band;
use super::WINDOW;

/// Fast attack / moderate release for linear band power, in seconds.
///
/// This runs before the dB curve. A single noisy FFT bin cannot peg a bar,
/// and a real hit still arrives inside a couple of UI frames.
const BAND_ATTACK: f32 = 0.018;
const BAND_RELEASE: f32 = 0.12;
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
    /// Smoothed bar heights, one per analyzer band. Display columns read these
    /// through [`column_band`]; they are not resampled per frame.
    bar_levels: Vec<f32>,
    /// Linear power envelope, one per band, before the dB curve.
    band_env: Vec<f32>,
    column_count: usize,
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
            bar_levels: Vec::new(),
            band_env: Vec::new(),
            column_count: 0,
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
        let averaged = self.average_bands(dt);
        let levels: Vec<f32> = averaged
            .into_iter()
            .map(|level| db_unit(level * gain))
            .collect();
        self.apply_bar_targets(&levels, dt);
    }

    /// Fast attack, moderate release on linear band power.
    fn average_bands(&mut self, dt: f32) -> Vec<f32> {
        if self.band_env.len() != self.magnitudes.len() {
            self.band_env = vec![0.0; self.magnitudes.len()];
        }
        for (env, sample) in self.band_env.iter_mut().zip(&self.magnitudes) {
            let sample = if sample.is_finite() {
                sample.max(0.0)
            } else {
                0.0
            };
            let tau = if sample > *env {
                BAND_ATTACK
            } else {
                BAND_RELEASE
            };
            *env = approach(*env, sample, dt, tau);
        }
        self.band_env.clone()
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

    /// Smooth `raw` into the bars.
    ///
    /// Neighbor blending and the spectrum ballistics happen here. The panel
    /// draws [`Meter::level`] and nothing above it, so a noisy bin cannot
    /// leave a cap behind in another column.
    fn apply_bar_targets(&mut self, raw: &[f32], dt: f32) {
        let _blended = smooth_spectrum(&mut self.bars, raw, dt);
        let count = raw.len().min(self.bars.len());
        self.bar_levels = self
            .bars
            .iter()
            .take(count)
            .map(|meter| meter.level)
            .collect();
    }

    /// How many spectrum columns the panel is painting.
    ///
    /// This records the width used to map columns onto bands.
    pub fn set_column_count(&mut self, columns: usize) {
        self.column_count = columns;
    }

    /// Smoothed height of display column `index`, `0..=1`.
    pub fn column_level(&self, index: usize) -> f32 {
        let band = column_band(index, self.column_count, self.bar_levels.len());
        self.bar_levels.get(band).copied().unwrap_or(0.0)
    }

    /// Smoothed bar height, `0..=1`.
    pub fn bar(&self, index: usize) -> f32 {
        self.bars.get(index).map(|meter| meter.level).unwrap_or(0.0)
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
        assert!(level > 0.3, "level {level}");
        for _ in 0..4 {
            state.tick(Some(&snap), 0.02);
        }
        assert!(
            max_bar(&state) > level * 0.75,
            "duplicate window drained the bar to {}",
            max_bar(&state)
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
            max_bar(&state) > level * 0.5,
            "a new quiet window snapped the bar off {}",
            max_bar(&state)
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
        // The 2048-point window and the power envelope reach the same gain a
        // little later than a single 512-point frame did.
        for generation in 2..90 {
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
    fn a_spike_stays_on_its_column_and_the_bar_follows_the_smoothed_level() {
        let mut state = VizState::new();
        state.set_column_count(5);
        state.apply_bar_targets(&[0.0, 0.0, 1.0, 0.0, 0.0], 0.05);
        assert!(
            state.column_level(2) > 0.60,
            "center {}",
            state.column_level(2)
        );
        assert!(
            state.column_level(0) < 0.05,
            "edge took the spike: {}",
            state.column_level(0)
        );
        // One short frame must not finish the attack. The drawn height is the
        // smoothed level, not the raw target.
        state.set_column_count(1);
        let mut fresh = VizState::new();
        fresh.set_column_count(1);
        fresh.apply_bar_targets(&[1.0], 0.02);
        assert!(
            fresh.column_level(0) < 0.90,
            "one short frame should not finish the attack: {}",
            fresh.column_level(0)
        );
    }

    #[test]
    fn columns_that_share_a_band_share_its_level() {
        let mut state = VizState::new();
        state.set_column_count(BARS * 2);
        let mut raw = vec![0.0; BARS];
        raw[10] = 1.0;
        state.apply_bar_targets(&raw, 0.05);
        assert_eq!(column_band(20, BARS * 2, BARS), 10);
        assert_eq!(column_band(21, BARS * 2, BARS), 10);
        assert!(
            (state.column_level(20) - state.column_level(21)).abs() < 1.0e-6,
            "shared band drew two heights: {} {}",
            state.column_level(20),
            state.column_level(21)
        );
        let held = state.column_level(20);
        state.set_column_count(BARS);
        assert!(
            (state.column_level(10) - held).abs() < 1.0e-4,
            "resize moved the bar: {held} -> {}",
            state.column_level(10)
        );
        assert!(
            state.column_level(9) < held - 0.15,
            "neighbor took the bar: {} vs {held}",
            state.column_level(9)
        );
    }

    #[test]
    fn overlapped_hops_of_a_steady_tone_stay_in_one_column() {
        let mut state = VizState::new();
        state.set_column_count(BARS);
        let mut loudest_column = None;
        for step in 0..24 {
            let snap = hop_snapshot(440.0, step * crate::viz::HOP, step as u64 + 1);
            state.tick(Some(&snap), crate::viz::HOP as f32 / 44_100.0);
            if step < 6 {
                continue;
            }
            let (index, level) = loudest_column_of(&state);
            assert!(level > 0.35, "step {step} level {level}");
            if let Some(previous) = loudest_column {
                let shift = index as isize - previous as isize;
                assert!(
                    shift.abs() <= 1,
                    "steady 440 Hz hopped from column {previous} to {index}"
                );
            }
            loudest_column = Some(index);
        }
        let loud = loudest_column.expect("a column");
        let mut snap = hop_snapshot(440.0, 24 * crate::viz::HOP, 100);
        snap.stereo = [0; WINDOW * 2];
        let baseline: Vec<f32> = (0..state.column_count)
            .map(|index| state.column_level(index))
            .collect();
        let mut previous = baseline[loud];
        let mut fell = false;
        for step in 0..40 {
            snap.gen = 200 + step;
            state.tick(Some(&snap), 0.05);
            for (index, before) in baseline.iter().copied().enumerate() {
                let level = state.column_level(index);
                assert!(
                    level <= before + 0.02,
                    "column {index} rose during silence: {before} -> {level}"
                );
            }
            let level = state.column_level(loud);
            if level + 0.01 < previous {
                fell = true;
            }
            assert!(
                level <= previous + 1.0e-4,
                "column {loud} rose during the fall: {previous} -> {level}"
            );
            previous = level;
        }
        assert!(fell, "the bar never fell after the tone stopped");
        assert!(
            state.column_level(loud) < baseline[loud] - 0.08,
            "silence did not drop the bar: {} -> {}",
            baseline[loud],
            state.column_level(loud)
        );
    }

    fn hop_snapshot(hz: f32, origin: usize, gen: u64) -> VizSnapshot {
        let mut stereo = [0i16; WINDOW * 2];
        for index in 0..WINDOW {
            let n = origin + index;
            let sample = (std::f32::consts::TAU * hz * n as f32 / 44_100.0).sin();
            let value = (sample * 16_000.0) as i16;
            stereo[index * 2] = value;
            stereo[index * 2 + 1] = value;
        }
        VizSnapshot {
            stereo,
            peaks: [4_000, 0, 0, 0],
            rate: 44_100,
            gen,
        }
    }

    fn loudest_column_of(state: &VizState) -> (usize, f32) {
        (0..state.column_count)
            .map(|index| (index, state.column_level(index)))
            .max_by(|left, right| left.1.partial_cmp(&right.1).unwrap())
            .unwrap()
    }

    fn max_bar(state: &VizState) -> f32 {
        (0..state.bar_count())
            .map(|index| state.bar(index))
            .fold(0.0f32, f32::max)
    }
}
