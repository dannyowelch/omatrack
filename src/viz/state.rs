//! Smoothed levels the UI draws. The FFT runs here, on the thread that calls
//! [`VizState::tick`], never inside the audio callback.

use super::ballistics::{Ballistics, Meter};
use super::bands::spectrum_bars;
use super::bus::VizSnapshot;
use super::WINDOW;
use crate::player::CHANNEL_PEAK_SCALE;

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
    stale: f32,
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
    /// not published yet. A snapshot whose generation has not changed does not
    /// recompute the FFT. After a short gap with no new audio, levels fall
    /// toward silence. `dt` is seconds since the previous call.
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
        } else {
            self.stale += dt;
            if self.stale >= 0.08 {
                self.release(dt);
            }
        }
        let energy = self.meters.iter().map(|meter| meter.level).sum::<f32>() / 4.0;
        self.phase = (self.phase + dt * (0.7 + energy * 2.2)) % std::f32::consts::TAU;
    }

    fn ingest(&mut self, snap: &VizSnapshot, dt: f32) {
        let targets = spectrum_bars(&snap.stereo, snap.rate, BARS);
        let ballistics = Ballistics::spectrum();
        for (meter, target) in self.bars.iter_mut().zip(targets) {
            meter.update(target, dt, ballistics);
        }
        let meter_ballistics = Ballistics::meter();
        for (meter, peak) in self.meters.iter_mut().zip(snap.peaks) {
            meter.update(channel_unit(peak), dt, meter_ballistics);
        }
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

    fn release(&mut self, dt: f32) {
        let ballistics = Ballistics::spectrum();
        for meter in &mut self.bars {
            meter.update(0.0, dt, ballistics);
        }
        let meter_ballistics = Ballistics::meter();
        for meter in &mut self.meters {
            meter.update(0.0, dt, meter_ballistics);
        }
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

/// Map a mixer peak onto `0..=1`.
pub fn channel_unit(peak: u16) -> f32 {
    (f32::from(peak) / f32::from(CHANNEL_PEAK_SCALE)).clamp(0.0, 1.0)
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
        assert!((state.meter(0).0 - before).abs() < 0.02, "same generation");
        state.tick(None, 0.10);
        state.tick(None, 0.20);
        assert!(state.meter(0).0 < before, "{}", state.meter(0).0);
    }
}
