//! Spectrum panel, channel-meter glyphs, and the braille scope.
//!
//! The spectrum and the scope share the sample list's row. Channel meters are
//! drawn under the pattern columns; [`meter_bar`] is the pair of strings that
//! row uses. All three read [`crate::viz::VizState`], which the UI thread
//! updates from the latest [`crate::viz::VizSnapshot`].

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::viz::{draw_scope, spectrum_column, ColumnInk, INK_CHANNEL, INK_GUIDE, INK_SCOPE};

use super::app::App;
use super::theme::{paint, Theme};

const HBLOCK: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

/// Horizontal meter for one pattern column.
///
/// `filled` is the level, left to right, in eighth-blocks. `rest` is the
/// unfilled remainder, one lower-eighth block per cell, so a silent channel
/// still shows the meter lane. The two strings together are `width` columns.
pub(crate) fn meter_bar(level: f32, width: usize) -> (String, String) {
    if width == 0 {
        return (String::new(), String::new());
    }
    let total = width.saturating_mul(8);
    let steps = quantize(level, total);
    let mut filled = String::new();
    let mut rest = String::new();
    for cell in 0..width {
        let eighths = steps.saturating_sub(cell * 8).min(8);
        if eighths == 0 {
            rest.push('▁');
        } else {
            filled.push(HBLOCK[eighths]);
        }
    }
    (filled, rest)
}

/// Spectrum bars across the whole pane. Channel meters live under the pattern.
pub(crate) fn draw_panel(frame: &mut Frame, area: Rect, app: &mut App, theme: Theme) {
    let block = Block::bordered()
        .title(Span::styled("  Spectrum", theme.title()))
        .border_style(paint(theme.border, theme.background, false))
        .style(theme.fill());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let lines = spectrum_lines(inner, app, theme);
    frame.render_widget(Paragraph::new(lines).style(theme.fill()), inner);
}

/// Vectorscope in the pane beside the sample list.
pub(crate) fn draw_scope_view(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let block = Block::bordered()
        .title(Span::styled("  Scope", theme.title()))
        .border_style(paint(theme.border, theme.background, false))
        .style(theme.fill());
    let canvas_area = block.inner(area);
    frame.render_widget(block, area);
    if canvas_area.width == 0 || canvas_area.height == 0 {
        return;
    }
    let width = usize::from(canvas_area.width);
    let height = usize::from(canvas_area.height);
    let levels = [
        app.viz.meter(0).0,
        app.viz.meter(1).0,
        app.viz.meter(2).0,
        app.viz.meter(3).0,
    ];
    let canvas = draw_scope(width, height, app.viz.stereo(), levels, app.viz.phase());
    if canvas.width == 0 || canvas.height == 0 {
        return;
    }
    let mut lines = Vec::with_capacity(canvas.height);
    for y in 0..canvas.height {
        lines.push(scope_line(&canvas, y, theme));
    }
    frame.render_widget(Paragraph::new(lines).style(theme.fill()), canvas_area);
}

fn spectrum_lines(area: Rect, app: &mut App, theme: Theme) -> Vec<Line<'static>> {
    let rows = usize::from(area.height);
    let columns = usize::from(area.width);
    if rows == 0 || columns == 0 {
        return Vec::new();
    }
    let empty = theme.fill();
    let mut grid = vec![vec![(' ', empty); columns]; rows];
    app.viz.set_column_count(columns);
    for column in 0..columns {
        paint_column(&mut grid, column, app.viz.column_level(column), theme);
    }
    grid.iter().map(|row| Line::from(group(row))).collect()
}

fn paint_column(grid: &mut [Vec<(char, Style)>], column: usize, level: f32, theme: Theme) {
    let rows = grid.len();
    for (row, cell) in spectrum_column(rows, level).into_iter().enumerate() {
        let from_bottom = rows - 1 - row;
        let style = match cell.ink {
            ColumnInk::Body => body_style(theme, from_bottom, rows),
            ColumnInk::Empty => theme.fill(),
        };
        grid[row][column] = (cell.glyph, style);
    }
}

/// Body color for one row. Truecolor themes darken toward the background at
/// the bottom of the column; named ANSI colors stay the single spectrum color.
fn body_style(theme: Theme, from_bottom: usize, rows: usize) -> Style {
    paint(
        body_color(theme, from_bottom, rows),
        theme.background,
        false,
    )
}

fn body_color(theme: Theme, from_bottom: usize, rows: usize) -> ratatui::style::Color {
    let (ratatui::style::Color::Rgb(sr, sg, sb), ratatui::style::Color::Rgb(br, bg, bb)) =
        (theme.spectrum, theme.background)
    else {
        return theme.spectrum;
    };
    let t = if rows <= 1 {
        1.0
    } else {
        from_bottom as f32 / (rows - 1) as f32
    };
    // The top of the bar is the theme color. Lower rows ease toward the
    // background so the column has a height gradient without a second hue.
    let mix = 0.45 + 0.55 * t;
    ratatui::style::Color::Rgb(lerp(br, sr, mix), lerp(bg, sg, mix), lerp(bb, sb, mix))
}

fn lerp(from: u8, to: u8, t: f32) -> u8 {
    let value = f32::from(from) + (f32::from(to) - f32::from(from)) * t;
    value.round().clamp(0.0, 255.0) as u8
}

fn scope_line(canvas: &crate::viz::ScopeCanvas, y: usize, theme: Theme) -> Line<'static> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut current = theme.fill();
    for x in 0..canvas.width {
        let glyph = canvas.glyph(x, y);
        let style = if glyph == ' ' {
            theme.fill()
        } else {
            paint(
                ink_color(theme, canvas.ink_at(x, y)),
                theme.background,
                false,
            )
        };
        if style != current && !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut buf), current));
        }
        current = style;
        buf.push(glyph);
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, current));
    }
    Line::from(spans)
}

fn ink_color(theme: Theme, ink: u8) -> ratatui::style::Color {
    if ink == INK_SCOPE {
        theme.waveform
    } else if ink == INK_GUIDE {
        theme.dim
    } else if (INK_CHANNEL..INK_CHANNEL + 4).contains(&ink) {
        theme.channels[usize::from(ink - INK_CHANNEL)]
    } else {
        theme.dim
    }
}

fn group(cells: &[(char, Style)]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut current: Option<Style> = None;
    for &(glyph, style) in cells {
        if current.is_some_and(|active| active != style) {
            if let Some(active) = current {
                spans.push(Span::styled(std::mem::take(&mut buf), active));
            }
        }
        current = Some(style);
        buf.push(glyph);
    }
    if let Some(active) = current {
        if !buf.is_empty() {
            spans.push(Span::styled(buf, active));
        }
    }
    spans
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
    fn meter_bar_grows_from_the_left_and_keeps_a_lane() {
        let (filled, rest) = meter_bar(0.0, 4);
        assert_eq!(filled, "");
        assert_eq!(rest, "▁▁▁▁");
        let (filled, rest) = meter_bar(1.0, 10);
        assert_eq!(filled, "██████████");
        assert_eq!(rest, "");
        let (filled, rest) = meter_bar(0.5, 4);
        assert_eq!(filled.chars().count() + rest.chars().count(), 4);
        assert!(filled.starts_with('█'), "{filled}");
        assert!(rest.chars().all(|glyph| glyph == '▁'), "{rest}");
        assert!(filled.contains('█') && !filled.contains('▔') && !filled.contains('│'));
    }

    #[test]
    fn spectrum_columns_are_bars_without_a_cap() {
        let theme = Theme::protracker();
        let rows = 4;
        let mut grid = vec![vec![(' ', theme.fill()); 1]; rows];
        paint_column(&mut grid, 0, 0.50, theme);
        assert!(grid.iter().all(|row| {
            let (glyph, style) = row[0];
            style != paint(theme.spectrum_peak, theme.background, false)
                && glyph != '▔'
                && glyph != '│'
        }));
        // 0.50 of four rows is two full blocks and nothing above them.
        assert_eq!(grid[0][0].0, ' ');
        assert_eq!(grid[1][0].0, ' ');
        assert_eq!(grid[2][0].0, '█');
        assert_eq!(grid[3][0].0, '█');
    }
}
