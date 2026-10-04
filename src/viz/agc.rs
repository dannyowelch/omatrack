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
}

impl AgcParams {
    /// Spectrum panel. Attack is a couple of UI frames; release is most of a second.
    pub const fn spectrum() -> Self {
        Self {
            attack: 0.035,
            release: 0.70,
            max_gain: 100.0,
            gate: 1.0e-4,
        }
    }
}

/// Tracked peak and the gain that places it at 0 dBFS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoGain {
    envelope: f32,
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
        let env = self.envelope.max(gate);
        let max_gain = if self.params.max_gain.is_finite() {
            self.params.max_gain.max(1.0)
        } else {
            1.0
        };
        (1.0 / env).clamp(0.0, max_gain)
    }

    /// Gain from the last [`Self::update`], without moving the envelope.
    pub fn gain(&self) -> f32 {
        let gate = self.params.gate.max(1.0e-8);
        let max_gain = self.params.max_gain.max(1.0);
        (1.0 / self.envelope.max(gate)).clamp(0.0, max_gain)
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
            cut < lifted * 0.5,
            "fast attack should drop gain from {lifted} to {cut}"
        );
        assert!(cut < 4.0, "one frame of a full-scale peak, gain {cut}");
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
