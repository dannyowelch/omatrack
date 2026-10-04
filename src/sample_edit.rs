//! Edits on one [`Sample`](crate::Sample).
//!
//! Each function changes the sample in place and returns [`Error::SampleEdit`]
//! when the request cannot be stored (a loop past the end, an odd trim, a
//! volume above 64). Callers that want undo snapshot the sample first; these
//! functions do not keep history themselves.

use crate::error::Error;
use crate::module::{Module, Sample, SAMPLE_COUNT};

/// Set the sample volume. ProTracker plays `0..=64`.
pub fn set_volume(sample: &mut Sample, volume: u8) -> Result<(), Error> {
    if volume > 64 {
        return Err(Error::SampleEdit(format!(
            "volume {volume} is outside 0..=64"
        )));
    }
    sample.volume = volume;
    Ok(())
}

/// Set the signed finetune. The low nibble is replaced; high bits stay.
pub fn set_finetune(sample: &mut Sample, finetune: i8) -> Result<(), Error> {
    if !(-8..=7).contains(&finetune) {
        return Err(Error::SampleEdit(format!(
            "finetune {finetune} is outside -8..=7"
        )));
    }
    let nibble = if finetune < 0 {
        u8::try_from(finetune + 16).unwrap_or(0)
    } else {
        u8::try_from(finetune).unwrap_or(0)
    };
    sample.finetune_raw = (sample.finetune_raw & 0xF0) | nibble;
    Ok(())
}

/// Set the loop in bytes. Both ends must be even, because the file stores words.
///
/// A `length` of 0 or 2 disables the loop (a word length of 0 or 1 does not
/// repeat). A real loop is at least 4 bytes and must lie inside the sample.
pub fn set_loop_bytes(sample: &mut Sample, start: usize, length: usize) -> Result<(), Error> {
    if start % 2 != 0 || length % 2 != 0 {
        return Err(Error::SampleEdit(format!(
            "loop start {start} and length {length} must be even byte offsets"
        )));
    }
    if start > sample.data.len() {
        return Err(Error::SampleEdit(format!(
            "loop start {start} is past the sample ({} bytes)",
            sample.data.len()
        )));
    }
    if length <= 2 {
        sample.loop_start = fit_word(start / 2, "loop start")?;
        sample.loop_length = 1;
        return Ok(());
    }
    let end = start.saturating_add(length);
    if end > sample.data.len() {
        return Err(Error::SampleEdit(format!(
            "loop {start}+{length} extends past the sample ({} bytes)",
            sample.data.len()
        )));
    }
    sample.loop_start = fit_word(start / 2, "loop start")?;
    sample.loop_length = fit_word(length / 2, "loop length")?;
    Ok(())
}

/// Turn the loop off, or loop the whole sample if it is long enough.
pub fn toggle_loop(sample: &mut Sample) -> Result<(), Error> {
    if sample.loops() {
        sample.loop_length = 1;
        return Ok(());
    }
    if sample.data.len() < 4 {
        return Err(Error::SampleEdit(
            "sample is too short to loop (need at least 4 bytes)".to_string(),
        ));
    }
    sample.loop_start = 0;
    sample.loop_length = fit_word(sample.data.len() / 2, "loop length")?;
    Ok(())
}

/// Keep `start..end` (end exclusive). Both bounds must be even.
///
/// A loop that sat entirely inside the range is shifted with the data. Any
/// other loop is turned off, because its points no longer describe this audio.
pub fn trim(sample: &mut Sample, start: usize, end: usize) -> Result<(), Error> {
    if start > end || end > sample.data.len() {
        return Err(Error::SampleEdit(format!(
            "trim {start}..{end} is outside the sample ({} bytes)",
            sample.data.len()
        )));
    }
    if start % 2 != 0 || end % 2 != 0 {
        return Err(Error::SampleEdit(
            "trim points must be even byte offsets".to_string(),
        ));
    }
    if start == end {
        return Err(Error::SampleEdit("trim range is empty".to_string()));
    }
    if sample.loops() {
        let loop_start = usize::from(sample.loop_start) * 2;
        let loop_end = loop_start.saturating_add(usize::from(sample.loop_length) * 2);
        if loop_start >= start && loop_end <= end && loop_end - loop_start >= 4 {
            sample.loop_start = fit_word((loop_start - start) / 2, "loop start")?;
        } else {
            sample.loop_start = 0;
            sample.loop_length = 1;
        }
    }
    sample.data = sample.data[start..end].to_vec();
    Ok(())
}

/// Scale the 8-bit data so the loudest byte is full scale. Silence is unchanged.
pub fn normalize(sample: &mut Sample) {
    let peak = sample
        .data
        .iter()
        .map(|byte| i32::from((*byte as i8).unsigned_abs()))
        .max()
        .unwrap_or(0);
    if peak == 0 || peak >= 127 {
        return;
    }
    for byte in &mut sample.data {
        let value = i32::from(*byte as i8) * 127 / peak;
        *byte = value.clamp(-128, 127) as i8 as u8;
    }
}

/// Reverse the PCM. A loop stays over the same audio, which is now at the other end.
pub fn reverse(sample: &mut Sample) {
    sample.data.reverse();
    if !sample.loops() {
        return;
    }
    let total = sample.data.len() / 2;
    let start = usize::from(sample.loop_start);
    let len = usize::from(sample.loop_length);
    let end = start.saturating_add(len);
    if len > 1 && end <= total {
        let new_start = total - end;
        sample.loop_start = u16::try_from(new_start).unwrap_or(0);
    }
}

/// Fade from silence to the original level.
pub fn fade_in(sample: &mut Sample) {
    fade(sample, true);
}

/// Fade from the original level to silence.
pub fn fade_out(sample: &mut Sample) {
    fade(sample, false);
}

/// Drop the PCM and turn the loop off. Name, volume, and finetune stay.
pub fn clear_data(sample: &mut Sample) {
    sample.data.clear();
    sample.loop_start = 0;
    sample.loop_length = 1;
}

/// Copy instrument `from` onto instrument `to`. Indexes are 0-based slots.
pub fn copy_to(module: &mut Module, from: usize, to: usize) -> Result<(), Error> {
    if from >= SAMPLE_COUNT || to >= SAMPLE_COUNT {
        return Err(Error::SampleEdit(format!(
            "sample slot is outside 1..={SAMPLE_COUNT}"
        )));
    }
    if from != to {
        module.samples[to] = module.samples[from].clone();
    }
    Ok(())
}

fn fade(sample: &mut Sample, fade_in: bool) {
    let len = sample.data.len();
    if len <= 1 {
        return;
    }
    let last = i32::try_from(len - 1).unwrap_or(1);
    for (index, byte) in sample.data.iter_mut().enumerate() {
        let value = i32::from(*byte as i8);
        let gain = i32::try_from(index).unwrap_or(0);
        let gain = if fade_in { gain } else { last - gain };
        let scaled = value * gain / last;
        *byte = scaled.clamp(-128, 127) as i8 as u8;
    }
}

fn fit_word(words: usize, what: &str) -> Result<u16, Error> {
    u16::try_from(words)
        .map_err(|_| Error::SampleEdit(format!("{what} does not fit in a 16-bit word count")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::Module;

    fn tone() -> Sample {
        let mut sample = Sample {
            volume: 64,
            ..Sample::default()
        };
        sample
            .set_data(vec![10, 20, 30, 40, 50, 60, 70, 80])
            .unwrap();
        sample
    }

    #[test]
    fn volume_and_finetune_keep_their_ranges() {
        let mut sample = tone();
        set_volume(&mut sample, 32).unwrap();
        assert_eq!(sample.volume, 32);
        let err = set_volume(&mut sample, 65).unwrap_err();
        assert!(err.to_string().contains("0..=64"), "{err}");
        assert_eq!(sample.volume, 32);

        sample.finetune_raw = 0xA0;
        set_finetune(&mut sample, -2).unwrap();
        assert_eq!(sample.finetune(), -2);
        assert_eq!(sample.finetune_raw, 0xAE);
        let err = set_finetune(&mut sample, 8).unwrap_err();
        assert!(err.to_string().contains("-8..=7"), "{err}");
    }

    #[test]
    fn loop_points_toggle_and_reject_a_range_past_the_end() {
        let mut sample = tone();
        set_loop_bytes(&mut sample, 2, 4).unwrap();
        assert!(sample.loops());
        assert_eq!(sample.loop_start, 1);
        assert_eq!(sample.loop_length, 2);

        toggle_loop(&mut sample).unwrap();
        assert!(!sample.loops());
        toggle_loop(&mut sample).unwrap();
        assert_eq!(sample.loop_start, 0);
        assert_eq!(sample.loop_length, 4);

        let err = set_loop_bytes(&mut sample, 2, 8).unwrap_err();
        assert!(err.to_string().contains("extends past"), "{err}");
        let err = set_loop_bytes(&mut sample, 1, 4).unwrap_err();
        assert!(err.to_string().contains("even"), "{err}");

        set_loop_bytes(&mut sample, 0, 0).unwrap();
        assert!(!sample.loops());

        let mut tiny = Sample::default();
        tiny.set_data(vec![1, 2]).unwrap();
        let err = toggle_loop(&mut tiny).unwrap_err();
        assert!(err.to_string().contains("too short"), "{err}");
    }

    #[test]
    fn trim_shifts_an_interior_loop_and_drops_one_that_hangs_off() {
        let mut sample = tone();
        set_loop_bytes(&mut sample, 2, 4).unwrap();
        trim(&mut sample, 2, 6).unwrap();
        assert_eq!(sample.data, vec![30, 40, 50, 60]);
        assert_eq!(sample.loop_start, 0);
        assert_eq!(sample.loop_length, 2);

        let mut sample = tone();
        set_loop_bytes(&mut sample, 0, 4).unwrap();
        trim(&mut sample, 2, 8).unwrap();
        assert!(!sample.loops());
        assert_eq!(sample.data, vec![30, 40, 50, 60, 70, 80]);

        let mut sample = tone();
        let err = trim(&mut sample, 1, 4).unwrap_err();
        assert!(err.to_string().contains("even"), "{err}");
        let err = trim(&mut sample, 0, 0).unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn normalize_reverse_fade_and_clear() {
        let mut sample = tone();
        normalize(&mut sample);
        let peak = sample
            .data
            .iter()
            .map(|byte| (*byte as i8) as i32)
            .max()
            .unwrap();
        assert_eq!(peak, 127);
        assert_eq!(sample.data[0] as i8, 15);

        let mut sample = tone();
        set_loop_bytes(&mut sample, 0, 4).unwrap();
        reverse(&mut sample);
        assert_eq!(sample.data, vec![80, 70, 60, 50, 40, 30, 20, 10]);
        assert_eq!(sample.loop_start, 2);
        assert_eq!(sample.loop_length, 2);

        let mut sample = tone();
        fade_in(&mut sample);
        assert_eq!(sample.data[0], 0);
        assert_eq!(sample.data[7], 80);
        fade_out(&mut sample);
        assert_eq!(sample.data[7], 0);

        clear_data(&mut sample);
        assert!(sample.data.is_empty());
        assert!(!sample.loops());
        assert_eq!(sample.volume, 64);
    }

    #[test]
    fn copy_replaces_the_destination_slot() {
        let mut module = Module::default();
        module.samples[0] = tone();
        module.samples[0].set_name("kick").unwrap();
        module.samples[1].volume = 10;
        copy_to(&mut module, 0, 1).unwrap();
        assert_eq!(module.samples[1].display_name(), "kick");
        assert_eq!(module.samples[1].data, module.samples[0].data);
        copy_to(&mut module, 1, 1).unwrap();
        let err = copy_to(&mut module, 0, 40).unwrap_err();
        assert!(err.to_string().contains("slot"), "{err}");
    }
}
