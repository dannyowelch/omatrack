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
    /// Fall time of the peak mark once `hold` has elapsed.
    pub peak_decay: f32,
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
        }
    }

    /// Spectrum bars. A slightly slower release keeps the row from flickering.
    pub const fn spectrum() -> Self {
        Self {
            attack: 0.025,
            decay: 0.16,
            hold: 0.22,
            peak_decay: 0.40,
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
        if input >= self.peak {
            self.peak = input;
            self.hold_left = ballistics.hold.max(0.0);
        } else {
            self.hold_left = (self.hold_left - dt).max(0.0);
            if self.hold_left == 0.0 {
                self.peak = approach(self.peak, input, dt, ballistics.peak_decay);
            }
        }
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
