//! Meter smoothing: fast attack, slower decay, and a peak mark that holds.

/// Time constants for a level meter. All values are seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ballistics {
    /// Time to cover about 63% of a rise.
    pub attack: f32,
    /// Time to cover about 63% of a fall.
    pub decay: f32,
    /// How long [`Meter::peak`] stays put after a new high.
    pub hold: f32,
    /// Fall of the peak mark once `hold` has elapsed.
    ///
    /// An exponential time constant when [`Self::linear_peak`] is false.
    /// Seconds to drop from 1 to 0 at a constant rate when it is true.
    pub peak_decay: f32,
    /// Drop the peak at a constant rate instead of chasing the input.
    pub linear_peak: bool,
}

impl Default for Ballistics {
    fn default() -> Self {
        Self::meter()
    }
}

impl Ballistics {
    /// Channel meters. The attack is short enough that one UI frame pegs a hit.
    pub const fn meter() -> Self {
        Self {
            attack: 0.015,
            decay: 0.28,
            hold: 0.40,
            peak_decay: 0.70,
            linear_peak: false,
        }
    }

    /// Spectrum bars. Attack is a frame or two. Release is a fraction of a
    /// second, in real time. The cap holds, then drops at a constant rate.
    pub const fn spectrum() -> Self {
        Self {
            attack: 0.020,
            decay: 0.40,
            hold: 0.80,
            peak_decay: 1.70,
            linear_peak: true,
        }
    }
}

/// One smoothed level plus its peak-hold mark, both in `0..=1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Meter {
    /// Ballistics-smoothed input.
    pub level: f32,
    /// Highest recent input, held and then released.
    pub peak: f32,
    hold_left: f32,
}

impl Default for Meter {
    fn default() -> Self {
        Self {
            level: 0.0,
            peak: 0.0,
            hold_left: 0.0,
        }
    }
}

impl Meter {
    /// Advance by `dt` seconds toward `input` (`0..=1`).
    ///
    /// The level never steps past `input` on the way up, and the peak never
    /// ends the frame below the level. A new high snaps the peak and restarts
    /// the hold. After the hold, a linear peak loses height at a constant
    /// rate; an exponential one eases toward `input`. Time spent inside the
    /// hold does not also count as fall time.
    pub fn update(&mut self, input: f32, dt: f32, ballistics: Ballistics) {
        let input = if input.is_finite() {
            input.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let tau = if input > self.level {
            ballistics.attack
        } else {
            ballistics.decay
        };
        self.level = approach(self.level, input, dt, tau);
        let fall_dt = if input >= self.peak {
            self.peak = input;
            self.hold_left = ballistics.hold.max(0.0);
            0.0
        } else if self.hold_left > dt {
            self.hold_left -= dt;
            0.0
        } else {
            let fall = dt - self.hold_left;
            self.hold_left = 0.0;
            fall
        };
        if fall_dt > 0.0 {
            if ballistics.linear_peak {
                let seconds = if ballistics.peak_decay.is_finite() {
                    ballistics.peak_decay.max(1.0e-3)
                } else {
                    1.0e-3
                };
                self.peak = (self.peak - fall_dt / seconds).max(input);
            } else {
                self.peak = approach(self.peak, input, fall_dt, ballistics.peak_decay);
            }
        }
        if self.peak < self.level {
            self.peak = self.level;
        }
    }
}

/// Weights for [`blend_neighbors`]. The center keeps most of the band.
const NEIGHBOR_WEIGHT: f32 = 0.18;

/// Mild mix with the bands on either side.
///
/// A single loud bin stops speckling the row. The two ends repeat their own
/// value for the missing neighbor, so the outside bars are not pulled toward
/// silence. Non-finite samples become 0. The result stays in `0..=1`.
pub(crate) fn blend_neighbors(levels: &[f32]) -> Vec<f32> {
    let n = levels.len();
    if n == 0 {
        return Vec::new();
    }
    let sample = |index: usize| {
        let value = levels[index];
        if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    if n == 1 {
        return vec![sample(0)];
    }
    let center_weight = 1.0 - 2.0 * NEIGHBOR_WEIGHT;
    (0..n)
        .map(|index| {
            let center = sample(index);
            let left = if index == 0 {
                center
            } else {
                sample(index - 1)
            };
            let right = if index + 1 == n {
                center
            } else {
                sample(index + 1)
            };
            (left * NEIGHBOR_WEIGHT + center * center_weight + right * NEIGHBOR_WEIGHT)
                .clamp(0.0, 1.0)
        })
        .collect()
}

/// Advance `meters` by `dt` seconds toward `inputs`.
///
/// `inputs` are display levels in `0..=1`, one per bar, already on the dB
/// curve. Neighbors are blended first, then each bar uses [`Ballistics::spectrum`].
/// Extra meters, past the end of `inputs`, are left alone.
pub fn smooth_spectrum(meters: &mut [Meter], inputs: &[f32], dt: f32) {
    let blended = blend_neighbors(inputs);
    let ballistics = Ballistics::spectrum();
    for (meter, input) in meters.iter_mut().zip(&blended) {
        meter.update(*input, dt, ballistics);
    }
}

pub(super) fn approach(current: f32, target: f32, dt: f32, tau: f32) -> f32 {
    if tau <= 1.0e-4 {
        return target;
    }
    let coef = 1.0 - (-dt / tau).exp();
    current + (target - current) * coef
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attack_is_quick_and_decay_is_not() {
        let mut meter = Meter::default();
        let ballistics = Ballistics::meter();
        meter.update(1.0, 0.05, ballistics);
        assert!(meter.level > 0.9, "attack {}", meter.level);
        assert!((meter.peak - 1.0).abs() < 1.0e-4);

        let held = meter.level;
        meter.update(0.0, 0.02, ballistics);
        assert!(meter.level < held);
        assert!(meter.level > 0.8, "decay {}", meter.level);
        assert!(
            (meter.peak - 1.0).abs() < 1.0e-3,
            "peak should still be holding, {}",
            meter.peak
        );
    }

    #[test]
    fn the_peak_holds_and_then_falls() {
        let ballistics = Ballistics {
            attack: 0.0,
            decay: 0.05,
            hold: 0.30,
            peak_decay: 0.10,
            linear_peak: false,
        };
        let mut meter = Meter::default();
        meter.update(1.0, 0.0, ballistics);
        meter.update(0.0, 0.20, ballistics);
        assert!(
            (meter.peak - 1.0).abs() < 1.0e-3,
            "still holding {}",
            meter.peak
        );
        meter.update(0.0, 0.20, ballistics);
        assert!(meter.peak < 0.95, "released {}", meter.peak);
        assert!(meter.peak > 0.1, "not snapped to zero {}", meter.peak);
    }

    #[test]
    fn spectrum_peak_holds_then_falls_at_a_constant_rate() {
        let ballistics = Ballistics::spectrum();
        assert!((0.5..=1.0).contains(&ballistics.hold));
        assert!(ballistics.linear_peak);
        let mut meter = Meter::default();
        meter.update(1.0, 0.05, ballistics);
        assert!(meter.level > 0.7, "attack {}", meter.level);
        assert!((meter.peak - 1.0).abs() < 1.0e-3);
        // The opening frame arms the hold; it does not spend it.
        let hold_frames = (ballistics.hold / 0.05).round() as usize;
        for _ in 0..hold_frames {
            meter.update(0.0, 0.05, ballistics);
            assert!(
                (meter.peak - 1.0).abs() < 1.0e-3,
                "still holding {}",
                meter.peak
            );
            assert!(meter.peak + 1.0e-4 >= meter.level);
        }
        let mut previous = meter.peak;
        let mut dropped = 0.0f32;
        for _ in 0..10 {
            meter.update(0.0, 0.05, ballistics);
            assert!(
                meter.peak <= previous + 1.0e-4,
                "{previous} -> {}",
                meter.peak
            );
            assert!(meter.peak + 1.0e-3 >= meter.level);
            dropped += previous - meter.peak;
            previous = meter.peak;
        }
        let expected = 0.50 / ballistics.peak_decay;
        assert!(
            (dropped - expected).abs() < 0.04,
            "dropped {dropped}, expected about {expected}"
        );
    }

    #[test]
    fn a_noisy_steady_spectrum_stays_bounded_and_a_transient_does_not_spike() {
        let mut meters = vec![Meter::default(); 5];
        let mut previous = [0.0f32; 5];
        let mut seen_max = 0.0f32;
        for frame in 0..50 {
            let mut inputs = vec![0.0f32; 5];
            for (index, input) in inputs.iter_mut().enumerate() {
                let wobble = ((frame * 3 + index * 5) % 7) as f32 / 7.0;
                *input = 0.42 + 0.08 * wobble;
            }
            let frame_max = inputs.iter().copied().fold(0.0f32, f32::max);
            seen_max = seen_max.max(frame_max);
            smooth_spectrum(&mut meters, &inputs, 0.02);
            for (index, meter) in meters.iter().enumerate() {
                assert!(
                    meter.level <= previous[index].max(frame_max) + 1.0e-3,
                    "frame {frame} bar {index} jumped to {} from {} toward {frame_max}",
                    meter.level,
                    previous[index]
                );
                assert!(meter.peak + 1.0e-3 >= meter.level);
                assert!((0.0..=1.0).contains(&meter.level));
                assert!((0.0..=1.0).contains(&meter.peak));
                assert!(meter.peak <= seen_max + 1.0e-3);
                previous[index] = meter.level;
            }
        }
        let spread = meters
            .iter()
            .map(|meter| meter.level)
            .fold((1.0f32, 0.0f32), |acc, level| {
                (acc.0.min(level), acc.1.max(level))
            });
        assert!(
            spread.1 - spread.0 < 0.2,
            "steady row still speckled: {spread:?}"
        );

        // One loud frame in the middle band, then back to the quiet floor.
        let spike = [0.30, 0.30, 0.95, 0.30, 0.30];
        let before = meters[2].level;
        smooth_spectrum(&mut meters, &spike, 0.02);
        assert!(meters[2].level <= before.max(0.95) + 1.0e-3);
        assert!(meters[2].level > before);
        assert!(meters[2].peak + 1.0e-3 >= meters[2].level);
        assert!(meters[0].peak < 0.7, "spike leaked into the side bars");
        let held = meters[2].peak;
        for _ in 0..20 {
            smooth_spectrum(&mut meters, &[0.30; 5], 0.02);
        }
        assert!(
            (meters[2].peak - held).abs() < 0.02,
            "cap fell during the hold: {held} -> {}",
            meters[2].peak
        );
        let mut previous_peak = meters[2].peak;
        for _ in 0..40 {
            smooth_spectrum(&mut meters, &[0.20; 5], 0.05);
            assert!(meters[2].peak <= previous_peak + 1.0e-4);
            assert!(meters[2].peak + 1.0e-3 >= meters[2].level);
            previous_peak = meters[2].peak;
        }
        assert!(
            meters[2].peak < held - 0.08,
            "cap never released: {}",
            meters[2].peak
        );
    }

    #[test]
    fn non_finite_input_is_silence() {
        let mut meter = Meter::default();
        meter.update(0.5, 1.0, Ballistics::meter());
        let before = meter.level;
        meter.update(f32::NAN, 0.0, Ballistics::meter());
        assert_eq!(meter.level, before);
        meter.update(f32::INFINITY, 1.0, Ballistics::meter());
        assert!(meter.level <= 1.0);
    }
}
