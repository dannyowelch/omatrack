//! XM period tables.
//!
//! Linear periods follow the FastTracker 2 formula: C-4 (note 48, counting
//! C-0 as 0) with finetune 0 is period 4608 and 8363 Hz. Amiga periods use
//! the 8-finetune table from the XM 2.04 specification, scaled so that same
//! note is 8363 Hz (`8363 * 1712 / period`).

/// One semitone in linear-period units.
pub const SEMITONE: i32 = 64;

/// C-4 linear period at finetune 0.
const LINEAR_C4: f64 = 4608.0;

/// XM Amiga finetune table. Eight columns per semitone, B then C then C# …
///
/// The values are the ones published with the XM 2.04 format note. Index 0 is
/// the B below C, which matches 1-based note numbers (`note % 12 == 1` is C).
const AMIGA_TAB: [u16; 96] = [
    907, 900, 894, 887, 881, 875, 868, 862, 856, 850, 844, 838, 832, 826, 820, 814, 808, 802, 796,
    791, 785, 779, 774, 768, 762, 757, 752, 746, 741, 736, 730, 725, 720, 715, 709, 704, 699, 694,
    689, 684, 678, 675, 670, 665, 660, 655, 651, 646, 640, 636, 632, 628, 623, 619, 614, 610, 604,
    601, 597, 592, 588, 584, 580, 575, 570, 567, 563, 559, 555, 551, 547, 543, 538, 535, 532, 528,
    524, 520, 516, 513, 508, 505, 502, 498, 494, 491, 487, 484, 480, 477, 474, 470, 467, 463, 460,
    457,
];

/// Linear period for a 0-based note (`0` = C-0) and an XM finetune.
pub fn linear_period(note: i32, finetune: i32) -> i32 {
    10 * 12 * 16 * 4 - note * 16 * 4 - finetune / 2
}

/// Hertz for a linear period.
pub fn linear_hz(period: i32) -> f64 {
    if period <= 0 {
        return 0.0;
    }
    8363.0 * 2f64.powf((LINEAR_C4 - f64::from(period)) / 768.0)
}

/// Amiga period for a 1-based note (`1` = C-0) and an XM finetune `-128..=127`.
pub fn amiga_period(note: i32, finetune: i32) -> i32 {
    let mut note = note.clamp(1, 118);
    let mut steps = f64::from(finetune.clamp(-128, 127)) / 16.0;
    while steps < 0.0 && note > 1 {
        steps += 8.0;
        note -= 1;
    }
    while steps >= 8.0 && note < 118 {
        steps -= 8.0;
        note += 1;
    }
    steps = steps.clamp(0.0, 7.999);
    let semi = (note.rem_euclid(12)) as usize;
    let base = semi * 8;
    let idx = (steps.floor() as usize).min(7);
    let frac = steps - idx as f64;
    let a = f64::from(AMIGA_TAB[base + idx]);
    let b = if idx >= 7 {
        let next = (note + 1).clamp(1, 118);
        let next_semi = (next.rem_euclid(12)) as usize;
        f64::from(AMIGA_TAB[next_semi * 8])
    } else {
        f64::from(AMIGA_TAB[base + idx + 1])
    };
    let lerped = a * (1.0 - frac) + b * frac;
    let octave = note.div_euclid(12).clamp(0, 16);
    let denom = f64::from(1_i32 << octave);
    (lerped * 32.0 / denom).round().max(1.0) as i32
}

/// Hertz for an Amiga period from [`amiga_period`].
pub fn amiga_hz(period: i32) -> f64 {
    if period <= 0 {
        return 0.0;
    }
    8363.0 * 1712.0 / f64::from(period)
}

/// Move `period` by `semitones` (positive is higher).
pub fn shift_semitones(period: i32, semitones: i32, linear: bool) -> i32 {
    if linear {
        (period - semitones * SEMITONE).max(1)
    } else if semitones == 0 || period <= 0 {
        period.max(1)
    } else {
        let factor = 2f64.powf(f64::from(-semitones) / 12.0);
        (f64::from(period) * factor).round().max(1.0) as i32
    }
}

/// Hertz of `period` in the song's frequency mode.
pub fn period_hz(period: i32, linear: bool) -> f64 {
    if linear {
        linear_hz(period)
    } else {
        amiga_hz(period)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c4_is_8363_in_both_tables() {
        let linear = linear_hz(linear_period(48, 0));
        assert!((linear - 8363.0).abs() < 0.01, "linear {linear}");
        let amiga = amiga_hz(amiga_period(49, 0));
        assert!((amiga - 8363.0).abs() < 0.01, "amiga {amiga}");
    }

    #[test]
    fn c0_is_four_octaves_under_c4() {
        let hz = amiga_hz(amiga_period(1, 0));
        assert!((hz - 8363.0 / 16.0).abs() < 0.05, "{hz}");
    }
}
