//! Spectrum panel and the braille scope.
//!
//! Both views read [`crate::viz::VizState`], which the UI thread updates from
//! the latest [`crate::viz::VizSnapshot`]. Neither view is drawn when the
//! terminal is too small for it.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::viz::{draw_scope, spectrum_column, ColumnInk, INK_CHANNEL, INK_GUIDE, INK_SCOPE};

use super::app::App;
use super::theme::{paint, Theme};

const HBLOCK: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

/// Rows the spectrum pane needs, including its border.
pub(crate) const PANEL_HEIGHT: u16 = 6;
/// Terminal height at which the panel fits beside the pattern and the samples.
pub(crate) const PANEL_MIN_HEIGHT: u16 = 28;

/// Spectrum bars and four channel meters.
pub(crate) fn draw_panel(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let block = Block::bordered()
        .title(Span::styled("  Spectrum", theme.title()))
        .border_style(paint(theme.border, theme.background, false))
        .style(theme.fill());
    let inner = block.inner(area);
    let lines = panel_lines(inner, app, theme);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Vectorscope over `area`, with the transport left to the caller.
pub(crate) fn draw_scope_view(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let header = if area.height > 6 { 1 } else { 0 };
    if header == 1 {
        let title = Rect { height: 1, ..area };
        frame.render_widget(
            Paragraph::new("Scope    F5 cycles    space plays").style(theme.title()),
            title,
        );
    }
    let canvas_area = Rect {
        x: area.x,
        y: area.y.saturating_add(header),
        width: area.width,
        height: area.height.saturating_sub(header),
    };
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

fn panel_lines(area: Rect, app: &App, theme: Theme) -> Vec<Line<'static>> {
    let rows = usize::from(area.height);
    let width = usize::from(area.width);
    if rows == 0 || width == 0 {
        return Vec::new();
    }
    let meter_w = if width >= 36 { 14 } else { 0 };
    let columns = width.saturating_sub(meter_w);
    if columns == 0 {
        return Vec::new();
    }
    let empty = theme.fill();
    let mut grid = vec![vec![(' ', empty); columns]; rows];
    for column in 0..columns {
        let level = column_value(app, columns, column, false);
        let peak = column_value(app, columns, column, true).max(level);
        paint_column(&mut grid, column, level, peak, theme);
    }
    let mut lines = Vec::with_capacity(rows);
    for (row, cells) in grid.iter().enumerate() {
        let mut spans = group(cells);
        if meter_w > 0 {
            if let Some(channel) = meter_for_row(row, rows) {
                spans.push(Span::styled(" ", theme.dim()));
                // " " plus "N " leaves the rest of the reserved meter column.
                push_meter(&mut spans, app, channel, meter_w.saturating_sub(3), theme);
            }
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn meter_for_row(row: usize, rows: usize) -> Option<usize> {
    if rows < 4 {
        return (row < rows).then_some(row);
    }
    let start = rows.saturating_sub(4);
    let index = row.checked_sub(start)?;
    (index < 4).then_some(index)
}

fn column_value(app: &App, columns: usize, index: usize, peak: bool) -> f32 {
    let count = app.viz.bar_count();
    if count == 0 || columns == 0 {
        return 0.0;
    }
    let sample = |bar: usize| {
        if peak {
            app.viz.bar_peak(bar)
        } else {
            app.viz.bar(bar)
        }
    };
    if count == 1 {
        return sample(0);
    }
    // Sample the center of the cell and blend the two bands it lands between,
    // so a column boundary does not jump to the louder neighbor.
    let pos =
        ((index as f32 + 0.5) * count as f32 / columns as f32 - 0.5).clamp(0.0, (count - 1) as f32);
    let left = pos.floor() as usize;
    let right = (left + 1).min(count - 1);
    let frac = pos - left as f32;
    sample(left) * (1.0 - frac) + sample(right) * frac
}

fn paint_column(
    grid: &mut [Vec<(char, Style)>],
    column: usize,
    level: f32,
    peak: f32,
    theme: Theme,
) {
    let rows = grid.len();
    let cap = paint(theme.spectrum_peak, theme.background, false);
    for (row, cell) in spectrum_column(rows, level, peak).into_iter().enumerate() {
        let from_bottom = rows - 1 - row;
        let style = match cell.ink {
            ColumnInk::Peak => cap,
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

fn push_meter(
    spans: &mut Vec<Span<'static>>,
    app: &App,
    channel: usize,
    width: usize,
    theme: Theme,
) {
    let (level, peak) = app.viz.meter(channel);
    let label = format!("{} ", channel + 1);
    let color = theme.channels.get(channel).copied().unwrap_or(theme.text);
    spans.push(Span::styled(label, paint(color, theme.background, true)));
    let body = paint(color, theme.background, false);
    let cap = paint(theme.spectrum_peak, theme.background, false);
    let bar_w = width;
    if bar_w == 0 {
        return;
    }
    let total = bar_w * 8;
    let steps = quantize(level, total);
    let peak_steps = quantize(peak.max(level), total);
    let mut buf = String::new();
    let mut current = body;
    let flush = |spans: &mut Vec<Span<'static>>, buf: &mut String, style: Style| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), style));
        }
    };
    for cell in 0..bar_w {
        let start = cell * 8;
        let filled = steps.saturating_sub(start).min(8);
        let floating = peak_steps > steps && peak_steps > start && peak_steps <= start + 8;
        let (glyph, style) = if filled == 0 && floating {
            ('│', cap)
        } else if filled == 0 {
            (' ', body)
        } else if cell == peak_steps.saturating_sub(1) / 8 {
            (HBLOCK[filled], cap)
        } else {
            (HBLOCK[filled], body)
        };
        if style != current {
            flush(spans, &mut buf, current);
            current = style;
        }
        buf.push(glyph);
    }
    flush(spans, &mut buf, current);
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
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use crate::viz::PEAK_CAP;

    #[test]
    fn the_peak_cap_renders_in_the_cell_above_its_bar() {
        let theme = Theme::protracker();
        let rows = 4;
        let columns = 3;
        let mut grid = vec![vec![(' ', theme.fill()); columns]; rows];
        // Close pairs: the held peak shares or just clears the bar's top cell.
        let levels = [0.40_f32, 0.50, 0.62];
        let peaks = [0.45_f32, 0.62, 0.70];
        for (column, (level, peak)) in levels.into_iter().zip(peaks).enumerate() {
            paint_column(&mut grid, column, level, peak, theme);
        }
        let lines: Vec<Line<'static>> = grid.iter().map(|row| Line::from(group(row))).collect();
        let backend = TestBackend::new(columns as u16, rows as u16);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new(lines), frame.area());
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        for column in 0..columns {
            let mut peak_at = None;
            let mut body_at = None;
            for row in 0..rows {
                let cell = &buffer[(column as u16, row as u16)];
                if cell.symbol() == " " {
                    continue;
                }
                if cell.fg == theme.spectrum_peak {
                    assert_eq!(
                        cell.symbol().chars().next(),
                        Some(PEAK_CAP),
                        "column {column} row {row}"
                    );
                    assert!(peak_at.is_none(), "two caps in column {column}");
                    peak_at = Some(row);
                } else if body_at.is_none() {
                    body_at = Some(row);
                }
            }
            let peak_at = peak_at.expect("cap");
            let body_at = body_at.expect("bar");
            assert_eq!(
                peak_at + 1,
                body_at,
                "column {column} cap row {peak_at} is not on top of bar row {body_at}"
            );
        }
    }
}
