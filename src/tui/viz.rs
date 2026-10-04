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

/// Terminal height at which the panel fits beside the pattern and the samples.
pub(crate) const PANEL_MIN_HEIGHT: u16 = 28;

/// Outer height of the spectrum pane, including its border.
///
/// Bar rows are the v0.2.2 pane: 4 on a 30-line terminal (outer 6). That is
/// the smallest height that still fits meters 1–4, so a 30-line window spends
/// 6/30 on the pane. From 40 lines up the pane also stays within 15% of the
/// window, growing to 5 bars around 50 lines and stopping at 6.
/// Shorter than [`PANEL_MIN_HEIGHT`] draws no panel.
pub(crate) fn panel_height(term_height: u16) -> u16 {
    if term_height < PANEL_MIN_HEIGHT {
        return 0;
    }
    let content = if term_height < 48 {
        4
    } else if term_height < 56 {
        5
    } else {
        6
    };
    let outer = content + 2;
    if term_height >= 40 {
        let cap = u16::try_from(u32::from(term_height) * 15 / 100).unwrap_or(u16::MAX);
        outer.min(cap.max(6))
    } else {
        outer
    }
}

/// Columns reserved for the four channel meters, or 0 when the pane is too
/// narrow to show a bar beside the label.
fn meter_columns(width: usize) -> usize {
    if width >= 36 {
        14
    } else if width >= 20 {
        8
    } else {
        0
    }
}

/// Spectrum bars and four channel meters.
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
    let meter_w = u16::try_from(meter_columns(usize::from(inner.width))).unwrap_or(0);
    let spectrum = Rect {
        width: inner.width.saturating_sub(meter_w),
        ..inner
    };
    let meters = Rect {
        x: inner.x.saturating_add(spectrum.width),
        width: meter_w,
        ..inner
    };
    let lines = spectrum_lines(spectrum, app, theme);
    frame.render_widget(Paragraph::new(lines).style(theme.fill()), spectrum);
    if meter_w > 0 {
        let lines = meter_lines(meters, app, theme);
        frame.render_widget(Paragraph::new(lines).style(theme.fill()), meters);
    }
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
            Paragraph::new("Scope    F5 cycles    space plays    Ctrl-R start")
                .style(theme.title()),
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

/// One row per channel, bottom-aligned, so a 4-row pane shows every meter.
fn meter_for_row(row: usize, rows: usize) -> Option<usize> {
    let shown = rows.min(4);
    let start = rows.saturating_sub(shown);
    let index = row.checked_sub(start)?;
    (index < shown).then_some(index)
}

fn meter_lines(area: Rect, app: &App, theme: Theme) -> Vec<Line<'static>> {
    let rows = usize::from(area.height);
    let width = usize::from(area.width);
    let channels = app.channel_count();
    if channels > 4 {
        return wide_meter_lines(rows, width, channels, app, theme);
    }
    let mut lines = Vec::with_capacity(rows);
    for row in 0..rows {
        if let Some(channel) = meter_for_row(row, rows) {
            lines.push(meter_line(app, channel, width, theme));
        } else {
            lines.push(Line::from(Span::styled(" ".repeat(width), theme.fill())));
        }
    }
    lines
}

/// Two meters per row, at most eight, scrolled with the pattern cursor.
///
/// The spectrum pane keeps its height. A `.mod` never takes this path.
fn wide_meter_lines(
    rows: usize,
    width: usize,
    channels: usize,
    app: &App,
    theme: Theme,
) -> Vec<Line<'static>> {
    let pairs = rows.min(4);
    let window = pairs * 2;
    let origin = app.channel_scroll.min(channels.saturating_sub(window)) / 2 * 2;
    let mut lines = Vec::with_capacity(rows);
    let blank = rows.saturating_sub(pairs);
    for _ in 0..blank {
        lines.push(Line::from(Span::styled(" ".repeat(width), theme.fill())));
    }
    for pair in 0..pairs {
        let left = origin + pair * 2;
        let right = left + 1;
        lines.push(wide_meter_line(app, left, right, channels, width, theme));
    }
    lines
}

fn wide_meter_line(
    app: &App,
    left: usize,
    right: usize,
    channels: usize,
    width: usize,
    theme: Theme,
) -> Line<'static> {
    let half = width / 2;
    let mut spans = meter_spans(app, left, channels, half, theme);
    let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
    if right < channels && used < width {
        spans.extend(meter_spans(app, right, channels, width - used, theme));
    }
    Line::from(spans)
}

fn meter_spans(
    app: &App,
    channel: usize,
    channels: usize,
    width: usize,
    theme: Theme,
) -> Vec<Span<'static>> {
    if channel >= channels || width == 0 {
        return vec![Span::styled(" ".repeat(width), theme.fill())];
    }
    let (level, _) = app.viz.meter(channel);
    let label = format!("{:>2}", channel + 1);
    let color = theme
        .channels
        .get(channel % 4)
        .copied()
        .unwrap_or(theme.text);
    let mut spans = vec![Span::styled(
        label.clone(),
        paint(color, theme.background, true),
    )];
    let bar_w = width.saturating_sub(label.chars().count());
    if bar_w == 0 {
        return spans;
    }
    let body = paint(color, theme.background, false);
    let total = bar_w * 8;
    let steps = quantize(level, total);
    let mut chars = String::with_capacity(bar_w);
    for cell in 0..bar_w {
        let filled = steps.saturating_sub(cell * 8).min(8);
        chars.push(HBLOCK[filled]);
    }
    spans.push(Span::styled(chars, body));
    spans
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

fn meter_line(app: &App, channel: usize, width: usize, theme: Theme) -> Line<'static> {
    let (level, _) = app.viz.meter(channel);
    let label = format!("{} ", channel + 1);
    let color = theme.channels.get(channel).copied().unwrap_or(theme.text);
    let mut spans = vec![Span::styled(
        label.clone(),
        paint(color, theme.background, true),
    )];
    let bar_w = width.saturating_sub(label.chars().count());
    if bar_w == 0 {
        return Line::from(spans);
    }
    let body = paint(color, theme.background, false);
    let total = bar_w * 8;
    let steps = quantize(level, total);
    let mut chars = String::with_capacity(bar_w);
    for cell in 0..bar_w {
        let filled = steps.saturating_sub(cell * 8).min(8);
        chars.push(HBLOCK[filled]);
    }
    spans.push(Span::styled(chars, body));
    Line::from(spans)
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

    use crate::module::{Cell, Module, Tag};
    use crate::player::{Playback, PlayerConfig};
    use crate::viz::{VizSnapshot, WINDOW};

    #[test]
    fn the_panel_stays_short_and_within_about_fifteen_percent() {
        assert_eq!(panel_height(24), 0);
        assert_eq!(panel_height(27), 0);
        // 4 bar rows on a 30-line terminal: the v0.2.2 pane (outer 6).
        assert_eq!(panel_height(30), 6);
        assert_eq!(panel_height(36), 6);
        // 40 lines is exactly 15%. 50 lines gains one bar row and stays under.
        assert_eq!(panel_height(40), 6);
        assert_eq!(panel_height(50), 7);
        assert_eq!(panel_height(80), 8);
        for height in 28..40 {
            assert_eq!(panel_height(height), 6, "height {height}");
        }
        for height in 40..120 {
            let outer = panel_height(height);
            assert!(
                u32::from(outer) * 100 <= u32::from(height) * 15,
                "height {height} outer {outer} is over 15%"
            );
            assert!((6..=8).contains(&outer), "height {height} outer {outer}");
        }
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

    #[test]
    fn a_playing_channel_meter_has_nonzero_height_on_a_short_panel() {
        let mut app = loud_channel_app();
        assert!(app.viz.meter(0).0 > 0.4, "level {}", app.viz.meter(0).0);
        // 30-line layout: outer pane is 6, so the bars and meters share 4 rows.
        for &(width, height) in &[(80u16, 30u16), (100, 40), (100, 50)] {
            let buffer = render_app(&mut app, width, height);
            let panel = spectrum_rows(&buffer);
            let outer = panel_height(height);
            assert_eq!(
                panel.len(),
                usize::from(outer),
                "{width}x{height} spectrum rows:\n{}",
                text_of(&buffer)
            );
            assert!(
                u32::from(outer) * 100 <= u32::from(height) * 20,
                "{height} -> {outer}"
            );
            let content = &panel[1..panel.len() - 1];
            assert!(
                content.len() >= 4,
                "meters need 4 rows, got {} at {height}",
                content.len()
            );
            let mut labels = Vec::new();
            for (row_index, row) in content.iter().enumerate() {
                let (spectrum, gutter) = split_meter(row);
                assert!(
                    !gutter.contains('▔') && !gutter.contains('│'),
                    "cap in the meter gutter at {width}x{height} row {row_index}: {gutter}"
                );
                assert!(
                    !spectrum.contains('▔') && !spectrum.contains('│'),
                    "cap in the spectrum at {width}x{height}: {spectrum}"
                );
                if let Some(channel) = gutter.chars().find(|ch| ('1'..='4').contains(ch)) {
                    let eighths = bar_eighths(&gutter);
                    labels.push(channel);
                    if channel == '1' {
                        assert!(
                            eighths > 0,
                            "channel 1 meter has no bar at {width}x{height}: {gutter:?}\n{}",
                            text_of(&buffer)
                        );
                    }
                }
            }
            assert_eq!(
                labels,
                vec!['1', '2', '3', '4'],
                "meters clipped at {width}x{height}: {labels:?}\n{}",
                text_of(&buffer)
            );
        }
    }

    fn loud_channel_app() -> App {
        let mut module = Module::new(Tag::Mk);
        module.set_title("Meter").unwrap();
        module.song_length = 1;
        module.order[0] = 0;
        module.resize_patterns();
        module.samples[0].set_name("tone").unwrap();
        module.samples[0].volume = 64;
        module.samples[0].set_data(vec![100; 4000]).unwrap();
        module.patterns[0].rows[0][0] = Cell {
            sample: 1,
            period: 428,
            effect: 0,
            param: 0,
        };
        let mut playback = Playback::new(PlayerConfig::default());
        playback.start(&module, 0, 0);
        let mut pcm = vec![0i16; WINDOW * 2];
        playback.render(&module, &mut pcm);
        let peaks = playback.channel_peaks();
        assert!(peaks[0] > 1_000, "channel did not play: {peaks:?}");
        let mut stereo = [0i16; WINDOW * 2];
        stereo.copy_from_slice(&pcm[..WINDOW * 2]);
        let mut app = App::new(module);
        let snap = VizSnapshot {
            stereo,
            peaks,
            rate: 44_100,
            gen: 1,
        };
        app.tick_viz(Some(&snap), 0.05);
        app
    }

    fn render_app(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| crate::tui::draw(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn text_of(buf: &ratatui::buffer::Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    /// Spectrum pane rows, including the border, top to bottom.
    fn spectrum_rows(buf: &ratatui::buffer::Buffer) -> Vec<String> {
        let mut rows = Vec::new();
        let mut started = false;
        for y in 0..buf.area.height {
            let mut line = String::new();
            for x in 0..buf.area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            if !started {
                if line.contains("Spectrum") {
                    started = true;
                    rows.push(line);
                }
                continue;
            }
            rows.push(line.clone());
            if line.contains("Play") || line.contains("Stop") || line.contains("VIEW") {
                rows.pop();
                break;
            }
        }
        rows
    }

    /// Spectrum columns and the meter gutter, without the pane's borders.
    fn split_meter(line: &str) -> (String, String) {
        let chars: Vec<char> = line.chars().collect();
        if chars.len() < 16 {
            return (String::new(), chars.into_iter().collect());
        }
        let meter_start = chars.len() - 1 - 14;
        let spectrum: String = chars[1..meter_start].iter().collect();
        let gutter: String = chars[meter_start..chars.len() - 1].iter().collect();
        (spectrum, gutter)
    }

    fn bar_eighths(gutter: &str) -> usize {
        gutter.chars().map(eighths_of).sum()
    }

    fn eighths_of(glyph: char) -> usize {
        match glyph {
            '▏' => 1,
            '▎' => 2,
            '▍' => 3,
            '▌' => 4,
            '▋' => 5,
            '▊' => 6,
            '▉' => 7,
            '█' => 8,
            _ => 0,
        }
    }
}
