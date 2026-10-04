//! In-place radix-2 FFT. The tracker only needs a few hundred bins, so this
//! stays in the crate instead of pulling in a transform library.

use std::f32::consts::TAU;

/// Multiply `samples` by a periodic Hann window.
pub fn apply_hann(samples: &mut [f32]) {
    let n = samples.len();
    if n < 2 {
        return;
    }
    let scale = TAU / n as f32;
    for (index, sample) in samples.iter_mut().enumerate() {
        let window = 0.5 - 0.5 * (scale * index as f32).cos();
        *sample *= window;
    }
}

/// Single-sided magnitudes of `input`.
///
/// `input.len()` must be a power of two and at least 2. The result has
/// `len / 2` bins (DC through the bin before Nyquist). A full-scale sine
/// lands near 1.0 in its bin. A bad length returns an empty vector.
pub fn magnitudes(input: &[f32]) -> Vec<f32> {
    let n = input.len();
    if n < 2 || !n.is_power_of_two() {
        return Vec::new();
    }
    let mut real = input.to_vec();
    let mut imag = vec![0.0f32; n];
    transform(&mut real, &mut imag);
    let scale = 2.0 / n as f32;
    let mut out = Vec::with_capacity(n / 2);
    for bin in 0..n / 2 {
        let power = real[bin] * real[bin] + imag[bin] * imag[bin];
        out.push(power.sqrt() * scale);
    }
    out
}

fn transform(real: &mut [f32], imag: &mut [f32]) {
    let n = real.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            real.swap(i, j);
            imag.swap(i, j);
        }
    }
    let mut len = 2usize;
    while len <= n {
        let angle = -TAU / len as f32;
        let step_re = angle.cos();
        let step_im = angle.sin();
        let mut start = 0usize;
        while start < n {
            let mut twiddle_re = 1.0f32;
            let mut twiddle_im = 0.0f32;
            for offset in 0..len / 2 {
                let even = start + offset;
                let odd = even + len / 2;
                let prod_re = twiddle_re * real[odd] - twiddle_im * imag[odd];
                let prod_im = twiddle_re * imag[odd] + twiddle_im * real[odd];
                real[odd] = real[even] - prod_re;
                imag[odd] = imag[even] - prod_im;
                real[even] += prod_re;
                imag[even] += prod_im;
                let next_re = twiddle_re * step_re - twiddle_im * step_im;
                twiddle_im = twiddle_re * step_im + twiddle_im * step_re;
                twiddle_re = next_re;
            }
            start += len;
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, cycles: usize, amplitude: f32) -> Vec<f32> {
        (0..n)
            .map(|index| (TAU * cycles as f32 * index as f32 / n as f32).sin() * amplitude)
            .collect()
    }

    #[test]
    fn a_sine_peaks_in_its_own_bin() {
        let n = 64;
        let cycles = 5usize;
        let mags = magnitudes(&sine(n, cycles, 1.0));
        assert_eq!(mags.len(), n / 2);
        let peak = mags
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .map(|(index, _)| index)
            .unwrap();
        assert_eq!(peak, cycles);
        assert!(mags[cycles] > 0.9, "{}", mags[cycles]);
        assert!(mags[cycles] > mags[cycles - 1] * 5.0);
        assert!(mags[cycles] > mags[cycles + 1] * 5.0);
        assert!(mags[0] < 0.05, "dc {}", mags[0]);
    }

    #[test]
    fn hann_keeps_the_same_peak_bin() {
        let n = 128;
        let mut tone = sine(n, 13, 1.0);
        apply_hann(&mut tone);
        let mags = magnitudes(&tone);
        let peak = mags
            .iter()
            .enumerate()
            .skip(1)
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .map(|(index, _)| index)
            .unwrap();
        assert_eq!(peak, 13);
    }

    #[test]
    fn a_bad_length_is_empty_rather_than_a_panic() {
        assert!(magnitudes(&[]).is_empty());
        assert!(magnitudes(&[0.0, 1.0, 0.0]).is_empty());
    }
}
