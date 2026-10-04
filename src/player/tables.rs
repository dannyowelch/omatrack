//! PAL periods, finetune tables, and the ProTracker vibrato waveform.
//!
//! Pattern cells store a finetune-0 period. Playback looks that period up in
//! [`crate::notes::PERIODS`] and reads the same note from the channel's
//! finetune table. The tables are the ones in the ProTracker 2.1 playroutine
//! (three octaves, C-1 through B-3).

use crate::notes::{finetune_from_byte, PERIODS};

/// Paula's PAL clock. A period plays at `PAL_CLOCK_HZ / period` samples per second.
pub const PAL_CLOCK_HZ: u32 = 3_546_895;

/// Lowest period a slide may reach (highest ProTracker note, B-3).
pub const MIN_PERIOD: u16 = 113;

/// Highest period a slide may reach (lowest ProTracker note, C-1).
pub const MAX_PERIOD: u16 = 856;

/// Fixed-point fractional bits of a sample cursor.
pub(crate) const FP_SHIFT: u32 = 16;

/// Finetune nibble `0..=15` → 36 periods, C-1 through B-3.
///
/// Index 0 is finetune 0, 1..=7 are +1..=+7, 8..=15 are -8..=-1. That is the
/// order stored in the sample header.
pub const FINETUNE_PERIODS: [[u16; 36]; 16] = [
    // 0
    [
        856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320,
        302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
    ],
    // +1
    [
        850, 802, 757, 715, 674, 637, 601, 567, 535, 505, 477, 450, 425, 401, 379, 357, 337, 318,
        300, 284, 268, 253, 239, 225, 213, 201, 189, 179, 169, 159, 150, 142, 134, 126, 119, 113,
    ],
    // +2
    [
        844, 796, 752, 709, 670, 632, 597, 563, 532, 502, 474, 447, 422, 398, 376, 355, 335, 316,
        298, 282, 266, 251, 237, 224, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118, 112,
    ],
    // +3
    [
        838, 791, 746, 704, 665, 628, 592, 559, 528, 498, 470, 444, 419, 395, 373, 352, 332, 314,
        296, 280, 264, 249, 235, 222, 209, 198, 187, 176, 166, 157, 148, 140, 132, 125, 118, 111,
    ],
    // +4
    [
        832, 785, 741, 699, 660, 623, 588, 555, 524, 495, 467, 441, 416, 392, 370, 350, 330, 312,
        294, 278, 262, 247, 233, 220, 208, 196, 185, 175, 165, 156, 147, 139, 131, 124, 117, 110,
    ],
    // +5
    [
        826, 779, 736, 694, 655, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328, 309,
        292, 276, 260, 245, 232, 219, 206, 195, 184, 174, 164, 155, 146, 138, 130, 123, 116, 109,
    ],
    // +6
    [
        820, 774, 730, 689, 651, 614, 580, 547, 516, 487, 460, 434, 410, 387, 365, 345, 325, 307,
        290, 274, 258, 244, 230, 217, 205, 193, 183, 172, 163, 154, 145, 137, 129, 122, 115, 109,
    ],
    // +7
    [
        814, 768, 725, 684, 646, 610, 575, 543, 513, 484, 457, 431, 407, 384, 363, 342, 323, 305,
        288, 272, 256, 242, 228, 216, 204, 192, 181, 171, 161, 152, 144, 136, 128, 121, 114, 108,
    ],
    // -8
    [
        907, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339,
        320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120,
    ],
    // -7
    [
        900, 850, 802, 757, 715, 675, 636, 601, 567, 535, 505, 477, 450, 425, 401, 379, 357, 337,
        318, 300, 284, 268, 253, 238, 225, 212, 200, 189, 179, 169, 159, 150, 142, 134, 126, 119,
    ],
    // -6
    [
        894, 844, 796, 752, 709, 670, 632, 597, 563, 532, 502, 474, 447, 422, 398, 376, 355, 335,
        316, 298, 282, 266, 251, 237, 223, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118,
    ],
    // -5
    [
        887, 838, 791, 746, 704, 665, 628, 592, 559, 528, 498, 470, 444, 419, 395, 373, 352, 332,
        314, 296, 280, 264, 249, 235, 222, 209, 198, 187, 176, 166, 157, 148, 140, 132, 125, 118,
    ],
    // -4
    [
        881, 832, 785, 741, 699, 660, 623, 588, 555, 524, 494, 467, 441, 416, 392, 370, 350, 330,
        312, 294, 278, 262, 247, 233, 220, 208, 196, 185, 175, 165, 156, 147, 139, 131, 123, 117,
    ],
    // -3
    [
        875, 826, 779, 736, 694, 655, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328,
        309, 292, 276, 260, 245, 232, 219, 206, 195, 184, 174, 164, 155, 146, 138, 130, 123, 116,
    ],
    // -2
    [
        868, 820, 774, 730, 689, 651, 614, 580, 547, 516, 487, 460, 434, 410, 387, 365, 345, 325,
        307, 290, 274, 258, 244, 230, 217, 205, 193, 183, 172, 163, 154, 145, 137, 129, 122, 115,
    ],
    // -1
    [
        862, 814, 768, 725, 684, 646, 610, 575, 543, 513, 484, 457, 431, 407, 384, 363, 342, 323,
        305, 288, 272, 256, 242, 228, 216, 203, 192, 181, 171, 161, 152, 144, 136, 128, 121, 114,
    ],
];

/// ProTracker vibrato / tremolo half-wave. Indexed by `position & 31`.
pub(crate) const SINE: [u8; 32] = [
    0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244,
    235, 224, 212, 197, 180, 161, 141, 120, 97, 74, 49, 24,
];

/// How a sample cursor advances, in 16.16 steps, for one output frame.
pub fn sample_step(period: u16, output_rate: u32) -> u64 {
    if period == 0 || output_rate == 0 {
        return 0;
    }
    let denom = u64::from(period) * u64::from(output_rate);
    (u64::from(PAL_CLOCK_HZ) << FP_SHIFT) / denom
}

/// Output frames in one tick. Tempo 125 is 50 Hz, so `rate * 5 / (tempo * 2)`.
pub fn samples_per_tick(sample_rate: u32, tempo: u8) -> u32 {
    let tempo = u32::from(tempo).max(1);
    let rate = sample_rate.max(1);
    let samples = u64::from(rate) * 5 / (u64::from(tempo) * 2);
    u32::try_from(samples.max(1)).unwrap_or(u32::MAX)
}

/// Paula rate for `period`, in hertz.
pub fn pal_hz(period: u16) -> f64 {
    if period == 0 {
        0.0
    } else {
        f64::from(PAL_CLOCK_HZ) / f64::from(period)
    }
}

pub(crate) fn finetune_table(finetune: u8) -> &'static [u16; 36] {
    &FINETUNE_PERIODS[usize::from(finetune & 0x0F)]
}

/// Period a pattern note actually plays at, after the channel finetune.
///
/// `period` is the finetune-0 value stored in the cell. Periods that are not
/// in the finetune-0 table are returned unchanged.
pub fn tuned_period(period: u16, finetune: u8) -> u16 {
    if period == 0 {
        return 0;
    }
    match PERIODS.iter().position(|&entry| entry == period) {
        Some(index) => finetune_table(finetune)[index],
        None => period,
    }
}

/// Move `semitones` up the finetune table from the note nearest `period`.
pub(crate) fn semitone_period(period: u16, finetune: u8, semitones: u8) -> u16 {
    if period == 0 || semitones == 0 {
        return period;
    }
    let table = finetune_table(finetune);
    let index = table
        .iter()
        .position(|&entry| entry <= period)
        .unwrap_or(table.len() - 1);
    let next = index
        .saturating_add(usize::from(semitones))
        .min(table.len() - 1);
    table[next]
}

/// Closest finetune-table period. Used when glissando is on.
pub(crate) fn nearest_period(period: u16, finetune: u8) -> u16 {
    let table = finetune_table(finetune);
    let mut best = table[0];
    let mut best_dist = period.abs_diff(best);
    for &entry in &table[1..] {
        let dist = period.abs_diff(entry);
        if dist < best_dist {
            best = entry;
            best_dist = dist;
        }
    }
    best
}

/// Signed LFO value for vibrato position `pos` (`0..64`) and waveform nibble.
///
/// Negative values raise the pitch when added to a period. Waveform bits 0–1
/// select sine, ramp, or square. Bit 2 is the "do not retrigger" flag and is
/// ignored here.
pub(crate) fn lfo(pos: u8, waveform: u8) -> i16 {
    let pos = pos & 63;
    let index = usize::from(pos & 31);
    let magnitude = match waveform & 3 {
        0 => i16::from(SINE[index]),
        1 => i16::from(index as u8) << 3,
        _ => 255,
    };
    if pos < 32 {
        -magnitude
    } else {
        magnitude
    }
}

/// Pattern-break parameter as a row. ProTracker reads the byte as two decimal
/// digits (`D16` is row 16). Results past row 63 become row 0.
pub(crate) fn break_row(param: u8) -> u8 {
    let row = (param >> 4) * 10 + (param & 0x0F);
    if row >= 64 {
        0
    } else {
        row
    }
}

pub(crate) fn signed_finetune(finetune: u8) -> i8 {
    finetune_from_byte(finetune)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finetune_zero_matches_the_note_table_and_pal_c2() {
        assert_eq!(FINETUNE_PERIODS[0].as_slice(), PERIODS.as_slice());
        let hz = pal_hz(856);
        assert!((hz - 4143.569).abs() < 0.001, "{hz}");
        assert!((pal_hz(428) - 8287.138).abs() < 0.001);
        assert_eq!(tuned_period(428, 0), 428);
        assert_eq!(tuned_period(428, 7), 407);
        assert_eq!(tuned_period(428, 8), 453);
        assert_eq!(tuned_period(0, 3), 0);
        assert_eq!(tuned_period(900, 1), 900);
        assert_eq!(signed_finetune(8), -8);
    }

    #[test]
    fn tick_length_and_sample_step_follow_the_pal_clock() {
        assert_eq!(samples_per_tick(44_100, 125), 882);
        assert_eq!(samples_per_tick(44_100, 150), 735);
        assert_eq!(samples_per_tick(100, 125), 2);
        let step = sample_step(428, 44_100);
        let advanced = (step * 44_100) >> FP_SHIFT;
        let expected = u64::from(PAL_CLOCK_HZ) / 428;
        assert!(advanced.abs_diff(expected) <= 1, "{advanced} vs {expected}");
    }

    #[test]
    fn arpeggio_and_break_rows_use_protracker_numbering() {
        assert_eq!(semitone_period(428, 0, 4), 339);
        assert_eq!(semitone_period(428, 0, 7), 285);
        assert_eq!(semitone_period(428, 0, 0), 428);
        assert_eq!(break_row(0x16), 16);
        assert_eq!(break_row(0x00), 0);
        assert_eq!(break_row(0x0A), 10);
        assert_eq!(break_row(0x64), 0);
    }
}
