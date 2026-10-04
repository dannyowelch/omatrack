//! Peak normalizer for the spectrum bars.
//!
//! The envelope follows the loudest tilted band. A louder frame raises it
//! quickly (fast attack, gain drops), and a quieter frame lets it fall slowly
//! (slow release, gain comes back). Silence below the gate holds the envelope
//! so a gap does not wind the gain up to the cap.

use super::ballistics::approach;

/// Time constants for [`AutoGain`]. Seconds, except the unitless limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgcParams {
    /// Envelope rise when the frame is louder than the tracked peak.
    pub attack: f32,
    /// Envelope fall when the frame is quieter.
    pub release: f32,
    /// Largest gain applied to a linear magnitude. `100` is +40 dB.
    pub max_gain: f32,
    /// Peaks below this hold the envelope instead of chasing silence.
    pub gate: f32,
    /// Fastest the applied gain may move, in dB per second.
    ///
    /// The envelope can still track quickly. The gain the bars actually
    /// multiply by is limited to this rate, so a loud frame does not resize
    /// every column in one paint.
    pub slew_db_per_sec: f32,
}

impl AgcParams {
    /// Spectrum panel. The envelope still reacts in a few frames. The gain
    /// handed to the bars moves slowly enough that a column does not pop.
    pub const fn spectrum() -> Self {
        Self {
            attack: 0.05,
            release: 0.70,
            max_gain: 100.0,
            gate: 1.0e-4,
            slew_db_per_sec: 18.0,
        }
    }
}

/// Tracked peak and the gain that places it at 0 dBFS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoGain {
    envelope: f32,
    /// Gain last returned. Slewed toward the envelope's target in dB.
    display_gain: f32,
    params: AgcParams,
}

impl Default for AutoGain {
    fn default() -> Self {
        Self::new(AgcParams::spectrum())
    }
}

impl AutoGain {
    /// Unity gain until the first real peak arrives.
    pub fn new(params: AgcParams) -> Self {
        Self {
            envelope: 1.0,
            display_gain: 1.0,
            params,
        }
    }

    /// Gain for this frame. Multiply linear band magnitudes by the result.
    ///
    /// `peak` is the loudest band, already tilted, before the dB curve. `dt`
    /// is seconds since the previous call. Non-finite inputs are treated as
    /// silence and a zero timestep.
    pub fn update(&mut self, peak: f32, dt: f32) -> f32 {
        let peak = if peak.is_finite() { peak.max(0.0) } else { 0.0 };
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let gate = if self.params.gate.is_finite() {
            self.params.gate.max(1.0e-8)
        } else {
            1.0e-4
        };
        if peak >= gate {
            let tau = if peak > self.envelope {
                self.params.attack
            } else {
                self.params.release
            };
            self.envelope = approach(self.envelope, peak, dt, tau).max(gate);
        }
        let target = self.target_gain();
        let slew = if self.params.slew_db_per_sec.is_finite() {
            self.params.slew_db_per_sec.max(0.0)
        } else {
            0.0
        };
        self.display_gain = slew_gain(self.display_gain, target, dt, slew);
        self.display_gain
    }

    /// Gain from the last [`Self::update`], without moving the envelope.
    pub fn gain(&self) -> f32 {
        self.display_gain
    }

    /// Park the applied gain. Tests use this to check peak rescaling without
    /// waiting out the envelope.
    #[cfg(test)]
    pub(crate) fn set_display_gain(&mut self, gain: f32) {
        self.display_gain = if gain.is_finite() {
            gain.max(1.0e-6)
        } else {
            1.0
        };
    }

    fn target_gain(&self) -> f32 {
        let gate = if self.params.gate.is_finite() {
            self.params.gate.max(1.0e-8)
        } else {
            1.0e-4
        };
        let max_gain = if self.params.max_gain.is_finite() {
            self.params.max_gain.max(1.0)
        } else {
            1.0
        };
        let env = if self.envelope.is_finite() {
            self.envelope.max(gate)
        } else {
            1.0
        };
        (1.0 / env).clamp(0.0, max_gain)
    }
}

/// Move `current` toward `target` by at most `db_per_sec * dt` decibels.
fn slew_gain(current: f32, target: f32, dt: f32, db_per_sec: f32) -> f32 {
    let current = if current.is_finite() && current > 0.0 {
        current
    } else {
        1.0
    };
    let target = if target.is_finite() && target > 0.0 {
        target
    } else {
        current
    };
    if db_per_sec <= 0.0 || dt <= 0.0 {
        return current;
    }
    let current_db = 20.0 * current.log10();
    let target_db = 20.0 * target.log10();
    let step = db_per_sec * dt;
    let next_db = current_db + (target_db - current_db).clamp(-step, step);
    let next = 10f32.powf(next_db / 20.0);
    if next.is_finite() {
        next
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_signal_gains_up_slowly_and_a_loud_one_cuts_fast() {
        let mut agc = AutoGain::default();
        let first = agc.update(0.05, 0.05);
        assert!(
            first < 2.0,
            "release must not jump to full compensation, gain {first}"
        );

        for _ in 0..80 {
            agc.update(0.05, 0.05);
        }
        let lifted = agc.gain();
        assert!(
            lifted > 8.0,
            "after a few seconds a −26 dB peak should be lifted, gain {lifted}"
        );
        assert!(lifted <= 100.0);

        let cut = agc.update(1.0, 0.05);
        assert!(
            cut < lifted,
            "loud peak should start reducing gain, {lifted} -> {cut}"
        );
        let dropped_db = 20.0 * (lifted / cut).log10();
        assert!(
            dropped_db < 2.0,
            "one frame popped the gain by {dropped_db:.2} dB ({lifted} -> {cut})"
        );
        for _ in 0..60 {
            agc.update(1.0, 0.05);
        }
        assert!(
            agc.gain() < 2.0,
            "gain never caught the full-scale peak: {}",
            agc.gain()
        );
    }

    #[test]
    fn silence_holds_the_gain_and_junk_input_does_not_explode() {
        let mut agc = AutoGain::default();
        agc.update(0.2, 0.2);
        let held = agc.gain();
        let after = agc.update(0.0, 2.0);
        assert!(
            (after - held).abs() < 1.0e-4,
            "silence should hold, {held} -> {after}"
        );
        let mut agc = AutoGain::default();
        let gain = agc.update(f32::NAN, f32::INFINITY);
        assert!(gain.is_finite() && gain <= 100.0, "{gain}");
        let gain = agc.update(1.0e20, 1.0);
        assert!(gain.is_finite() && (0.0..=100.0).contains(&gain), "{gain}");
    }
}
