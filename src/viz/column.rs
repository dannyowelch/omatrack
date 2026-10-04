//! Geometry of one spectrum column.
//!
//! The bar is a solid stack of eighth-blocks in the body color, drawn from
//! the smoothed level. There is no peak-hold mark above it.

/// What a cell in a spectrum column represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnInk {
    /// Nothing in this row.
    Empty,
    /// Part of the bar body.
    Body,
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

/// Paint one column, top row first.
///
/// `level` is `0..=1`, the smoothed bar height. The column is a stack of
/// eighth-blocks from the bottom and nothing else. Non-finite input draws an
/// empty column.
pub fn spectrum_column(rows: usize, level: f32) -> Vec<ColumnCell> {
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
    cells
}

/// Analyzer band drawn in display column `index`.
///
/// The mapping depends only on the column count and the band count. It does
/// not look at the levels, so a loud neighbor cannot pull the column sideways.
/// `index` past `columns`, or an empty series, is 0.
pub fn column_band(index: usize, columns: usize, bars: usize) -> usize {
    if columns == 0 || bars == 0 {
        return 0;
    }
    let index = index.min(columns - 1);
    let slot = (index as u64).saturating_mul(bars as u64) / columns as u64;
    (slot as usize).min(bars - 1)
}

/// Sample `values` at display column `index`.
///
/// The point is the center of the column, blended between the two bands it
/// falls between. `index` past `columns`, or an empty series, is 0.
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
    fn a_column_always_reads_the_same_band() {
        assert_eq!(column_band(0, 10, 10), 0);
        assert_eq!(column_band(9, 10, 10), 9);
        // Wider than the analyzer: neighboring columns share a band, and the
        // choice does not depend on the levels.
        assert_eq!(column_band(0, 100, 48), 0);
        assert_eq!(column_band(1, 100, 48), 0);
        assert_eq!(column_band(20, 96, 48), 10);
        assert_eq!(column_band(21, 96, 48), 10);
        let left = column_band(40, 104, 48);
        let right = column_band(41, 104, 48);
        assert!(right <= left + 1);
        assert_eq!(column_band(0, 0, 48), 0);
        assert_eq!(column_band(3, 8, 0), 0);
    }

    #[test]
    fn the_bar_is_fractional_blocks_and_nothing_floats_above_it() {
        // 0.50 of four rows is 16 eighths: two full body rows, no mark above.
        let half = spectrum_column(4, 0.50);
        assert!(half
            .iter()
            .all(|cell| cell.ink != ColumnInk::Empty || cell.glyph == ' '));
        assert!(half
            .iter()
            .all(|cell| cell.ink != ColumnInk::Body || is_body(cell.glyph)));
        assert_eq!(body_top(&half), 2);
        assert!(half[..2].iter().all(|cell| cell.ink == ColumnInk::Empty));
        assert!(body_is_contiguous(&half));

        let cells = spectrum_column(4, 0.40);
        assert!(body_is_contiguous(&cells));
        let partial = cells
            .iter()
            .find(|cell| cell.ink == ColumnInk::Body)
            .unwrap();
        assert!(is_body(partial.glyph), "{partial:?}");
        assert_ne!(partial.glyph, '▔');

        let full = spectrum_column(4, 1.0);
        assert!(full
            .iter()
            .all(|cell| cell.ink == ColumnInk::Body && cell.glyph == '█'));
        assert!(spectrum_column(4, 0.0)
            .iter()
            .all(|cell| cell.ink == ColumnInk::Empty && cell.glyph == ' '));
        assert!(spectrum_column(0, 0.5).is_empty());
        assert!(spectrum_column(4, f32::NAN)
            .iter()
            .all(|cell| cell.ink == ColumnInk::Empty));
    }

    fn is_body(glyph: char) -> bool {
        matches!(glyph, '▁' | '▂' | '▃' | '▄' | '▅' | '▆' | '▇' | '█')
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
                ColumnInk::Empty => {
                    if seen {
                        finished = true;
                    }
                }
            }
        }
        seen
    }
}
