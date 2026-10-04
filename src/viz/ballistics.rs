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
    /// An exponential time constant when [`Self::linear_peak`] is false and
    /// [`Self::peak_gravity`] is 0. Seconds to drop from 1 to 0 at a constant
    /// rate when [`Self::linear_peak`] is true.
    pub peak_decay: f32,
    /// Drop the peak at a constant rate instead of chasing the input.
    pub linear_peak: bool,
    /// Downward acceleration after the hold, in display units per second squared.
    ///
    /// Above zero, the mark starts from rest when the hold ends and speeds up
    /// until it meets the bar. Zero keeps the linear or exponential fall.
    pub peak_gravity: f32,
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
            peak_gravity: 0.0,
        }
    }

    /// Spectrum bars. Attack is about one UI frame. Release is a fraction of
    /// a second. The cap holds briefly, then falls from rest with gravity
    /// until it meets the smoothed bar.
    pub const fn spectrum() -> Self {
        Self {
            attack: 0.015,
            decay: 0.16,
            hold: 0.30,
            peak_decay: 0.70,
            linear_peak: false,
            // 0.5 * 3.2 * t^2 covers the full scale in about 0.8 s, and the
            // first tenth of a second barely moves.
            peak_gravity: 3.2,
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
    fall_speed: f32,
}

impl Default for Meter {
    fn default() -> Self {
        Self {
            level: 0.0,
            peak: 0.0,
            hold_left: 0.0,
            fall_speed: 0.0,
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
        let input = sanitize_unit(input);
        let dt = sanitize_dt(dt);
        let tau = if input > self.level {
            ballistics.attack
        } else {
            ballistics.decay
        };
        self.level = approach(self.level, input, dt, tau);
        advance_peak(
            &mut self.peak,
            &mut self.hold_left,
            &mut self.fall_speed,
            input,
            dt,
            ballistics,
        );
        if self.peak < self.level {
            self.peak = self.level;
            self.fall_speed = 0.0;
        }
    }

    /// Slide the held mark by `shift` display units.
    ///
    /// Automatic gain moves the whole scale. Adding the same shift the new
    /// gain applied keeps a held mark attached to the magnitude that set it,
    /// instead of treating the louder scale as a new transient. The hold
    /// clock is left alone.
    pub(crate) fn rescale_peak(&mut self, shift: f32) {
        if !shift.is_finite() || self.peak <= 0.0 {
            return;
        }
        self.peak = (self.peak + shift).clamp(0.0, 1.0);
    }
}

/// One column's peak-hold mark, in the same `0..=1` display units as the bar.
///
/// The value is independent of every other column. Smoothing belongs on the
/// levels that are fed in, not on these marks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeakMark {
    /// Held height, `0..=1`.
    pub value: f32,
    hold_left: f32,
    /// Downward speed in display units per second. Zero while the mark is held.
    fall_speed: f32,
}

impl Default for PeakMark {
    fn default() -> Self {
        Self {
            value: 0.0,
            hold_left: 0.0,
            fall_speed: 0.0,
        }
    }
}

impl PeakMark {
    /// Advance by `dt` seconds toward `input`.
    ///
    /// A higher input snaps the mark and restarts the hold. Otherwise the
    /// hold runs out, then a linear spectrum mark falls at a constant rate
    /// and never passes below `input`.
    pub fn update(&mut self, input: f32, dt: f32, ballistics: Ballistics) {
        advance_peak(
            &mut self.value,
            &mut self.hold_left,
            &mut self.fall_speed,
            sanitize_unit(input),
            sanitize_dt(dt),
            ballistics,
        );
    }

    /// Move the mark with a gain change. See [`Meter::rescale_peak`].
    pub fn rescale(&mut self, shift: f32) {
        if !shift.is_finite() || self.value <= 0.0 {
            return;
        }
        self.value = (self.value + shift).clamp(0.0, 1.0);
    }
}

/// Advance each mark toward its own `levels` entry.
///
/// `levels[i]` is the only input mark `i` sees. A short `levels` slice leaves
/// the extra marks untouched. Non-finite levels count as silence.
pub fn advance_column_peaks(marks: &mut [PeakMark], levels: &[f32], dt: f32) {
    let ballistics = Ballistics::spectrum();
    for (mark, level) in marks.iter_mut().zip(levels) {
        let level = sanitize_unit(*level);
        mark.update(level, dt, ballistics);
        if mark.value < level {
            mark.value = level;
        }
    }
}

fn sanitize_unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn sanitize_dt(dt: f32) -> f32 {
    if dt.is_finite() {
        dt.max(0.0)
    } else {
        0.0
    }
}

fn advance_peak(
    peak: &mut f32,
    hold_left: &mut f32,
    fall_speed: &mut f32,
    input: f32,
    dt: f32,
    ballistics: Ballistics,
) {
    let fall_dt = if input >= *peak {
        *peak = input;
        *hold_left = ballistics.hold.max(0.0);
        *fall_speed = 0.0;
        0.0
    } else if *hold_left > dt {
        *hold_left -= dt;
        *fall_speed = 0.0;
        0.0
    } else {
        let fall = dt - *hold_left;
        *hold_left = 0.0;
        fall
    };
    if fall_dt <= 0.0 {
        return;
    }
    let gravity = if ballistics.peak_gravity.is_finite() {
        ballistics.peak_gravity
    } else {
        0.0
    };
    if gravity > 0.0 {
        // Semi-implicit Euler: accelerate first, then step. The mark leaves
        // the hold at rest, so the first frames barely move and later frames
        // drop faster, until the bar catches it.
        *fall_speed = (*fall_speed + gravity * fall_dt).max(0.0);
        *peak = (*peak - *fall_speed * fall_dt).max(input);
        if *peak <= input + 1.0e-6 {
            *fall_speed = 0.0;
        }
    } else if ballistics.linear_peak {
        let seconds = if ballistics.peak_decay.is_finite() {
            ballistics.peak_decay.max(1.0e-3)
        } else {
            1.0e-3
        };
        *peak = (*peak - fall_dt / seconds).max(input);
        *fall_speed = 0.0;
    } else {
        *peak = approach(*peak, input, fall_dt, ballistics.peak_decay);
        *fall_speed = 0.0;
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
/// The blend is returned so a caller can track peaks from these bar values
/// without blending the peaks a second time. Extra meters, past the end of
/// `inputs`, are left alone.
pub fn smooth_spectrum(meters: &mut [Meter], inputs: &[f32], dt: f32) -> Vec<f32> {
    let blended = blend_neighbors(inputs);
    let ballistics = Ballistics::spectrum();
    for (meter, input) in meters.iter_mut().zip(&blended) {
        meter.update(*input, dt, ballistics);
    }
    blended
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
            peak_gravity: 0.0,
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
    fn spectrum_peak_holds_then_falls_with_gravity() {
        let ballistics = Ballistics::spectrum();
        assert!((0.2..=0.45).contains(&ballistics.hold));
        assert!(ballistics.peak_gravity > 1.0);
        let mut meter = Meter::default();
        meter.update(1.0, 0.05, ballistics);
        assert!(meter.level > 0.9, "attack {}", meter.level);
        assert!((meter.peak - 1.0).abs() < 1.0e-3);
        // The opening frame arms the hold; it does not spend it.
        let mut elapsed = 0.05f32;
        while elapsed + 1.0e-4 < ballistics.hold {
            meter.update(0.0, 0.05, ballistics);
            elapsed += 0.05;
            assert!(
                (meter.peak - 1.0).abs() < 1.0e-3,
                "still holding at {elapsed}: {}",
                meter.peak
            );
            assert!(meter.peak + 1.0e-4 >= meter.level);
        }
        meter.update(0.0, ballistics.hold - elapsed, ballistics);
        assert!(
            (meter.peak - 1.0).abs() < 1.0e-3,
            "boundary frame dropped the cap to {}",
            meter.peak
        );

        meter.update(0.0, 0.10, ballistics);
        let early = 1.0 - meter.peak;
        assert!(
            (0.005..0.08).contains(&early),
            "gravity should start slowly, dropped {early}"
        );
        let mut previous = meter.peak;
        for _ in 0..6 {
            meter.update(0.0, 0.05, ballistics);
            assert!(
                meter.peak <= previous + 1.0e-4,
                "fall reversed {previous} -> {}",
                meter.peak
            );
            assert!(meter.peak + 1.0e-3 >= meter.level);
            previous = meter.peak;
        }
        let before = meter.peak;
        meter.update(0.0, 0.10, ballistics);
        let later = before - meter.peak;
        assert!(
            later > early * 1.5,
            "fall should accelerate: first 0.10s dropped {early}, a later 0.10s dropped {later}"
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
    fn column_peaks_stay_independent_hold_and_then_fall() {
        let ballistics = Ballistics::spectrum();
        let mut marks = vec![PeakMark::default(); 5];
        // A spike in the middle column must not raise its neighbors.
        advance_column_peaks(&mut marks, &[0.15, 0.20, 1.0, 0.10, 0.05], 0.05);
        assert!(
            (marks[2].value - 1.0).abs() < 1.0e-3,
            "spike column {}",
            marks[2].value
        );
        assert!(
            (marks[0].value - 0.15).abs() < 1.0e-3,
            "left edge borrowed the spike: {}",
            marks[0].value
        );
        assert!(
            (marks[1].value - 0.20).abs() < 1.0e-3,
            "neighbor borrowed the spike: {}",
            marks[1].value
        );
        assert!((marks[3].value - 0.10).abs() < 1.0e-3, "{}", marks[3].value);
        assert!((marks[4].value - 0.05).abs() < 1.0e-3, "{}", marks[4].value);

        let floor = [0.15, 0.20, 0.0, 0.10, 0.05];
        let mut elapsed = 0.0f32;
        while elapsed + 1.0e-4 < ballistics.hold {
            advance_column_peaks(&mut marks, &floor, 0.10);
            elapsed += 0.10;
            assert!(
                (marks[2].value - 1.0).abs() < 1.0e-3,
                "fell during the hold at {elapsed}s: {}",
                marks[2].value
            );
            assert!((marks[1].value - 0.20).abs() < 1.0e-3);
        }
        // The frame that lands on the hold boundary does not also fall.
        let rest = ballistics.hold - elapsed;
        advance_column_peaks(&mut marks, &floor, rest);
        assert!(
            (marks[2].value - 1.0).abs() < 1.0e-3,
            "boundary frame dropped the cap to {}",
            marks[2].value
        );

        let held_height = marks[2].value;
        advance_column_peaks(&mut marks, &floor, 0.10);
        let early = held_height - marks[2].value;
        assert!(
            (0.005..0.08).contains(&early),
            "gravity should start slowly, dropped {early}"
        );
        let mut previous = marks[2].value;
        advance_column_peaks(&mut marks, &floor, 0.40);
        let later = previous - marks[2].value;
        assert!(
            later > early * 2.0,
            "fall should accelerate: 0.10s dropped {early}, the next 0.40s dropped {later}"
        );
        assert!(marks[2].value < previous);
        assert!(marks[2].value + 1.0e-4 >= floor[2]);
        previous = marks[2].value;
        advance_column_peaks(&mut marks, &floor, 0.50);
        assert!(
            marks[2].value <= previous + 1.0e-4,
            "fall reversed: {previous} -> {}",
            marks[2].value
        );
        assert!(marks[2].value + 1.0e-4 >= floor[2]);
        // Neighbors that were never spiked stay on their own floor.
        assert!((marks[1].value - 0.20).abs() < 1.0e-3);
        assert!((marks[3].value - 0.10).abs() < 1.0e-3);
    }

    #[test]
    fn a_gain_shift_does_not_turn_a_quieter_frame_into_a_new_peak() {
        let ballistics = Ballistics::spectrum();
        let mut mark = PeakMark::default();
        mark.update(0.70, 0.0, ballistics);
        // +0.10 display units of gain. The new frame is louder on screen
        // (0.75) but quieter than the magnitude the mark is holding (0.80).
        mark.rescale(0.10);
        assert!((mark.value - 0.80).abs() < 1.0e-4);
        mark.update(0.75, 0.05, ballistics);
        assert!(
            (mark.value - 0.80).abs() < 1.0e-3,
            "gain shift was taken as a new high: {}",
            mark.value
        );
        let mut elapsed = 0.05f32;
        while elapsed + 0.10 < ballistics.hold {
            mark.update(0.75, 0.10, ballistics);
            elapsed += 0.10;
            assert!(
                (mark.value - 0.80).abs() < 1.0e-3,
                "fell early at {elapsed}: {}",
                mark.value
            );
            assert!(mark.value + 1.0e-4 >= 0.75);
        }
        mark.update(0.75, ballistics.hold - elapsed + 0.40, ballistics);
        assert!(mark.value < 0.78, "cap never left the hold: {}", mark.value);
        assert!(mark.value + 1.0e-4 >= 0.75);
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
