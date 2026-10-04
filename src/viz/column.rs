//! Geometry of one spectrum column.
//!
//! The bar is a solid stack of eighth-blocks in the body color. The peak hold
//! is a thin mark (`▁` or `▔`) in the cell that contains the fractional peak,
//! on the lower or upper edge of that cell. It is not a half-block or a full
//! cell, and it is not shoved up a row when it still shares the bar's top cell.

/// What a cell in a spectrum column represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnInk {
    /// Nothing in this row.
    Empty,
    /// Part of the bar body.
    Body,
    /// The peak-hold cap.
    Peak,
}

/// One cell of a column, top row first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnCell {
    /// Glyph to draw. A space when [`ColumnInk::Empty`].
    pub glyph: char,
    /// Which role the glyph plays.
    pub ink: ColumnInk,
}

const VBLOCK: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Thin peak mark on the bottom eighth of its cell.
pub const PEAK_CAP_LOW: char = '▁';
/// Thin peak mark on the top eighth of its cell.
pub const PEAK_CAP_HIGH: char = '▔';

/// Paint one column, top row first.
///
/// `level` and `peak` are `0..=1`. The cap is the eighth that contains the
/// peak: `▁` on the lower half of that cell, `▔` on the upper half. A peak
/// that still sits inside the bar's top cell is not drawn — lifting it a
/// whole row is what stretched a small hold into a floating shelf. A peak
/// that has fallen only part of the way sits in its own cell, with empty
/// rows between it and the bar when the gap is that large. Non-finite inputs
/// draw an empty column.
pub fn spectrum_column(rows: usize, level: f32, peak: f32) -> Vec<ColumnCell> {
    let mut cells = vec![
        ColumnCell {
            glyph: ' ',
            ink: ColumnInk::Empty,
        };
        rows
    ];
    if rows == 0 {
        return cells;
    }
    let total = rows * 8;
    let steps = quantize(level, total);
    let peak_steps = quantize(peak.max(level), total).max(steps);
    for (row, cell) in cells.iter_mut().enumerate() {
        let from_bottom = rows - 1 - row;
        let filled = steps.saturating_sub(from_bottom * 8).min(8);
        if filled > 0 {
            *cell = ColumnCell {
                glyph: VBLOCK[filled],
                ink: ColumnInk::Body,
            };
        }
    }
    if let Some((from_bottom, glyph)) = peak_placement(rows, steps, peak_steps) {
        let row = rows - 1 - from_bottom;
        if cells[row].ink != ColumnInk::Body {
            cells[row] = ColumnCell {
                glyph,
                ink: ColumnInk::Peak,
            };
        }
    }
    cells
}

/// Cell and thin glyph for a peak that clears the bar, counted from the bottom.
///
/// `steps` and `peak_steps` are eighths, `1..=rows*8`. The same cell as the
/// bar's top eighth returns `None` so the body glyph stays put.
fn peak_placement(rows: usize, steps: usize, peak_steps: usize) -> Option<(usize, char)> {
    if rows == 0 || peak_steps <= steps || peak_steps == 0 {
        return None;
    }
    let eighth = peak_steps - 1;
    let from_bottom = eighth / 8;
    if from_bottom >= rows {
        return None;
    }
    let bar_cell = if steps == 0 {
        None
    } else {
        Some((steps - 1) / 8)
    };
    if bar_cell == Some(from_bottom) {
        return None;
    }
    let within = eighth % 8;
    let glyph = if within >= 4 {
        PEAK_CAP_HIGH
    } else {
        PEAK_CAP_LOW
    };
    Some((from_bottom, glyph))
}

/// Sample `values` at display column `index`.
///
/// The point is the center of the column, blended between the two bands it
/// falls between. This is the bar shape. Peak marks are not sampled this way:
/// each column holds its own. `index` past `columns`, or an empty series, is 0.
pub fn sample_series(values: &[f32], columns: usize, index: usize) -> f32 {
    let count = values.len();
    if count == 0 || columns == 0 || index >= columns {
        return 0.0;
    }
    let sample = |slot: usize| {
        let value = values.get(slot).copied().unwrap_or(0.0);
        if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    if count == 1 {
        return sample(0);
    }
    let pos =
        ((index as f32 + 0.5) * count as f32 / columns as f32 - 0.5).clamp(0.0, (count - 1) as f32);
    let left = pos.floor() as usize;
    let right = (left + 1).min(count - 1);
    let frac = pos - left as f32;
    sample(left) * (1.0 - frac) + sample(right) * frac
}

fn quantize(level: f32, total: usize) -> usize {
    if total == 0 || !level.is_finite() {
        return 0;
    }
    let steps = (level.clamp(0.0, 1.0) * total as f32).round() as usize;
    steps.min(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_peak_cap_is_a_thin_glyph_at_the_fractional_height() {
        // 0.50 fills two rows exactly (16 eighths). 0.62 is 20 eighths: cell 2
        // from the bottom, lower half, so ▁ in the row just above the bar.
        let cells = spectrum_column(4, 0.50, 0.62);
        let cap = peak_row(&cells);
        assert_eq!(cap, 1, "row from the top: {cells:?}");
        assert_eq!(cells[cap].glyph, PEAK_CAP_LOW);
        assert_eq!(cells[cap].ink, ColumnInk::Peak);
        assert_eq!(body_top(&cells), 2);
        assert_eq!(
            cells
                .iter()
                .filter(|cell| cell.ink == ColumnInk::Peak)
                .count(),
            1
        );
        assert!(body_is_contiguous(&cells));
        assert!(cells
            .iter()
            .all(|cell| cell.ink != ColumnInk::Peak || is_thin(cell.glyph)));

        // 0.90 is 29 eighths: top cell, upper half, so ▔.
        let high = spectrum_column(4, 0.50, 0.90);
        assert_eq!(high[0].ink, ColumnInk::Peak);
        assert_eq!(high[0].glyph, PEAK_CAP_HIGH);
        assert_ne!(high[0].glyph, '▄');
        assert_ne!(high[0].glyph, '█');
    }

    #[test]
    fn a_peak_inside_the_top_cell_does_not_float_a_row_above_the_bar() {
        // 0.40 is 13 eighths and 0.45 is 14, both in the same cell. Lifting the
        // cap a full row painted the shelf in the screenshot.
        let cells = spectrum_column(4, 0.40, 0.45);
        assert!(
            cells.iter().all(|cell| cell.ink != ColumnInk::Peak),
            "false cap: {cells:?}"
        );
        assert_eq!(cells[body_top(&cells)].ink, ColumnInk::Body);
        assert_ne!(cells[body_top(&cells)].glyph, ' ');
    }

    #[test]
    fn the_body_uses_fractional_blocks_and_a_full_bar_has_no_cap() {
        let cells = spectrum_column(4, 0.40, 0.40);
        assert!(cells.iter().all(|cell| cell.ink != ColumnInk::Peak));
        assert!(body_is_contiguous(&cells));
        let partial = cells
            .iter()
            .find(|cell| cell.ink == ColumnInk::Body)
            .unwrap();
        assert!(
            matches!(partial.glyph, '▁' | '▂' | '▃' | '▄' | '▅' | '▆' | '▇' | '█'),
            "{partial:?}"
        );

        let full = spectrum_column(4, 1.0, 1.0);
        assert!(full
            .iter()
            .all(|cell| cell.ink == ColumnInk::Body && cell.glyph == '█'));
        assert!(spectrum_column(4, 0.0, 0.0)
            .iter()
            .all(|cell| cell.ink == ColumnInk::Empty));
        assert!(spectrum_column(0, 0.5, 0.8).is_empty());
        assert!(spectrum_column(4, f32::NAN, 0.5)
            .iter()
            .all(|cell| cell.ink == ColumnInk::Empty || cell.ink == ColumnInk::Peak));
    }

    fn is_thin(glyph: char) -> bool {
        glyph == PEAK_CAP_LOW || glyph == PEAK_CAP_HIGH
    }

    fn peak_row(cells: &[ColumnCell]) -> usize {
        cells
            .iter()
            .position(|cell| cell.ink == ColumnInk::Peak)
            .expect("peak cap")
    }

    fn body_top(cells: &[ColumnCell]) -> usize {
        cells
            .iter()
            .position(|cell| cell.ink == ColumnInk::Body)
            .expect("bar")
    }

    fn body_is_contiguous(cells: &[ColumnCell]) -> bool {
        let mut seen = false;
        let mut finished = false;
        for cell in cells.iter().rev() {
            match cell.ink {
                ColumnInk::Body => {
                    if finished {
                        return false;
                    }
                    seen = true;
                }
                ColumnInk::Empty | ColumnInk::Peak => {
                    if seen {
                        finished = true;
                    }
                }
            }
        }
        seen
    }
}
