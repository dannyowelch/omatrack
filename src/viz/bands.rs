//! Log-spaced FFT bands and the display curve for the spectrum bars.

use super::fft::{apply_hann, windowed_magnitudes};
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
///
/// A 512-point FFT at 44.1 kHz cannot separate 30 Hz from the next bin, so
/// that energy lives in the first bar rather than in a run of identical bars.
pub const F_MIN_HZ: f32 = 30.0;
/// Highest frequency the spectrum tries to show, before the Nyquist clamp.
pub const F_MAX_HZ: f32 = 16_000.0;
/// Bottom of the bar scale. 0 dBFS is the top of the panel.
///
/// A four-row panel only has 32 steps. −36 dB (rather than −60) keeps a dense
/// module from painting the lower rows solid, so the contour still moves.
/// Quieter energy than the floor disappears.
pub const DB_FLOOR: f32 = -36.0;
/// High-frequency lift, so a cymbal is not a flat line next to the bass.
pub const TILT_DB_PER_OCTAVE: f32 = 3.0;
/// Frequency that the tilt leaves unchanged. Lower bands are eased down.
const TILT_REF_HZ: f32 = 250.0;

/// Map `bars` onto FFT bins from `f_min` to `f_max`, logarithmically.
///
/// DC is skipped. Bands are contiguous, non-empty, and cover every bin from
/// the first resolved frequency through `f_max` (or Nyquist). When several
/// ideal log bands would fall inside one low bin, that bin is kept as a
/// single bar and the spare bars subdivide the wider high bands, so the left
/// edge is not a stack of copies of bin 1. A bad size yields `(0, 0)` bands,
/// which [`band_levels`] treats as silence.
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
    let low_hz = f_min.clamp(1.0, nyquist.max(1.0));
    let high_hz = f_max.clamp(low_hz, nyquist.max(low_hz));
    let first = hz_to_bin_floor(low_hz, fft_size, sample_rate).clamp(1, bins - 1);
    let last = hz_to_bin_ceil(high_hz, fft_size, sample_rate).clamp(first + 1, bins);
    partition_bins(first, last, bars)
}

fn hz_to_bin_floor(hz: f32, fft_size: usize, sample_rate: f32) -> usize {
    let raw = hz / sample_rate * fft_size as f32;
    if raw.is_finite() && raw > 0.0 {
        raw.floor() as usize
    } else {
        1
    }
}

fn hz_to_bin_ceil(hz: f32, fft_size: usize, sample_rate: f32) -> usize {
    let raw = hz / sample_rate * fft_size as f32;
    if raw.is_finite() && raw > 0.0 {
        raw.ceil() as usize
    } else {
        1
    }
}

/// Contiguous log bands over `first..last`, exactly `bars` of them when there
/// are enough bins. Extra bars split the widest ranges instead of repeating
/// the lowest bin. Too few bins repeat the top band.
fn partition_bins(first: usize, last: usize, bars: usize) -> Vec<Band> {
    let span = last.saturating_sub(first);
    if bars == 0 {
        return Vec::new();
    }
    if span == 0 {
        return vec![Band { start: 0, end: 0 }; bars];
    }
    let f0 = (first as f32 + 0.5).max(1.0);
    let f1 = (last as f32).max(f0 + 0.5);
    let log0 = f0.ln();
    let log_span = (f1.ln() - log0).max(1.0e-6);
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut current = usize::MAX;
    let mut group_start = first;
    for bin in first..last {
        let t = ((bin as f32 + 0.5).ln() - log0) / log_span;
        let bar = ((t.clamp(0.0, 0.999_999) * bars as f32) as usize).min(bars - 1);
        if bar != current {
            if current != usize::MAX {
                groups.push((group_start, bin));
            }
            current = bar;
            group_start = bin;
        }
    }
    groups.push((group_start, last));

    while groups.len() < bars {
        let Some(index) = widest(&groups) else {
            break;
        };
        let (start, end) = groups[index];
        let mid = start + (end - start) / 2;
        groups[index] = (start, mid);
        groups.insert(index + 1, (mid, end));
    }

    let mut bands: Vec<Band> = groups
        .into_iter()
        .map(|(start, end)| Band { start, end })
        .collect();
    if bands.len() > bars {
        bands.truncate(bars);
        if let Some(band) = bands.last_mut() {
            band.end = last;
        }
    }
    while bands.len() < bars {
        let last_band = bands.last().copied().unwrap_or(Band { start: 0, end: 0 });
        bands.push(last_band);
    }
    bands
}

fn widest(groups: &[(usize, usize)]) -> Option<usize> {
    groups
        .iter()
        .enumerate()
        .filter(|(_, (start, end))| end - start >= 2)
        .max_by(|left, right| {
            let left_width = left.1 .1 - left.1 .0;
            let right_width = right.1 .1 - right.1 .0;
            left_width.cmp(&right_width).then(right.0.cmp(&left.0))
        })
        .map(|(index, _)| index)
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

/// Geometric center of `band`, in hertz. An empty band is 0.
pub fn band_center_hz(band: Band, fft_size: usize, sample_rate: f32) -> f32 {
    if band.end <= band.start || fft_size == 0 || sample_rate <= 0.0 {
        return 0.0;
    }
    let lo = (band.start as f32).max(0.5);
    let hi = (band.end as f32).max(lo + 0.5);
    (lo * hi).sqrt() * sample_rate / fft_size as f32
}

/// Linear gain for `+TILT_DB_PER_OCTAVE` relative to [`TILT_REF_HZ`].
pub fn tilt_gain(freq_hz: f32) -> f32 {
    tilt_gain_at(freq_hz, TILT_REF_HZ, TILT_DB_PER_OCTAVE)
}

/// Linear gain of `db_per_octave` relative to `ref_hz`. Non-positive
/// frequencies leave the magnitude alone.
pub fn tilt_gain_at(freq_hz: f32, ref_hz: f32, db_per_octave: f32) -> f32 {
    if !freq_hz.is_finite()
        || !ref_hz.is_finite()
        || !db_per_octave.is_finite()
        || freq_hz <= 0.0
        || ref_hz <= 0.0
    {
        return 1.0;
    }
    let octaves = (freq_hz / ref_hz).log2();
    10f32.powf(db_per_octave * octaves / 20.0)
}

/// Map a linear magnitude onto `0..=1` using [`DB_FLOOR`] .. 0 dBFS.
pub fn db_unit(magnitude: f32) -> f32 {
    if !magnitude.is_finite() || magnitude <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * magnitude.max(1.0e-12).log10();
    ((db - DB_FLOOR) / -DB_FLOOR).clamp(0.0, 1.0)
}

/// Display-unit shift when linear gain moves from `old_gain` to `new_gain`.
///
/// The bar curve is logarithmic, so a gain change is an offset in `0..=1`
/// units (`+6 dB` is `6 / -DB_FLOOR`). Held peaks add this instead of reading
/// the louder scale as a new transient. A non-positive or non-finite gain
/// contributes no shift.
pub fn display_shift(old_gain: f32, new_gain: f32) -> f32 {
    if !old_gain.is_finite() || !new_gain.is_finite() || old_gain <= 1.0e-8 || new_gain <= 1.0e-8 {
        return 0.0;
    }
    let db = 20.0 * (new_gain / old_gain).log10();
    if db.is_finite() {
        db / -DB_FLOOR
    } else {
        0.0
    }
}

/// Hann-normalized, tilted band magnitudes for an interleaved stereo window.
///
/// The mix is averaged to mono. Values are linear (1.0 is about 0 dBFS before
/// tilt), not display height. [`spectrum_bars`] applies [`db_unit`] with unity
/// gain; the panel's automatic gain lives in [`super::AutoGain`] because it
/// has to remember the previous frame. `bars == 0` or a zero rate yields an
/// empty or all-zero vector and does not panic.
pub fn spectrum_magnitudes(stereo: &[i16], rate: u32, bars: usize) -> Vec<f32> {
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
    let window_sum = apply_hann(&mut mono);
    let mags = windowed_magnitudes(&mono, window_sum);
    let bands = log_bands(bars, WINDOW, rate as f32, F_MIN_HZ, F_MAX_HZ);
    let mut levels = band_levels(&mags, &bands);
    for (level, band) in levels.iter_mut().zip(&bands) {
        let hz = band_center_hz(*band, WINDOW, rate as f32);
        *level *= tilt_gain(hz);
    }
    levels
}

/// Log-spaced bar heights in `0..=1` for an interleaved stereo window.
///
/// Same analysis as [`spectrum_magnitudes`], then [`db_unit`] with no
/// automatic gain. `bars == 0` or a zero rate yields an empty or all-zero
/// vector of the requested length and does not panic.
pub fn spectrum_bars(stereo: &[i16], rate: u32, bars: usize) -> Vec<f32> {
    spectrum_magnitudes(stereo, rate, bars)
        .into_iter()
        .map(db_unit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_cover_the_range_without_repeating_the_bass_bin() {
        let bands = log_bands(48, 512, 44_100.0, F_MIN_HZ, F_MAX_HZ);
        assert_eq!(bands.len(), 48);
        assert_eq!(bands[0].start, 1, "30 Hz energy sits in the first bin");
        let mut previous = 0usize;
        for band in &bands {
            assert!(band.end > band.start, "{band:?}");
            assert!(band.end <= 256, "{band:?}");
            assert!(band.start >= previous, "{band:?} after {previous}");
            previous = band.start;
        }
        for pair in bands.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "gap at {pair:?}");
        }
        let top_hz = bands.last().unwrap().end as f32 * 44_100.0 / 512.0;
        assert!(top_hz >= 15_000.0, "top of the last band is {top_hz} Hz");
        let bass = bands
            .iter()
            .filter(|band| band.start == 1 && band.end <= 3)
            .count();
        assert_eq!(bass, 1, "the low bin was cloned: {bands:?}");
        assert!(
            bands[0].end - bands[0].start <= 2,
            "first bar swallowed the bass: {:?}",
            bands[0]
        );
        let unique = bands
            .iter()
            .map(|band| (band.start, band.end))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), bands.len(), "duplicate ranges: {bands:?}");
    }

    #[test]
    fn extra_bars_repeat_the_top_rather_than_the_bass() {
        let bands = log_bands(40, 32, 44_100.0, 30.0, 16_000.0);
        assert_eq!(bands.len(), 40);
        let bass = bands.iter().filter(|band| band.start == 1).count();
        assert!(bass <= 2, "bass cloned across {bass} bars: {bands:?}");
        assert!(bands.last().unwrap().end > bands[0].end);
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
    fn the_db_scale_maps_the_floor_through_zero() {
        assert_eq!(db_unit(0.0), 0.0);
        assert_eq!(db_unit(-1.0), 0.0);
        assert!((db_unit(1.0) - 1.0).abs() < 1.0e-4);
        let floor = 10f32.powf(DB_FLOOR / 20.0);
        assert!(db_unit(floor) < 0.02, "{}", db_unit(floor));
        let mid = 10f32.powf((DB_FLOOR / 2.0) / 20.0);
        assert!((db_unit(mid) - 0.5).abs() < 0.02, "{}", db_unit(mid));
        assert_eq!(db_unit(4.0), 1.0);
        assert_eq!(db_unit(f32::NAN), 0.0);
        // Doubling the gain is +6.02 dB, about 1/6 of the 36 dB window.
        let doubled = display_shift(1.0, 2.0);
        assert!((doubled - 6.0206 / -DB_FLOOR).abs() < 1.0e-3, "{doubled}");
        assert!(display_shift(2.0, 1.0) < 0.0);
        assert_eq!(display_shift(1.0, 0.0), 0.0);
        assert_eq!(display_shift(f32::NAN, 2.0), 0.0);
    }

    #[test]
    fn tilt_lifts_two_octaves_by_about_six_db() {
        let low = tilt_gain(1_000.0);
        let high = tilt_gain(4_000.0);
        let db = 20.0 * (high / low).log10();
        assert!((db - 6.0).abs() < 0.15, "{db}");
        assert!((tilt_gain(250.0) - 1.0).abs() < 1.0e-3);
        assert_eq!(tilt_gain(0.0), 1.0);
        assert_eq!(tilt_gain_at(100.0, 0.0, 3.0), 1.0);
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

    #[test]
    fn a_full_scale_bin_tone_reaches_the_top_of_the_scale() {
        let bars = spectrum_bars(&tone_amp(1_000.0, 0.95), 44_100, 48);
        let peak = bars.iter().copied().fold(0.0f32, f32::max);
        assert!(peak > 0.9, "full-scale tone only reached {peak}");
    }

    fn tone(hz: f32) -> Vec<i16> {
        tone_amp(hz, 16_000.0 / 32_768.0)
    }

    fn tone_amp(hz: f32, amplitude: f32) -> Vec<i16> {
        (0..WINDOW)
            .flat_map(|index| {
                let sample = ((std::f32::consts::TAU * hz * index as f32) / 44_100.0).sin();
                let value = (sample * amplitude * 32_768.0) as i16;
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
