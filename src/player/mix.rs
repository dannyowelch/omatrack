//! Mix four Amiga channels into interleaved stereo frames.
//!
//! Channels 0 and 3 are left, 1 and 2 are right. `stereo_separation` of 100
//! is hard panning; 0 sends every channel to both ears at full level. A
//! sample byte times a 0–64 volume fits four channels into an `i16` without
//! a further scale (`127 * 64 * 4 = 32512`).

use super::tables::{sample_step, FP_SHIFT};
use super::{Interpolation, Voice};
use crate::module::{Module, Sample, CHANNELS};

pub(super) fn mix_frames(
    module: &Module,
    voices: &mut [Voice; CHANNELS],
    output_rate: u32,
    interpolation: Interpolation,
    separation: u8,
    output: &mut [i16],
) -> [u16; CHANNELS] {
    let separation = separation.min(100);
    let frames = output.len() / 2;
    let mut peaks = [0u16; CHANNELS];
    let limit = u32::from(super::CHANNEL_PEAK_SCALE);
    for frame in 0..frames {
        let mut left = 0i32;
        let mut right = 0i32;
        for (index, voice) in voices.iter_mut().enumerate() {
            let (sample, step) = voice_sample(module, voice, output_rate);
            let Some(sample) = sample else {
                continue;
            };
            let region = region_of(sample);
            if step == 0 || !voice.active {
                continue;
            }
            let value = read_sample(
                sample.data.as_slice(),
                voice.position,
                &region,
                interpolation,
            );
            let volume = i32::from(voice.audible_volume);
            if !voice.muted && volume != 0 && value != 0 {
                let amp = value * volume;
                let mag = amp.unsigned_abs().min(limit);
                if mag > u32::from(peaks[index]) {
                    peaks[index] = mag as u16;
                }
                let (gain_l, gain_r) = gains(index, separation);
                left += amp * gain_l / 100;
                right += amp * gain_r / 100;
            }
            voice.active = advance(&mut voice.position, step, &region);
        }
        let base = frame * 2;
        output[base] = clamp_i16(left);
        output[base + 1] = clamp_i16(right);
    }
    peaks
}

fn voice_sample<'a>(
    module: &'a Module,
    voice: &Voice,
    output_rate: u32,
) -> (Option<&'a Sample>, u64) {
    if voice.sample == 0 || voice.audible_period == 0 {
        return (None, 0);
    }
    let Some(index) = usize::from(voice.sample).checked_sub(1) else {
        return (None, 0);
    };
    let step = sample_step(voice.audible_period, output_rate);
    (module.samples.get(index), step)
}

struct Region {
    start: usize,
    end: usize,
    length: usize,
    enabled: bool,
}

fn region_of(sample: &Sample) -> Region {
    let length = sample.data.len();
    if !sample.loops() {
        return Region {
            start: 0,
            end: length,
            length,
            enabled: false,
        };
    }
    let start = usize::from(sample.loop_start) * 2;
    let mut end = start.saturating_add(usize::from(sample.loop_length) * 2);
    if start >= length || end <= start {
        return Region {
            start: 0,
            end: length,
            length,
            enabled: false,
        };
    }
    if end > length {
        end = length;
    }
    if end - start < 2 {
        return Region {
            start: 0,
            end: length,
            length,
            enabled: false,
        };
    }
    Region {
        start,
        end,
        length,
        enabled: true,
    }
}

fn read_sample(data: &[u8], position: u64, region: &Region, interpolation: Interpolation) -> i32 {
    let index = usize::try_from(position >> FP_SHIFT).unwrap_or(usize::MAX);
    let frac = i32::try_from(position & ((1 << FP_SHIFT) - 1)).unwrap_or(0);
    let Some(first) = map_index(index, region) else {
        return 0;
    };
    let s0 = i32::from(data[first] as i8);
    if interpolation == Interpolation::Nearest || frac == 0 {
        return s0;
    }
    let Some(second) = map_index(index.saturating_add(1), region) else {
        return s0;
    };
    let s1 = i32::from(data[second] as i8);
    s0 + (s1 - s0) * frac / (1 << FP_SHIFT)
}

fn map_index(index: usize, region: &Region) -> Option<usize> {
    if region.length == 0 {
        return None;
    }
    if region.enabled {
        if index < region.end {
            return (index < region.length).then_some(index);
        }
        let span = region.end - region.start;
        if span == 0 {
            return None;
        }
        Some(region.start + (index - region.start) % span)
    } else if index < region.length {
        Some(index)
    } else {
        None
    }
}

/// Returns whether the voice is still inside a one-shot sample.
fn advance(position: &mut u64, step: u64, region: &Region) -> bool {
    *position = position.saturating_add(step);
    if region.enabled {
        let end = (region.end as u64) << FP_SHIFT;
        let start = (region.start as u64) << FP_SHIFT;
        let span = end.saturating_sub(start);
        if *position >= end && span > 0 {
            let over = *position - end;
            *position = start + (over % span);
        }
        true
    } else {
        let end = (region.length as u64) << FP_SHIFT;
        if *position >= end {
            *position = end;
            false
        } else {
            true
        }
    }
}

fn gains(channel: usize, separation: u8) -> (i32, i32) {
    let bleed = i32::from(100 - separation);
    if channel == 0 || channel == 3 {
        (100, bleed)
    } else {
        (bleed, 100)
    }
}

fn clamp_i16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}
