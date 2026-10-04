//! A small block-character waveform for the sample editor.
//!
//! Each column is the peak of the bytes that fall in it. Loop points are a
//! separate ruler so the drawing stays readable in a normal terminal.

/// Vertical blocks, quietest first. Index 0 is silence.
const BLOCKS: [char; 8] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇'];

/// `rows` lines of a min/max waveform. Row 0 is the top (positive peaks).
///
/// A single row falls back to [`waveform_row`], which uses block heights.
pub fn waveform_grid(data: &[u8], columns: usize, rows: usize) -> Vec<String> {
    if rows <= 1 {
        return vec![waveform_row(data, columns)];
    }
    if columns == 0 {
        return vec![String::new(); rows];
    }
    let mut grid = vec![vec![' '; columns]; rows];
    if data.is_empty() {
        return grid
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect();
    }
    let last = rows - 1;
    for column in 0..columns {
        let (start, end) = bucket(data.len(), columns, column);
        let mut low = 0i8;
        let mut high = 0i8;
        for byte in &data[start..end] {
            let value = *byte as i8;
            low = low.min(value);
            high = high.max(value);
        }
        let top = amplitude_row(high, rows);
        let bottom = amplitude_row(low, rows);
        let (from, to) = if top <= bottom {
            (top, bottom)
        } else {
            (bottom, top)
        };
        for row in grid.iter_mut().take(to.min(last) + 1).skip(from) {
            row[column] = '█';
        }
    }
    grid.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

fn amplitude_row(value: i8, rows: usize) -> usize {
    let last = rows.saturating_sub(1).max(1);
    let shifted = i32::from(value) - i32::from(i8::MIN);
    let row = (255 - shifted) * i32::try_from(last).unwrap_or(1) / 255;
    usize::try_from(row).unwrap_or(0).min(rows - 1)
}

/// One row of the waveform, `columns` cells wide.
pub fn waveform_row(data: &[u8], columns: usize) -> String {
    if columns == 0 {
        return String::new();
    }
    if data.is_empty() {
        return " ".repeat(columns);
    }
    let mut line = String::with_capacity(columns * 3);
    for column in 0..columns {
        let (start, end) = bucket(data.len(), columns, column);
        let mut peak = 0u8;
        for byte in &data[start..end] {
            peak = peak.max((*byte as i8).unsigned_abs());
        }
        let level = usize::from(peak) * (BLOCKS.len() - 1) / 128;
        line.push(BLOCKS[level.min(BLOCKS.len() - 1)]);
    }
    line
}

/// A ruler the same width as [`waveform_row`], with `|` at `points` (byte offsets).
///
/// Points that land on the same column collapse to `+`.
pub fn marker_row(data_len: usize, columns: usize, points: &[usize]) -> String {
    let mut row = vec![' '; columns.max(0)];
    if columns == 0 || data_len == 0 {
        return row.into_iter().collect();
    }
    for point in points {
        if *point > data_len {
            continue;
        }
        let column = column_for(*point, data_len, columns);
        let cell = &mut row[column];
        *cell = if *cell == ' ' { '|' } else { '+' };
    }
    row.into_iter().collect()
}

fn bucket(len: usize, columns: usize, column: usize) -> (usize, usize) {
    let start = column * len / columns;
    let end = (column + 1) * len / columns;
    let end = end.max(start + 1).min(len);
    (start.min(len), end)
}

fn column_for(byte: usize, len: usize, columns: usize) -> usize {
    if len == 0 || columns == 0 {
        return 0;
    }
    let byte = byte.min(len);
    let column = byte.saturating_mul(columns) / len;
    column.min(columns - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spike_on_the_left_is_taller_than_the_rest() {
        let mut data = vec![0u8; 64];
        data[0] = 127;
        data[1] = 129; // -127
        let row = waveform_row(&data, 8);
        assert_eq!(row.chars().count(), 8);
        let chars: Vec<char> = row.chars().collect();
        assert_ne!(chars[0], ' ');
        assert!(
            chars[1..].iter().all(|ch| *ch == ' ' || *ch == '▁'),
            "{row}"
        );
    }

    #[test]
    fn loop_marks_land_on_the_ruler() {
        let ruler = marker_row(100, 10, &[0, 50]);
        assert_eq!(ruler.chars().count(), 10);
        let chars: Vec<char> = ruler.chars().collect();
        assert_eq!(chars[0], '|');
        assert_eq!(chars[5], '|');
        let collapsed = marker_row(4, 2, &[0, 1]);
        assert!(
            collapsed.contains('+') || collapsed.matches('|').count() == 2,
            "{collapsed}"
        );
    }

    #[test]
    fn positive_and_negative_peaks_sit_on_opposite_rows() {
        let mut data = vec![0u8; 16];
        data[0] = 127;
        data[8] = 128; // -128
        let grid = waveform_grid(&data, 2, 5);
        assert_eq!(grid.len(), 5);
        assert!(grid[0].starts_with('█'), "top row: {:?}", grid[0]);
        assert!(
            grid[4].contains('█'),
            "bottom row should hold the negative peak: {:?}",
            grid[4]
        );
    }

    #[test]
    fn empty_sample_is_blank() {
        assert_eq!(waveform_row(&[], 4), "    ");
        assert_eq!(marker_row(0, 4, &[0]), "    ");
        assert_eq!(waveform_row(&[127, 127], 0), "");
    }
}
