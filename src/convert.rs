//! Turn decoded PCM into the signed 8-bit mono a `.mod` sample stores.
//!
//! The steps are pure: downmix, optional peak normalize, linear resample to a
//! target rate, quantize (with or without triangular dither), then truncate to
//! the ProTracker word-count limit and pad to an even length. Nothing here
//! touches the disk or the terminal.
//!
//! The usual target rate is the Amiga playback rate of a note. A sample
//! resampled to the C-2 rate plays at its original pitch when the pattern
//! hits C-2.

use crate::error::Error;
use crate::module::MAX_SAMPLE_BYTES;
use crate::notes::{period_at, C2_NOTE};
use crate::player::{tuned_period, PAL_CLOCK_HZ};
use crate::wav::DecodedWav;

/// Highest target rate [`import_pcm`] will resample to.
pub const MAX_TARGET_RATE: u32 = 384_000;

/// Seed used when the caller does not care which dither sequence it gets.
pub const DEFAULT_DITHER_SEED: u32 = 0x4F6D_6174;

/// How [`import_pcm`] turns [`DecodedWav`] into sample bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportOptions {
    /// Output rate of the 8-bit data, in hertz.
    pub target_rate: u32,
    /// Scale the kept audio so its peak is full scale.
    pub normalize: bool,
    /// Add triangular dither before rounding. Off rounds to the nearest code.
    pub dither: bool,
    /// Seed for the dither generator. Ignored when [`Self::dither`] is false.
    pub dither_seed: u32,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            target_rate: c2_rate(0),
            normalize: true,
            dither: false,
            dither_seed: DEFAULT_DITHER_SEED,
        }
    }
}

/// Signed 8-bit sample bytes plus what the conversion had to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSample {
    /// Even length, at most [`MAX_SAMPLE_BYTES`].
    pub data: Vec<u8>,
    /// Rate of the WAV that was read.
    pub source_rate: u32,
    /// Rate the bytes are meant to be played at.
    pub target_rate: u32,
    /// Frames in the source, after the channel count is accounted for.
    pub source_frames: usize,
    /// The resampled audio was longer than [`MAX_SAMPLE_BYTES`] and was cut.
    pub truncated: bool,
    /// A message to show when [`Self::truncated`] is set.
    pub warning: Option<String>,
}

/// Paula rate for `period` after `finetune_raw`, rounded to the nearest hertz.
///
/// This is `PAL_CLOCK / tuned_period`. Playing the imported bytes at that
/// period reproduces the source pitch.
pub fn amiga_rate(period: u16, finetune_raw: u8) -> Result<u32, Error> {
    let tuned = tuned_period(period, finetune_raw);
    if tuned == 0 {
        return Err(Error::SampleEdit(
            "period 0 has no Amiga playback rate".to_string(),
        ));
    }
    let period = u64::from(tuned);
    let clock = u64::from(PAL_CLOCK_HZ);
    let rate = (clock + period / 2) / period;
    u32::try_from(rate).map_err(|_| {
        Error::SampleEdit(format!(
            "Amiga rate for period {tuned} does not fit in a u32"
        ))
    })
}

/// Playback rate of C-2 at `finetune_raw`. Finetune 0 is 8287 Hz.
pub fn c2_rate(finetune_raw: u8) -> u32 {
    amiga_rate(period_at(C2_NOTE), finetune_raw).unwrap_or(8_287)
}

/// Rate so that `note` (an index into the finetune-0 period table) plays the
/// source at its original pitch, using `finetune_raw`.
pub fn rate_for_note(note: usize, finetune_raw: u8) -> u32 {
    amiga_rate(period_at(note), finetune_raw).unwrap_or(8_287)
}

/// Downmix, resample, and quantize `wav` into a module sample.
pub fn import_pcm(wav: &DecodedWav, options: &ImportOptions) -> Result<ImportedSample, Error> {
    if options.target_rate == 0 {
        return Err(Error::Wav(
            "target sample rate must be greater than zero".to_string(),
        ));
    }
    if options.target_rate > MAX_TARGET_RATE {
        return Err(Error::Wav(format!(
            "target sample rate {} Hz is above {MAX_TARGET_RATE} Hz",
            options.target_rate
        )));
    }
    if wav.sample_rate == 0 {
        return Err(Error::Wav("WAV sample rate is 0".to_string()));
    }
    let channels = usize::from(wav.channels);
    let mono = downmix_mono(&wav.interleaved, channels)?;
    let source_frames = mono.len();
    let projected = projected_len(mono.len(), wav.sample_rate, options.target_rate);
    let mut resampled = resample_linear(
        &mono,
        wav.sample_rate,
        options.target_rate,
        MAX_SAMPLE_BYTES,
    );
    if options.normalize {
        normalize_peak(&mut resampled);
    }
    let mut data = quantize_signed8(&resampled, options.dither, options.dither_seed);
    let truncated = projected > data.len();
    if data.len() % 2 == 1 {
        data.push(0);
    }
    let warning = truncated.then(|| {
        format!(
            "truncated from {projected} to {} bytes; a .mod sample holds at most {MAX_SAMPLE_BYTES} bytes",
            data.len()
        )
    });
    Ok(ImportedSample {
        data,
        source_rate: wav.sample_rate,
        target_rate: options.target_rate,
        source_frames,
        truncated,
        warning,
    })
}

/// Average interleaved frames down to mono.
pub fn downmix_mono(interleaved: &[f32], channels: usize) -> Result<Vec<f32>, Error> {
    if channels == 0 {
        return Err(Error::Wav("WAV has no channels".to_string()));
    }
    if channels > 64 {
        return Err(Error::Wav(format!(
            "WAV has {channels} channels; omatrack can downmix up to 64"
        )));
    }
    if channels == 1 {
        return Ok(interleaved.to_vec());
    }
    let frames = interleaved.len() / channels;
    let mut out = Vec::with_capacity(frames);
    let scale = channels as f32;
    for frame in 0..frames {
        let mut sum = 0.0f32;
        for channel in 0..channels {
            sum += interleaved[frame * channels + channel];
        }
        out.push(sum / scale);
    }
    Ok(out)
}

/// Linear resample. Stops after `limit` samples so a long file cannot allocate
/// past the ProTracker maximum.
pub fn resample_linear(
    input: &[f32],
    source_rate: u32,
    target_rate: u32,
    limit: usize,
) -> Vec<f32> {
    if input.is_empty() || source_rate == 0 || target_rate == 0 || limit == 0 {
        return Vec::new();
    }
    if source_rate == target_rate {
        let take = input.len().min(limit);
        return input[..take].to_vec();
    }
    let projected = projected_len(input.len(), source_rate, target_rate).min(limit);
    let step = f64::from(source_rate) / f64::from(target_rate);
    let mut out = Vec::with_capacity(projected);
    let last = input.len() - 1;
    for index in 0..projected {
        let pos = index as f64 * step;
        if pos >= input.len() as f64 {
            break;
        }
        let left = pos.floor() as usize;
        let frac = (pos - left as f64) as f32;
        let s0 = input[left.min(last)];
        let s1 = input[left.saturating_add(1).min(last)];
        out.push(s0 + (s1 - s0) * frac);
    }
    out
}

/// Scale `samples` so the loudest absolute peak is `1.0`. Silence is left alone.
pub fn normalize_peak(samples: &mut [f32]) {
    let mut peak = 0.0f32;
    for sample in samples.iter() {
        let abs = sample.abs();
        if abs > peak {
            peak = abs;
        }
    }
    if peak <= f32::EPSILON {
        return;
    }
    let gain = 1.0 / peak;
    for sample in samples.iter_mut() {
        *sample *= gain;
    }
}

/// Round `samples` in `-1.0..=1.0` to signed 8-bit codes.
///
/// Full scale maps with a factor of 128 and is clamped to `-128..=127`, which
/// is the integer range of a `.mod` byte. When `dither` is set, one LSB of
/// triangular noise (two uniform values, subtracted) is added before rounding.
pub fn quantize_signed8(samples: &[f32], dither: bool, seed: u32) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(samples.len());
    for sample in samples {
        let mut scaled = sample.clamp(-1.0, 1.0) * 128.0;
        if dither {
            scaled += rng.tpdf();
        }
        let rounded = scaled.round().clamp(-128.0, 127.0);
        let code = i8::try_from(rounded as i32).unwrap_or(0);
        out.push(code as u8);
    }
    out
}

fn projected_len(input_len: usize, source_rate: u32, target_rate: u32) -> usize {
    if input_len == 0 || source_rate == 0 {
        return 0;
    }
    let num = (input_len as u128).saturating_mul(u128::from(target_rate));
    let den = u128::from(source_rate);
    usize::try_from(num / den).unwrap_or(usize::MAX)
}

struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Self(seed | 1)
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    fn unit(&mut self) -> f32 {
        let bits = self.next_u32() >> 8;
        bits as f32 / 16_777_216.0
    }

    /// Triangular noise in `(-1, 1)`, one LSB wide once the signal is scaled.
    fn tpdf(&mut self) -> f32 {
        self.unit() - self.unit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav::{decode_wav, encode_mono8_wav, DecodedWav};

    fn mono(rate: u32, samples: &[f32]) -> DecodedWav {
        DecodedWav {
            sample_rate: rate,
            channels: 1,
            interleaved: samples.to_vec(),
        }
    }

    #[test]
    fn c2_finetune_zero_is_the_amiga_rate() {
        let rate = c2_rate(0);
        assert_eq!(rate, 8_287);
        assert_eq!(amiga_rate(856, 0).unwrap(), 4_144);
        assert_ne!(c2_rate(0x0F), rate);
        assert!(amiga_rate(0, 0).is_err());
    }

    #[test]
    fn stereo_downmix_averages_the_channels() {
        let mixed = downmix_mono(&[1.0, -1.0, 0.5, 0.5, 0.0, 1.0], 2).unwrap();
        assert_eq!(mixed, vec![0.0, 0.5, 0.5]);
    }

    #[test]
    fn linear_resample_doubles_the_length() {
        let input = [0.0, 1.0, 0.0, -1.0];
        let out = resample_linear(&input, 1_000, 2_000, 100);
        assert_eq!(out.len(), 8);
        assert!((out[0] - 0.0).abs() < 1e-6);
        assert!((out[1] - 0.5).abs() < 1e-6);
        assert!((out[2] - 1.0).abs() < 1e-6);
        assert!((out[3] - 0.5).abs() < 1e-6);
        assert!((out[4] - 0.0).abs() < 1e-6);
        assert!((out[5] + 0.5).abs() < 1e-6);
        assert!((out[6] + 1.0).abs() < 1e-6);
        assert!((out[7] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn normalize_lifts_a_quiet_peak_to_full_scale() {
        let wav = mono(8_000, &[0.25, 0.5, 0.125, 0.0]);
        let options = ImportOptions {
            target_rate: 8_000,
            normalize: true,
            dither: false,
            dither_seed: 1,
        };
        let imported = import_pcm(&wav, &options).unwrap();
        let peak = imported
            .data
            .iter()
            .map(|byte| (*byte as i8).unsigned_abs())
            .max()
            .unwrap();
        assert_eq!(peak, 127);
        assert!(!imported.truncated);
        assert!(imported.warning.is_none());
    }

    #[test]
    fn clean_quantization_matches_known_codes() {
        let samples = [0.0, 0.5, -0.5, 1.0, -1.0, 64.0 / 128.0];
        let codes = quantize_signed8(&samples, false, 1);
        assert_eq!(codes[0] as i8, 0);
        assert_eq!(codes[1] as i8, 64);
        assert_eq!(codes[2] as i8, -64);
        assert_eq!(codes[3] as i8, 127);
        assert_eq!(codes[4] as i8, -128);
        assert_eq!(codes[5] as i8, 64);
    }

    #[test]
    fn dither_is_deterministic_and_not_identical_to_rounding() {
        let samples: Vec<f32> = (0..64).map(|i| (i as f32 - 32.0) / 10_000.0).collect();
        let clean = quantize_signed8(&samples, false, 1);
        let once = quantize_signed8(&samples, true, 0x1234);
        let twice = quantize_signed8(&samples, true, 0x1234);
        let other = quantize_signed8(&samples, true, 0x99);
        assert_eq!(once, twice);
        assert_ne!(once, clean);
        assert_ne!(once, other);
    }

    #[test]
    fn odd_lengths_are_padded_and_long_audio_is_truncated() {
        let wav = mono(1_000, &[0.2, -0.4, 0.6]);
        let options = ImportOptions {
            target_rate: 1_000,
            normalize: false,
            dither: false,
            dither_seed: 1,
        };
        let imported = import_pcm(&wav, &options).unwrap();
        assert_eq!(imported.data.len(), 4);
        assert_eq!(imported.data[3], 0);

        let long = vec![0.5f32; MAX_SAMPLE_BYTES + 50];
        let wav = mono(8_000, &long);
        let imported = import_pcm(
            &wav,
            &ImportOptions {
                target_rate: 8_000,
                ..options
            },
        )
        .unwrap();
        assert!(imported.truncated);
        assert_eq!(imported.data.len(), MAX_SAMPLE_BYTES);
        let warning = imported.warning.expect("warning");
        assert!(
            warning.contains("131070") || warning.contains(&MAX_SAMPLE_BYTES.to_string()),
            "{warning}"
        );
        assert!(warning.contains("truncat"), "{warning}");
    }

    #[test]
    fn eight_bit_export_round_trips_through_import() {
        let signed = [0u8, 1, 64, 127, 128, 255, 200];
        let exported = encode_mono8_wav(8_000, &signed).unwrap();
        let decoded = decode_wav(&exported).unwrap();
        let imported = import_pcm(
            &decoded,
            &ImportOptions {
                target_rate: 8_000,
                normalize: false,
                dither: false,
                dither_seed: 1,
            },
        )
        .unwrap();
        assert_eq!(&imported.data[..signed.len()], &signed);
        assert_eq!(imported.data.len() % 2, 0);
        assert_eq!(*imported.data.last().unwrap(), 0);
    }

    #[test]
    fn resampling_to_c2_shortens_a_cd_rate_buffer() {
        let wav = mono(44_100, &vec![0.25; 44_100]);
        let rate = c2_rate(0);
        let imported = import_pcm(
            &wav,
            &ImportOptions {
                target_rate: rate,
                normalize: false,
                dither: false,
                dither_seed: 1,
            },
        )
        .unwrap();
        assert_eq!(imported.target_rate, 8_287);
        assert_eq!(imported.data.len(), 8_288);
        assert!(!imported.truncated);
    }

    #[test]
    fn same_rate_import_then_export_matches_the_quantized_bytes() {
        let mut pcm = Vec::new();
        for index in 0..32 {
            let sample = ((index * 2000) as i16).wrapping_sub(16_000);
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
        let wav_bytes = test_wav(1, 16_000, 16, &pcm);
        let decoded = decode_wav(&wav_bytes).unwrap();
        let options = ImportOptions {
            target_rate: 16_000,
            normalize: false,
            dither: false,
            dither_seed: 1,
        };
        let imported = import_pcm(&decoded, &options).unwrap();
        let exported = encode_mono8_wav(imported.target_rate, &imported.data).unwrap();
        let again = decode_wav(&exported).unwrap();
        let round = import_pcm(
            &again,
            &ImportOptions {
                normalize: false,
                ..options
            },
        )
        .unwrap();
        assert_eq!(round.data, imported.data);
        assert_eq!(round.target_rate, 16_000);
    }

    #[test]
    fn rejects_a_zero_target_rate() {
        let wav = mono(8_000, &[0.0, 0.1]);
        let options = ImportOptions {
            target_rate: 0,
            normalize: false,
            dither: false,
            dither_seed: 1,
        };
        let err = import_pcm(&wav, &options).unwrap_err();
        assert!(err.to_string().contains("greater than zero"), "{err}");
    }

    fn test_wav(channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let block = channels * (bits / 8);
        let byte_rate = rate * u32::from(block);
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&block.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        let mut body = Vec::new();
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        body.extend_from_slice(&fmt);
        body.extend_from_slice(b"data");
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(data);
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&body);
        out
    }
}
