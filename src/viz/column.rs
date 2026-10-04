//! Geometry of one spectrum column.
//!
//! The bar is a solid stack of eighth-blocks in the body color. The peak hold
//! is a separate one-cell cap in the row above that stack (or higher, while
//! it is still falling). It is not the bar's own top cell repainted, and it
//! is not a variable-height block sitting on the floor of whatever row the
//! peak happened to land in — those both read as detached fragments.

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

/// Peak-hold mark. Lower half-block, so the cap sits on the bottom edge of
/// its cell — directly against the bar when that bar fills the cell below.
pub const PEAK_CAP: char = '▄';

/// Paint one column, top row first.
///
/// `level` and `peak` are `0..=1`. The cap is never drawn inside the bar.
/// When the held peak still shares the bar's top cell, the cap moves up one
/// row so it stays a mark above the body instead of recoloring that body.
/// A peak that has fallen only part of the way sits in its own row, with
/// empty rows between it and the bar. Non-finite inputs draw an empty column.
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
    if peak_steps > steps {
        let bar_top = if steps > 0 {
            Some((steps - 1) / 8)
        } else {
            None
        };
        let peak_row = (peak_steps - 1) / 8;
        let cap_from_bottom = match bar_top {
            Some(top) => peak_row.max(top + 1),
            None => peak_row,
        };
        if let Some(from_bottom) = (cap_from_bottom < rows).then_some(cap_from_bottom) {
            let row = rows - 1 - from_bottom;
            if cells[row].ink != ColumnInk::Body {
                cells[row] = ColumnCell {
                    glyph: PEAK_CAP,
                    ink: ColumnInk::Peak,
                };
            }
        }
    }
    cells
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
    fn the_peak_cap_is_the_cell_above_the_bar() {
        // 0.50 fills two rows of a four-row column exactly. 0.62 is the next row.
        let cells = spectrum_column(4, 0.50, 0.62);
        assert_eq!(peak_row(&cells) + 1, body_top(&cells));
        assert_eq!(cells[peak_row(&cells)].glyph, PEAK_CAP);
        assert!(
            cells
                .iter()
                .filter(|cell| cell.ink == ColumnInk::Peak)
                .count()
                == 1
        );
        assert!(body_is_contiguous(&cells));
        assert_ne!(cells[body_top(&cells)].ink, ColumnInk::Peak);
    }

    #[test]
    fn a_peak_inside_the_top_cell_does_not_recolor_the_bar() {
        // Both land in the same eighth-block row. The cap still steps up one cell.
        let cells = spectrum_column(4, 0.40, 0.45);
        let top = body_top(&cells);
        let cap = peak_row(&cells);
        assert_eq!(cap + 1, top, "cap {cap} body {top}: {cells:?}");
        assert_eq!(cells[top].ink, ColumnInk::Body);
        assert_ne!(cells[top].glyph, ' ');
        assert_ne!(cells[top].glyph, PEAK_CAP);
        assert_eq!(cells[cap].ink, ColumnInk::Peak);
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
