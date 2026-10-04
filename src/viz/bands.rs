//! Log-spaced FFT bands and the display curve for the spectrum bars.

use super::fft::{apply_hann, magnitudes};
use super::WINDOW;

/// One display bar, as a half-open range of FFT bins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    /// First bin, inclusive.
    pub start: usize,
    /// One past the last bin.
    pub end: usize,
}

/// Lowest frequency the spectrum tries to show.
pub const F_MIN_HZ: f32 = 40.0;
/// Highest frequency the spectrum tries to show, before the Nyquist clamp.
pub const F_MAX_HZ: f32 = 16_000.0;

/// Map `bars` onto FFT bins from `f_min` to `f_max`, logarithmically.
///
/// DC is skipped. Bands are non-empty when `fft_size` is a usable power of
/// two; otherwise every band is `(0, 0)`, which [`band_levels`] treats as
/// silence. `end` is exclusive and never past `fft_size / 2`.
pub fn log_bands(
    bars: usize,
    fft_size: usize,
    sample_rate: f32,
    f_min: f32,
    f_max: f32,
) -> Vec<Band> {
    if bars == 0 {
        return Vec::new();
    }
    let bins = fft_size / 2;
    if fft_size < 8
        || !fft_size.is_power_of_two()
        || sample_rate <= 1.0
        || !f_min.is_finite()
        || !f_max.is_finite()
        || bins < 2
    {
        return vec![Band { start: 0, end: 0 }; bars];
    }
    let nyquist = sample_rate * 0.5;
    let low = f_min.clamp(1.0, nyquist.max(1.0));
    let high = f_max.clamp(low, nyquist.max(low));
    let log_low = low.ln();
    let log_span = (high.ln() - log_low).max(0.0);
    let mut out = Vec::with_capacity(bars);
    for index in 0..bars {
        let t0 = index as f32 / bars as f32;
        let t1 = (index + 1) as f32 / bars as f32;
        let hz0 = (log_low + log_span * t0).exp();
        let hz1 = (log_low + log_span * t1).exp();
        let start = hz_to_bin(hz0, fft_size, sample_rate).clamp(1, bins - 1);
        let end = hz_to_bin(hz1, fft_size, sample_rate).clamp(start + 1, bins);
        out.push(Band { start, end });
    }
    out
}

fn hz_to_bin(hz: f32, fft_size: usize, sample_rate: f32) -> usize {
    let raw = (hz / sample_rate * fft_size as f32).round();
    if raw.is_finite() && raw > 0.0 {
        raw as usize
    } else {
        1
    }
}

/// Peak magnitude inside each band. Empty or out-of-range bands are 0.
pub fn band_levels(magnitudes: &[f32], bands: &[Band]) -> Vec<f32> {
    bands
        .iter()
        .map(|band| {
            if band.start >= band.end || band.start >= magnitudes.len() {
                0.0
            } else {
                let end = band.end.min(magnitudes.len());
                magnitudes[band.start..end]
                    .iter()
                    .copied()
                    .fold(0.0f32, f32::max)
            }
        })
        .collect()
}

/// Log-spaced bar heights in `0..=1` for an interleaved stereo window.
///
/// The mix is averaged to mono, Hann-windowed, and mapped from about −54 dB
/// to 0 dB. `bars == 0` or a zero rate yields an empty or all-zero vector of
/// the requested length and does not panic.
pub fn spectrum_bars(stereo: &[i16], rate: u32, bars: usize) -> Vec<f32> {
    if bars == 0 {
        return Vec::new();
    }
    if rate == 0 || stereo.len() < 4 {
        return vec![0.0; bars];
    }
    let mut mono = [0.0f32; WINDOW];
    let frames = stereo.len() / 2;
    let count = frames.min(WINDOW);
    let start = frames - count;
    for index in 0..count {
        let left = f32::from(stereo[(start + index) * 2]) / 32768.0;
        let right = f32::from(stereo[(start + index) * 2 + 1]) / 32768.0;
        mono[index] = (left + right) * 0.5;
    }
    apply_hann(&mut mono);
    let mags = magnitudes(&mono);
    let bands = log_bands(bars, WINDOW, rate as f32, F_MIN_HZ, F_MAX_HZ);
    band_levels(&mags, &bands)
        .into_iter()
        .map(display_level)
        .collect()
}

fn display_level(magnitude: f32) -> f32 {
    if !magnitude.is_finite() || magnitude <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * magnitude.max(1.0e-8).log10();
    ((db + 54.0) / 54.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_are_ordered_and_stay_inside_the_fft() {
        let bands = log_bands(24, 512, 44_100.0, F_MIN_HZ, F_MAX_HZ);
        assert_eq!(bands.len(), 24);
        let mut previous = 0usize;
        for band in &bands {
            assert!(band.end > band.start, "{band:?}");
            assert!(band.end <= 256, "{band:?}");
            assert!(band.start >= previous, "{band:?} after {previous}");
            previous = band.start;
        }
        assert!(bands.first().unwrap().start < bands.last().unwrap().start);
        assert!(bands.last().unwrap().end > bands[bands.len() / 2].end);
    }

    #[test]
    fn a_low_rate_or_tiny_fft_does_not_panic() {
        let bands = log_bands(8, 4, 44_100.0, 40.0, 16_000.0);
        assert!(bands.iter().all(|band| band.start == 0 && band.end == 0));
        let levels = band_levels(&[0.0, 1.0], &bands);
        assert_eq!(levels, vec![0.0; 8]);
        assert!(log_bands(0, 512, 44_100.0, 40.0, 1000.0).is_empty());
    }

    #[test]
    fn higher_tones_light_higher_bars() {
        let low = tone(110.0);
        let high = tone(3_500.0);
        let low_bars = spectrum_bars(&low, 44_100, 32);
        let high_bars = spectrum_bars(&high, 44_100, 32);
        let low_at = argmax(&low_bars);
        let high_at = argmax(&high_bars);
        assert!(
            high_at > low_at,
            "low bar {low_at}, high bar {high_at}\n{low_bars:?}\n{high_bars:?}"
        );
        assert!(low_bars[low_at] > 0.45, "{}", low_bars[low_at]);
        assert!(
            low_bars[low_at] > low_bars[low_bars.len() - 1] + 0.2,
            "low tone leaked into the top bar: {low_bars:?}"
        );
        assert!(
            high_bars[high_at] > high_bars[0] + 0.2,
            "high tone leaked into the bottom bar: {high_bars:?}"
        );
        assert!(spectrum_bars(&vec![0i16; WINDOW * 2], 44_100, 16)
            .iter()
            .all(|level| *level < 0.05));
    }

    fn tone(hz: f32) -> Vec<i16> {
        (0..WINDOW)
            .flat_map(|index| {
                let sample = ((std::f32::consts::TAU * hz * index as f32) / 44_100.0).sin();
                let value = (sample * 16_000.0) as i16;
                [value, value]
            })
            .collect()
    }

    fn argmax(values: &[f32]) -> usize {
        values
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .map(|(index, _)| index)
            .unwrap()
    }
}
