//! ProTracker note names and effect labels.
//!
//! Cells store an Amiga period, not a note number. The table below is finetune
//! 0, which is the period written into the pattern. The replayer in
//! [`crate::player`] keeps its own voice state and applies each sample's
//! finetune on top of these base periods.

/// Finetune-0 periods for C-1 through B-3, the range ProTracker edits.
pub const PERIODS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, // C-1 .. B-1
    428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, // C-2 .. B-2
    214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, // C-3 .. B-3
];

const NOTE_NAMES: [&str; 12] = [
    "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-",
];

/// Render a period as a 3-character tracker cell.
///
/// Known finetune-0 periods become `C-1` style names (ProTracker octave
/// numbering). Period 0 is `---`. Anything else is the decimal period when it
/// fits in 3 digits, or `***` when it does not. The status line shows the raw
/// period either way.
pub fn format_period(period: u16) -> String {
    if period == 0 {
        return "---".to_string();
    }
    if let Some(index) = PERIODS.iter().position(|&p| p == period) {
        let octave = index / 12 + 1;
        return format!("{}{octave}", NOTE_NAMES[index % 12]);
    }
    if period < 1000 {
        format!("{period:03}")
    } else {
        "***".to_string()
    }
}

/// Signed finetune from the low nibble. `0..=7` is `0..=+7`, `8..=15` is `-8..=-1`.
pub fn finetune_from_byte(raw: u8) -> i8 {
    let nibble = i8::try_from(raw & 0x0F).unwrap_or(0);
    if nibble >= 8 {
        nibble - 16
    } else {
        nibble
    }
}

/// Format a finetune byte as `+0`, `+7`, or `-8`. High bits are ignored here;
/// the raw byte is what the writer stores.
pub fn format_finetune(raw: u8) -> String {
    format!("{:+}", finetune_from_byte(raw))
}

/// Short name for a ProTracker effect. `Fxx` splits speed and tempo at 32.
pub fn effect_description(effect: u8, param: u8) -> &'static str {
    match effect {
        0x0 => "arpeggio",
        0x1 => "slide up",
        0x2 => "slide down",
        0x3 => "tone portamento",
        0x4 => "vibrato",
        0x5 => "tone portamento + vol slide",
        0x6 => "vibrato + vol slide",
        0x7 => "tremolo",
        0x8 => "unused",
        0x9 => "sample offset",
        0xA => "volume slide",
        0xB => "position jump",
        0xC => "set volume",
        0xD => "pattern break",
        0xE => extended_effect_name(param),
        0xF if param < 0x20 => "set speed",
        0xF => "set tempo",
        _ => "effect",
    }
}

fn extended_effect_name(param: u8) -> &'static str {
    match param >> 4 {
        0x0 => "set filter",
        0x1 => "fine slide up",
        0x2 => "fine slide down",
        0x3 => "glissando control",
        0x4 => "vibrato waveform",
        0x5 => "set finetune",
        0x6 => "pattern loop",
        0x7 => "tremolo waveform",
        0x8 => "unused",
        0x9 => "retrigger",
        0xA => "fine volume up",
        0xB => "fine volume down",
        0xC => "note cut",
        0xD => "note delay",
        0xE => "pattern delay",
        0xF => "invert loop",
        _ => "extended",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_use_protracker_octave_numbers() {
        assert_eq!(format_period(0), "---");
        assert_eq!(format_period(856), "C-1");
        assert_eq!(format_period(428), "C-2");
        assert_eq!(format_period(214), "C-3");
        assert_eq!(format_period(113), "B-3");
        assert_eq!(format_period(453), "B-1");
        assert_eq!(format_period(999), "999");
        assert_eq!(format_period(1000), "***");
        assert_eq!(format_period(4095), "***");
    }

    #[test]
    fn finetune_nibble_is_signed() {
        assert_eq!(finetune_from_byte(0x00), 0);
        assert_eq!(finetune_from_byte(0x07), 7);
        assert_eq!(finetune_from_byte(0x08), -8);
        assert_eq!(finetune_from_byte(0x0F), -1);
        // High bits are preserved by the file but do not change the nibble.
        assert_eq!(finetune_from_byte(0x1F), -1);
        assert_eq!(format_finetune(0x08), "-8");
        assert_eq!(format_finetune(0x03), "+3");
    }

    #[test]
    fn effect_names_cover_the_protracker_set() {
        assert_eq!(effect_description(0x0, 0x00), "arpeggio");
        assert_eq!(effect_description(0xC, 0x40), "set volume");
        assert_eq!(effect_description(0xE, 0xC1), "note cut");
        assert_eq!(effect_description(0xE, 0xD0), "note delay");
        assert_eq!(effect_description(0xF, 0x06), "set speed");
        assert_eq!(effect_description(0xF, 0x20), "set tempo");
        assert_eq!(effect_description(0xF, 0x7D), "set tempo");
    }
}
