//! Classic tracker layout: song header, pattern, sample list.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::module::{Sample, CHANNELS};
use crate::notes::{effect_description, format_finetune, format_period};

use super::app::{App, Focus};
use super::theme::{paint, Theme};

const MIN_WIDTH: u16 = 76;
const MIN_HEIGHT: u16 = 20;
const HELP: &str = "q quit  tab view  arrows/hjkl  pgup/pgdn  home/end  [ ] order  , . pattern";

/// Draw the viewer into `frame`.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let theme = Theme::protracker();
    let area = frame.area();
    frame.render_widget(Block::default().style(theme.fill()), area);

    let Some(regions) = layout(area, app) else {
        draw_too_small(frame, area, theme);
        return;
    };

    let row_window = usize::from(regions.pattern.height.saturating_sub(2)).saturating_sub(2);
    let sample_window = usize::from(regions.samples.height.saturating_sub(2)).saturating_sub(1);
    app.reconcile_scroll(row_window, sample_window);

    draw_song(frame, regions.song, app, theme);
    draw_pattern(frame, regions.pattern, app, theme);
    draw_samples(frame, regions.samples, app, theme);
    frame.render_widget(
        Paragraph::new(clip(status_text(app), regions.status.width)).style(theme.text()),
        regions.status,
    );
    frame.render_widget(
        Paragraph::new(clip(HELP.to_string(), regions.help.width)).style(theme.dim()),
        regions.help,
    );
}

struct Regions {
    song: Rect,
    pattern: Rect,
    samples: Rect,
    status: Rect,
    help: Rect,
}

fn layout(area: Rect, app: &App) -> Option<Regions> {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return None;
    }
    let [body, status, help] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    let inner_w = usize::from(area.width.saturating_sub(2));
    let order_lines = desired_order_lines(app, inner_w);
    let header_h = header_height(body.height, order_lines);
    let sample_h = sample_height(body.height, header_h);
    let [song, pattern, samples] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Fill(1),
        Constraint::Length(sample_h),
    ])
    .areas(body);

    Some(Regions {
        song,
        pattern,
        samples,
        status,
        help,
    })
}

fn header_height(body_h: u16, order_lines: u16) -> u16 {
    let desired = (4 + order_lines).clamp(5, 8);
    let cap = body_h.saturating_sub(9).max(5);
    desired.min(cap).min(body_h)
}

fn sample_height(body_h: u16, header_h: u16) -> u16 {
    let rest = body_h.saturating_sub(header_h);
    let max_sample = rest.saturating_sub(6);
    8.min(max_sample).max(4).min(rest)
}

fn draw_too_small(frame: &mut Frame, area: Rect, theme: Theme) {
    let message = format!(
        "Terminal too small ({}×{}). Need at least {MIN_WIDTH}×{MIN_HEIGHT}.",
        area.width, area.height
    );
    let [_, mid, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(message)
            .style(theme.title())
            .alignment(Alignment::Center),
        mid,
    );
}

fn draw_song(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let block = pane("Song", false, theme);
    let inner_w = usize::from(area.width.saturating_sub(2));
    let inner_h = usize::from(area.height.saturating_sub(2));
    let mut lines = Vec::new();
    if inner_h > 0 {
        let title = app.module.display_title();
        let (text, style) = if title.is_empty() {
            ("(untitled)".to_string(), theme.dim())
        } else {
            (title, theme.title())
        };
        lines.push(styled(text, style));
    }
    if inner_h > 1 {
        lines.push(styled(
            format!(
                "Length {}   Restart {}   Patterns {}   {}",
                app.module.song_length,
                app.module.restart,
                app.module.patterns.len(),
                app.module.tag.as_str(),
            ),
            theme.text(),
        ));
    }
    let room = inner_h.saturating_sub(lines.len());
    if room > 0 && inner_w > 0 {
        lines.extend(order_lines(app, theme, inner_w, room));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_pattern(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let focused = app.focus == Focus::Pattern;
    let block = pane("Pattern", focused, theme);
    let inner_w = usize::from(area.width.saturating_sub(2));
    let inner_h = usize::from(area.height.saturating_sub(2));
    let row_window = inner_h.saturating_sub(2);

    let mut lines = Vec::new();
    if inner_h > 0 {
        lines.push(styled(
            pattern_info(app),
            paint(theme.accent, theme.background, false),
        ));
    }
    if inner_h > 1 {
        lines.push(styled(column_header(), theme.dim()));
    }
    if let Some(pattern) = app.module.patterns.get(app.view_pattern) {
        let start = app.row_offset;
        let end = (start + row_window).min(crate::module::ROWS);
        for row in start..end {
            lines.push(pattern_row(pattern, row, app, theme, inner_w));
        }
    } else if inner_h > 2 {
        lines.push(styled(
            "Pattern is not in the file.".to_string(),
            theme.dim(),
        ));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_samples(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let focused = app.focus == Focus::Samples;
    let block = pane("Samples", focused, theme);
    let inner_w = usize::from(area.width.saturating_sub(2));
    let inner_h = usize::from(area.height.saturating_sub(2));
    let sample_window = inner_h.saturating_sub(1);

    let mut lines = Vec::new();
    if inner_h > 0 {
        lines.push(styled(sample_header(), theme.dim()));
    }
    let start = app.sample_offset;
    let end = (start + sample_window).min(crate::module::SAMPLE_COUNT);
    for index in start..end {
        let selected = index == app.sample;
        let style = if selected && focused {
            paint(theme.cursor_fg, theme.cursor_bg, true)
        } else if selected {
            paint(theme.text, theme.row_bg, false)
        } else {
            theme.text()
        };
        let mut text = sample_row(&app.module.samples[index], index + 1);
        let pad = inner_w.saturating_sub(text.chars().count());
        if selected && pad > 0 {
            text.push_str(&" ".repeat(pad));
        }
        lines.push(styled(text, style));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn pane(title: &str, focused: bool, theme: Theme) -> Block<'static> {
    let label = if focused {
        format!("* {title}")
    } else if title == "Song" {
        "Song".to_string()
    } else {
        format!("  {title}")
    };
    let border = if focused {
        theme.border_focus
    } else {
        theme.border
    };
    Block::bordered()
        .title(Span::styled(label, theme.title()))
        .border_style(paint(border, theme.background, false))
        .style(theme.fill())
}

fn pattern_info(app: &App) -> String {
    let order_pat = usize::from(app.order_pattern());
    let note = if app.view_pattern != order_pat {
        format!(" (order has pat {})", fmt_num(order_pat))
    } else {
        String::new()
    };
    format!(
        "Pat {}{note}  Pos {} of {}  Row {:02}  Channel {}",
        fmt_num(app.view_pattern),
        app.order_pos,
        app.song_len(),
        app.row,
        app.channel + 1,
    )
}

fn fmt_num(value: usize) -> String {
    if value >= 100 {
        format!("{value:03}")
    } else {
        format!("{value:02}")
    }
}

fn column_header() -> String {
    let mut line = "   ".to_string();
    for channel in 1..=CHANNELS {
        if channel > 1 {
            line.push_str(" | ");
        }
        line.push_str(&format!("{:<10}", format!("Ch {channel}")));
    }
    line
}

fn pattern_row(
    pattern: &crate::module::Pattern,
    row: usize,
    app: &App,
    theme: Theme,
    inner_w: usize,
) -> Line<'static> {
    let on_row = row == app.row;
    let row_bg = if on_row {
        theme.row_bg
    } else {
        theme.background
    };
    let mut spans = vec![Span::styled(
        format!("{row:02} "),
        paint(theme.accent, row_bg, false),
    )];
    let mut width = 3usize;
    for channel in 0..CHANNELS {
        if channel > 0 {
            spans.push(Span::styled(" | ", paint(theme.dim, row_bg, false)));
            width += 3;
        }
        let cell = pattern.rows[row][channel];
        let cursor = on_row && channel == app.channel && app.focus == Focus::Pattern;
        let bg = if cursor { theme.cursor_bg } else { row_bg };
        let note_fg = if cursor {
            theme.cursor_fg
        } else if cell.period == 0 {
            theme.dim
        } else {
            theme.note
        };
        let sample_fg = if cursor {
            theme.cursor_fg
        } else if cell.sample == 0 {
            theme.dim
        } else {
            theme.text
        };
        let effect_fg = if cursor {
            theme.cursor_fg
        } else if cell.effect == 0 && cell.param == 0 {
            theme.dim
        } else {
            theme.effect
        };
        spans.push(Span::styled(
            format_period(cell.period),
            paint(note_fg, bg, cursor),
        ));
        spans.push(Span::styled(" ", paint(sample_fg, bg, false)));
        spans.push(Span::styled(
            sample_field(cell.sample),
            paint(sample_fg, bg, cursor),
        ));
        spans.push(Span::styled(" ", paint(effect_fg, bg, false)));
        spans.push(Span::styled(
            effect_field(cell.effect, cell.param),
            paint(effect_fg, bg, cursor),
        ));
        width += 10;
    }
    let pad = inner_w.saturating_sub(width);
    if on_row && pad > 0 {
        spans.push(Span::styled(
            " ".repeat(pad),
            paint(theme.text, row_bg, false),
        ));
    }
    Line::from(spans)
}

fn sample_field(sample: u8) -> String {
    if sample <= 31 {
        format!("{sample:02}")
    } else {
        "??".to_string()
    }
}

fn effect_field(effect: u8, param: u8) -> String {
    if effect > 0x0F {
        format!("?{param:02X}")
    } else {
        format!("{effect:X}{param:02X}")
    }
}

fn sample_header() -> String {
    format!(
        "{:>2} {:<22} {:>6} {:>3} {:>2} {}",
        "#", "Name", "Bytes", "Vol", "Fn", "Loop"
    )
}

fn sample_row(sample: &Sample, number: usize) -> String {
    format!(
        "{number:02} {:<22} {:6} {:3} {:>2} {}",
        fit_chars(&sample.display_name(), 22),
        sample.byte_length(),
        sample.volume,
        format_finetune(sample.finetune_raw),
        format_loop(sample),
    )
}

fn format_loop(sample: &Sample) -> String {
    if sample.loops() {
        let start = u32::from(sample.loop_start) * 2;
        let len = u32::from(sample.loop_length) * 2;
        format!("{start}+{len}")
    } else {
        "-".to_string()
    }
}

fn fit_chars(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    let count = out.chars().count();
    if count < width {
        out.push_str(&" ".repeat(width - count));
    }
    out
}

fn status_text(app: &App) -> String {
    let Some(cell) = app.current_cell() else {
        return "Pattern is not in the file.".to_string();
    };
    let sample_label = if (1..=31).contains(&cell.sample) {
        let name = app.module.samples[usize::from(cell.sample) - 1].display_name();
        if name.is_empty() {
            format!("sample {:02}", cell.sample)
        } else {
            format!("sample {:02} {name}", cell.sample)
        }
    } else if cell.sample == 0 {
        "sample --".to_string()
    } else {
        format!("sample {}", cell.sample)
    };
    format!(
        "{}  {sample_label}  period {}  {} {}",
        format_period(cell.period),
        cell.period,
        effect_field(cell.effect, cell.param),
        effect_description(cell.effect, cell.param),
    )
}

fn order_lines(app: &App, theme: Theme, inner_w: usize, max_lines: usize) -> Vec<Line<'static>> {
    let song_len = app.song_len();
    let entry_w = order_entry_width(app, song_len);
    let per_line = (inner_w / entry_w).max(1);
    let mut rows: Vec<Vec<(String, bool)>> = Vec::new();
    let mut row = Vec::new();
    for index in 0..song_len {
        if row.len() == per_line {
            rows.push(std::mem::take(&mut row));
        }
        let value = app.module.order[index];
        let text = if entry_w >= 4 {
            format!("{value:03} ")
        } else {
            format!("{value:02} ")
        };
        row.push((text, index == app.order_pos));
    }
    if !row.is_empty() {
        rows.push(row);
    }
    let cursor_line = app.order_pos / per_line;
    let rows = window_vec(rows, cursor_line, max_lines);
    rows.into_iter()
        .map(|entries| {
            let spans = entries
                .into_iter()
                .map(|(text, selected)| {
                    let style = if selected {
                        paint(theme.cursor_fg, theme.order_bg, true)
                    } else {
                        theme.text()
                    };
                    Span::styled(text, style)
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect()
}

fn desired_order_lines(app: &App, inner_w: usize) -> u16 {
    let song_len = app.song_len();
    let entry_w = order_entry_width(app, song_len);
    let per_line = (inner_w / entry_w).max(1);
    let lines = song_len.div_ceil(per_line);
    u16::try_from(lines).unwrap_or(u16::MAX)
}

fn order_entry_width(app: &App, song_len: usize) -> usize {
    let max = app.module.order[..song_len]
        .iter()
        .copied()
        .max()
        .unwrap_or(0);
    if max >= 100 {
        4
    } else {
        3
    }
}

fn window_vec<T>(mut rows: Vec<T>, cursor_line: usize, max_lines: usize) -> Vec<T> {
    if max_lines == 0 || rows.is_empty() || rows.len() <= max_lines {
        if max_lines == 0 {
            rows.clear();
        }
        return rows;
    }
    let start = cursor_line
        .saturating_sub(max_lines / 2)
        .min(rows.len() - max_lines);
    rows.drain(start..start + max_lines).collect()
}

fn styled(text: String, style: ratatui::style::Style) -> Line<'static> {
    Line::from(Span::styled(text, style))
}

fn clip(text: String, width: u16) -> String {
    text.chars().take(usize::from(width)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::{Cell, Module, Sample, Tag};
    use crate::tui::app::{command_for, App, Command, Key};
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;

    fn demo() -> App {
        let mut module = Module::new(Tag::Mk);
        module.set_title("Demo Tune").unwrap();
        module.song_length = 2;
        module.restart = 127;
        module.order[0] = 0;
        module.order[1] = 1;
        module.resize_patterns();
        module.samples[0].set_name("kickdrum").unwrap();
        module.samples[0].volume = 64;
        module.samples[0].set_data(vec![1, 2, 3, 4]).unwrap();
        module.samples[1].set_name("snare").unwrap();
        module.samples[1].volume = 48;
        module.samples[1].finetune_raw = 0x0E;
        module.samples[1].loop_start = 1;
        module.samples[1].loop_length = 4;
        module.patterns[0].rows[0][0] = Cell {
            sample: 1,
            period: 856,
            effect: 0xC,
            param: 0x40,
        };
        module.patterns[1].rows[0][0] = Cell {
            sample: 2,
            period: 428,
            effect: 0,
            param: 0,
        };
        App::new(module)
    }

    fn render(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
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

    fn assert_has(screen: &str, needle: &str) {
        assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
    }

    fn find_sequence(buf: &ratatui::buffer::Buffer, seq: &[&str]) -> Option<(u16, u16)> {
        if buf.area.width < seq.len() as u16 {
            return None;
        }
        for y in 0..buf.area.height {
            for x in 0..=buf.area.width - seq.len() as u16 {
                let matched = seq
                    .iter()
                    .enumerate()
                    .all(|(index, symbol)| buf[(x + index as u16, y)].symbol() == *symbol);
                if matched {
                    return Some((x, y));
                }
            }
        }
        None
    }

    #[test]
    fn shows_song_pattern_and_samples_on_a_classic_terminal() {
        let mut app = demo();
        let buf = render(&mut app, 80, 24);
        let screen = text_of(&buf);
        assert_has(&screen, "Demo Tune");
        assert_has(&screen, "Length 2");
        assert_has(&screen, "Restart 127");
        assert_has(&screen, "M.K.");
        assert_has(&screen, "Pat 00");
        assert_has(&screen, "Pos 0 of 2");
        assert_has(&screen, "Row 00");
        assert_has(&screen, "Channel 1");
        assert_has(&screen, "C-1");
        assert_has(&screen, "C40");
        assert_has(&screen, "kickdrum");
        assert_has(&screen, "snare");
        assert_has(&screen, "set volume");
        assert_has(&screen, "q quit");
        assert_has(&screen, "* Pattern");
        assert!(!screen.contains("too small"), "{screen}");

        let (x, y) = find_sequence(&buf, &["C", "-", "1"]).expect("note");
        assert_eq!(buf[(x, y)].bg, Color::Yellow);
    }

    #[test]
    fn keys_move_the_cursor_across_the_pattern_and_samples() {
        let mut app = demo();
        let down = command_for(app.focus, Key::Down).unwrap();
        app.apply(down);
        let screen = text_of(&render(&mut app, 100, 40));
        assert_has(&screen, "Row 01");
        assert!(!screen.contains("Row 00"), "{screen}");

        app.apply(command_for(app.focus, Key::Right).unwrap());
        let screen = text_of(&render(&mut app, 100, 40));
        assert_has(&screen, "Channel 2");
        assert!(!screen.contains("Channel 1"), "{screen}");

        app.apply(command_for(app.focus, Key::Char('.')).unwrap());
        let screen = text_of(&render(&mut app, 100, 40));
        assert_has(&screen, "Pat 01 (order has pat 00)");
        assert_has(&screen, "Pos 0 of 2");
        assert_has(&screen, "C-2");

        app.apply(command_for(app.focus, Key::Char(']')).unwrap());
        let screen = text_of(&render(&mut app, 100, 40));
        assert_has(&screen, "Pos 1 of 2");
        assert!(!screen.contains("order has pat"), "{screen}");

        app.apply(Command::FirstRow);
        app.apply(command_for(app.focus, Key::Tab).unwrap());
        let buf = render(&mut app, 100, 40);
        let screen = text_of(&buf);
        assert_has(&screen, "* Samples");
        assert!(!screen.contains("* Pattern"), "{screen}");
        let (x, y) = find_sequence(&buf, &["k", "i", "c", "k"]).expect("kick");
        assert_eq!(buf[(x, y)].bg, Color::Yellow);

        app.apply(command_for(app.focus, Key::Down).unwrap());
        let buf = render(&mut app, 100, 40);
        let (x, y) = find_sequence(&buf, &["s", "n", "a", "r", "e"]).expect("snare");
        assert_eq!(buf[(x, y)].bg, Color::Yellow);
        assert_has(&text_of(&buf), "-2");
        assert_has(&text_of(&buf), "2+8");
    }

    #[test]
    fn a_tiny_terminal_asks_for_more_room() {
        let mut app = demo();
        let screen = text_of(&render(&mut app, 40, 10));
        assert_has(&screen, "too small");
    }

    #[test]
    fn sample_header_lines_up_with_rows() {
        let sample = Sample {
            name: {
                let mut name = [0u8; 22];
                name[..4].copy_from_slice(b"kick");
                name
            },
            finetune_raw: 0x08,
            volume: 64,
            loop_start: 0,
            loop_length: 1,
            data: vec![0, 1],
        };
        let header = sample_header();
        let row = sample_row(&sample, 1);
        assert_eq!(header.find("Name"), row.find("kick"));
        assert!(row.contains("-8"), "{row}");
        assert!(row.contains("    2"), "{row}");
    }
}
