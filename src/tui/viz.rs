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

use crate::viz::{draw_scope, INK_CHANNEL, INK_GUIDE, INK_SCOPE};

use super::app::App;
use super::theme::{paint, Theme};

const VBLOCK: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
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
    let body = paint(theme.spectrum, theme.background, false);
    let cap = paint(theme.spectrum_peak, theme.background, false);
    let mut grid = vec![vec![(' ', body); columns]; rows];
    for column in 0..columns {
        let level = column_value(app, columns, column, false);
        let peak = column_value(app, columns, column, true).max(level);
        paint_column(&mut grid, column, level, peak, body, cap);
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
    let start = index * count / columns;
    let end = ((index + 1) * count / columns).max(start + 1).min(count);
    let mut best = 0.0f32;
    for bar in start..end {
        let value = if peak {
            app.viz.bar_peak(bar)
        } else {
            app.viz.bar(bar)
        };
        best = best.max(value);
    }
    best
}

fn paint_column(
    grid: &mut [Vec<(char, Style)>],
    column: usize,
    level: f32,
    peak: f32,
    body: Style,
    cap: Style,
) {
    let rows = grid.len();
    let total = rows * 8;
    let steps = quantize(level, total);
    let peak_steps = quantize(peak, total).max(steps);
    for (row, line) in grid.iter_mut().enumerate() {
        let from_bottom = rows - 1 - row;
        let start = from_bottom * 8;
        let filled = steps.saturating_sub(start).min(8);
        let floating = peak_steps > steps && peak_steps > start && peak_steps <= start + 8;
        let (glyph, style) = if filled == 0 && floating {
            let mark = (peak_steps - start).clamp(1, 8);
            (VBLOCK[mark], cap)
        } else if filled == 0 {
            (' ', body)
        } else if from_bottom == steps.saturating_sub(1) / 8 {
            (VBLOCK[filled], cap)
        } else {
            (VBLOCK[filled], body)
        };
        line[column] = (glyph, style);
    }
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
