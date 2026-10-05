//! Tracker layout: song header, pattern with a meter under each column, and a
//! sample list. The spectrum or the scope shares the sample list's row.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::convert::rate_for_note;
use crate::edit::Field;
use crate::module::{Cell, Sample, CHANNELS, SAMPLE_NAME_LEN, TITLE_LEN};
use crate::notes::{effect_description, format_finetune, format_period};
use crate::waveform::{marker_row, waveform_row};

use crate::viz::VizMode;

use super::app::{App, Focus, Followup, Overlay, TextTarget};
use super::sample::{FieldKind, FieldPrompt, ImportPrompt, PathKind, PathPrompt};
use super::theme::{paint, Theme};
use super::viz;

const MIN_WIDTH: u16 = 76;
const MIN_HEIGHT: u16 = 20;
const HELP_LINES: &[&str] = &[
    "Omatrack keys                                          ? or Esc closes",
    "Enter edit/browse   Space play/stop   Ctrl-S save   Ctrl-Z undo  Ctrl-Y redo",
    "Ctrl-R rewinds. Playing continues; stopped only moves. r types a note.",
    "Ctrl-Q quits anywhere. q quits from browse. Esc: block, then edit, then quit.",
    "Ctrl-F file: n new, o open, s save, a save as. New, open, and quit ask",
    "when the song is unsaved. An untitled save asks for a path. Ctrl-C copies.",
    "Arrows move. Tab changes pane. Ctrl-Left/Right edits the slot's pattern.",
    "F1 F2 octave 1-3    F3 F4 step 0-16    F5 cycles spectrum, scope, off",
    "Column meters scroll with the pattern. default_view is spectrum, scope, or off.",
    "Alt-1..4 mute a channel while editing. 1-4 mute in browse.",
    "",
    "Edit mode. Lower row is the octave, upper row is one octave higher.",
    "  Z S X D C V G B H N J M    C C# D D# E F F# G G# A A# B",
    "  Q 2 W 3 E R 5 T 6 Y 7 U    same notes, one octave higher",
    "Delete clears a cell or digit. Backspace steps up. Tab changes channel.",
    "Insert inserts a channel row. Ctrl-Backspace deletes it. Ctrl-Insert /",
    "Ctrl-Delete do that on every channel. Digits: sample decimal, effect hex.",
    "",
    "Block: Ctrl-B select, Ctrl-A all, Ctrl-C copy, Ctrl-X cut, Ctrl-V paste.",
    "Alt-Up/Down semitone, Alt-Left/Right octave. Alt-K channel, Alt-P pattern.",
    "Order: Up/Down pattern, Ins/Del, +/- length, N new. Ctrl-Right adds one.",
    "Ctrl-T title. Samples: R renames. XM/IT read-only. i import WAV, Ctrl-G render.",
    "v volume  f finetune  l loop  / toggle  t trim  n normalize  w reverse",
    "(R still renames)  a/z fade  c clear  y copy  p preview  u undo",
];

/// Draw the viewer into `frame`.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let theme = app.theme;
    let area = frame.area();
    frame.render_widget(Block::default().style(theme.fill()), area);

    let Some(regions) = layout(area, app) else {
        draw_too_small(frame, area, theme);
        return;
    };
    if matches!(app.overlay, Overlay::Help) {
        draw_help(frame, area, theme);
        return;
    }

    let inner_h = usize::from(regions.pattern.height.saturating_sub(2));
    let (row_window, _) = pattern_chrome(inner_h);
    let wave_lines = if app.focus == Focus::Samples { 2 } else { 0 };
    let sample_window =
        usize::from(regions.samples.height.saturating_sub(2)).saturating_sub(1 + wave_lines);
    app.reconcile_scroll(row_window, sample_window);

    draw_song(frame, regions.song, app, theme);
    draw_pattern(frame, regions.pattern, app, theme);
    if let Some(panel) = regions.viz {
        viz::draw_panel(frame, panel, app, theme);
    }
    if let Some(scope) = regions.scope {
        viz::draw_scope_view(frame, scope, app, theme);
    }
    draw_samples(frame, regions.samples, app, theme);
    frame.render_widget(
        Paragraph::new(clip(transport_text(app), regions.transport.width)).style(
            if app.audio_error.is_some() {
                paint(theme.error, theme.background, false)
            } else {
                theme.text()
            },
        ),
        regions.transport,
    );
    let status_style = if app.message_error {
        paint(theme.error, theme.background, false)
    } else {
        theme.text()
    };
    frame.render_widget(
        Paragraph::new(clip(status_text(app), regions.status.width)).style(status_style),
        regions.status,
    );
    frame.render_widget(
        Paragraph::new(hint_line(app, usize::from(regions.help.width))).style(theme.dim()),
        regions.help,
    );
    match &app.overlay {
        Overlay::Quit => draw_guard(frame, regions.status, regions.help, theme, quit_prompt()),
        Overlay::Guard(followup) => draw_guard(
            frame,
            regions.status,
            regions.help,
            theme,
            guard_prompt(followup),
        ),
        Overlay::File => draw_file_menu(frame, area, app, theme),
        Overlay::Text { target, buffer } => {
            frame.render_widget(
                Paragraph::new(clip(text_prompt(target, buffer), regions.status.width))
                    .style(theme.title()),
                regions.status,
            );
        }
        Overlay::None | Overlay::Help => {}
        Overlay::Path(prompt) => draw_path_prompt(frame, area, prompt, theme),
        Overlay::Import(prompt) => draw_import_prompt(frame, area, prompt, theme),
        Overlay::Field(prompt) => draw_field_prompt(frame, area, prompt, theme),
    }
}

fn draw_help(frame: &mut Frame, area: Rect, theme: Theme) {
    let lines: Vec<Line<'static>> = HELP_LINES
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let style = if index == 0 {
                theme.title()
            } else {
                theme.text()
            };
            styled((*line).to_string(), style)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).style(theme.fill()), area);
}

fn quit_prompt() -> &'static str {
    "Unsaved changes.  y save and quit    n discard    Esc cancel"
}

fn guard_prompt(followup: &Followup) -> &'static str {
    match followup {
        Followup::New => "Unsaved changes.  y save, then new    n discard    Esc cancel",
        Followup::Open => "Unsaved changes.  y save, then open    n discard    Esc cancel",
        Followup::Quit => quit_prompt(),
    }
}

fn draw_guard(frame: &mut Frame, status: Rect, help: Rect, theme: Theme, text: &str) {
    let bar = Rect {
        x: status.x,
        y: status.y,
        width: status.width,
        height: status.height.saturating_add(help.height),
    };
    frame.render_widget(
        Paragraph::new(text).style(paint(theme.cursor_fg, theme.cursor_bg, true)),
        bar,
    );
}

fn draw_file_menu(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let path = if app.path.as_os_str().is_empty() {
        "Untitled".to_string()
    } else {
        app.path.display().to_string()
    };
    let state = if app.is_dirty() {
        "unsaved changes"
    } else {
        "saved"
    };
    let lines = [
        format!("{path} - {state}"),
        "n  New module".to_string(),
        "o  Open...".to_string(),
        "s  Save".to_string(),
        "a  Save as...".to_string(),
        "Esc closes".to_string(),
    ];
    draw_dialog(frame, area, "File", &lines, None, theme);
}

fn text_prompt(target: &TextTarget, buffer: &str) -> String {
    let (label, max) = match target {
        TextTarget::Title => ("Title".to_string(), TITLE_LEN),
        TextTarget::Sample(index) => (format!("Sample {:02}", index + 1), SAMPLE_NAME_LEN),
    };
    format!(
        "{label} {}/{max}: {buffer}_   Enter saves, Esc cancels",
        buffer.chars().count()
    )
}

struct Regions {
    song: Rect,
    pattern: Rect,
    samples: Rect,
    viz: Option<Rect>,
    scope: Option<Rect>,
    transport: Rect,
    status: Rect,
    help: Rect,
}

fn layout(area: Rect, app: &App) -> Option<Regions> {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return None;
    }
    let [body, transport, status, help] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    let inner_w = usize::from(area.width.saturating_sub(2));
    let order_lines = desired_order_lines(app, inner_w);
    let header_h = header_height(body.height, order_lines);
    let sample_h = sample_height(body.height, header_h);
    let [song, pattern, band] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Fill(1),
        Constraint::Length(sample_h),
    ])
    .areas(body);
    let (samples, viz, scope) = split_band(band, app.viz_mode);

    Some(Regions {
        song,
        pattern,
        samples,
        viz,
        scope,
        transport,
        status,
        help,
    })
}

/// Samples on the left. Spectrum or scope takes the right half of that row.
///
/// `off` leaves the sample list the full width.
fn split_band(band: Rect, mode: VizMode) -> (Rect, Option<Rect>, Option<Rect>) {
    if mode == VizMode::Off {
        return (band, None, None);
    }
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(band);
    if mode == VizMode::Scope {
        (left, None, Some(right))
    } else {
        (left, Some(right), None)
    }
}

/// Note rows that fit in the pattern pane, and whether the meter strip takes a row.
///
/// One row is the pattern title, one is the column headers. The meter strip
/// takes one more when the pane is tall enough, so the notes give up a single row.
fn pattern_chrome(inner_h: usize) -> (usize, usize) {
    if inner_h > 2 {
        (inner_h - 3, 1)
    } else {
        (inner_h.saturating_sub(2), 0)
    }
}

fn header_height(body_h: u16, order_lines: u16) -> u16 {
    let desired = (4 + order_lines).clamp(5, 8);
    let cap = body_h.saturating_sub(9).max(5);
    desired.min(cap).min(body_h)
}

fn sample_height(body_h: u16, header_h: u16) -> u16 {
    let rest = body_h.saturating_sub(header_h);
    // Leave the pattern enough rows for a header and a handful of notes.
    let max_sample = rest.saturating_sub(8);
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
    let focused = app.focus == Focus::Order;
    let block = pane("Song", focused, theme);
    let inner_w = usize::from(area.width.saturating_sub(2));
    let inner_h = usize::from(area.height.saturating_sub(2));
    let mut lines = Vec::new();
    if inner_h > 0 {
        let title = plain(&if let Some(song) = &app.track {
            song.title.clone()
        } else {
            app.module.display_title()
        });
        let dirty = if app.is_dirty() { " *" } else { "" };
        let (text, style) = if title.is_empty() {
            (format!("(untitled){dirty}"), theme.dim())
        } else {
            (format!("{title}{dirty}"), theme.title())
        };
        lines.push(styled(text, style));
    }
    if inner_h > 1 {
        let info = if let Some(song) = &app.track {
            format!(
                "Length {}   Restart {}   Patterns {}   {} {}ch   Theme {}",
                song.order_len(),
                song.restart,
                song.patterns.len(),
                song.format.label(),
                song.channels,
                fit_chars(&app.theme_label, 16),
            )
        } else {
            format!(
                "Length {}   Restart {}   Patterns {}   {}   Theme {}",
                app.module.song_length,
                app.module.restart,
                app.module.patterns.len(),
                app.module.tag.as_str(),
                fit_chars(&app.theme_label, 24),
            )
        };
        lines.push(styled(fit_chars(&info, inner_w), theme.text()));
    }
    let room = inner_h.saturating_sub(lines.len());
    if room > 0 && inner_w > 0 {
        lines.extend(order_lines(app, theme, inner_w, room));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Digit columns of a pattern row number.
///
/// Two digits through index 99. One more digit each time the last index
/// crosses a power of ten, so a 114-row pattern and a 256-row XM pattern use
/// three, and an IT pattern of 1024 rows uses four. Every row of that pattern,
/// the channel header, and the meter strip use this same width.
fn row_gutter_digits(row_count: usize) -> usize {
    let last = row_count.saturating_sub(1).max(10);
    (last.ilog10() as usize) + 1
}

/// Columns taken by the row number, including the blank before channel 1.
fn row_gutter_width(row_count: usize) -> usize {
    row_gutter_digits(row_count) + 1
}

/// Row label padded so row 0 lines up with the last row.
///
/// Formatting each index on its own (the way [`fmt_num`] does) is what shifts
/// row 100 one column to the right of row 99.
fn row_gutter_text(row: usize, row_count: usize) -> String {
    let digits = row_gutter_digits(row_count);
    let text = format!("{row:0digits$} ");
    debug_assert_eq!(text.chars().count(), row_gutter_width(row_count));
    text
}

fn row_gutter_blank(row_count: usize) -> String {
    " ".repeat(row_gutter_width(row_count))
}

/// Columns of one track channel, including the gap before the next channel.
const TRACK_CELL: usize = 11;

/// Content column where visible channel `index` begins.
///
/// Column 0 is the first digit of the row number, inside the pane border.
/// Channel 0 starts just after the gutter. Each later channel starts
/// [`TRACK_CELL`] columns later (10 cell columns plus the gap).
fn track_cell_x(row_count: usize, index: usize) -> usize {
    row_gutter_width(row_count) + index * TRACK_CELL
}

/// How many track columns fit in `inner_w`.
///
/// The quotient is `(inner_w - gutter) / TRACK_CELL`, the same count a fixed
/// 3-column gutter produced. Patterns of 100 rows or fewer therefore keep the
/// column count they had. The gutter comes from [`track_cell_x`], so a wider
/// row number cannot leave horizontal scrolling one column ahead of the cells.
fn track_channels_fit(inner_w: usize, row_count: usize) -> usize {
    let fit = inner_w.saturating_sub(track_cell_x(row_count, 0)) / TRACK_CELL;
    debug_assert!(
        fit == 0 || track_channel_at_x(track_cell_x(row_count, 0), row_count, fit) == Some(0)
    );
    fit
}

/// Visible channel whose cell covers content column `x`.
///
/// `None` on the row-number gutter and on the blank between columns. The
/// index is relative to the first drawn channel; add the horizontal scroll
/// to get the song channel. The cursor cell starts at [`track_cell_x`].
fn track_channel_at_x(x: usize, row_count: usize, visible: usize) -> Option<usize> {
    let origin = track_cell_x(row_count, 0);
    if x < origin || visible == 0 {
        return None;
    }
    let offset = x - origin;
    let index = offset / TRACK_CELL;
    let column = offset % TRACK_CELL;
    if index < visible && column < TRACK_CELL - 1 {
        Some(index)
    } else {
        None
    }
}

fn draw_pattern(frame: &mut Frame, area: Rect, app: &mut App, theme: Theme) {
    if app.track.is_some() {
        draw_track_pattern(frame, area, app, theme);
        return;
    }
    let focused = app.focus == Focus::Pattern;
    let title = if app.editing { "EDIT" } else { "Pattern" };
    let block = pane(title, focused, theme);
    let inner_w = usize::from(area.width.saturating_sub(2));
    let inner_h = usize::from(area.height.saturating_sub(2));
    let (row_window, meter_rows) = pattern_chrome(inner_h);

    let mut lines = Vec::new();
    if inner_h > 0 {
        lines.push(styled(
            pattern_info(app),
            paint(theme.accent, theme.background, false),
        ));
    }
    if inner_h > 1 {
        lines.push(column_header_line(theme, app.row_count()));
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
    if meter_rows == 1 {
        seal_pattern_lines(&mut lines, inner_h, mod_meter_line(app, theme));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_track_pattern(frame: &mut Frame, area: Rect, app: &mut App, theme: Theme) {
    let focused = app.focus == Focus::Pattern;
    let block = pane("Pattern", focused, theme);
    let inner_w = usize::from(area.width.saturating_sub(2));
    let inner_h = usize::from(area.height.saturating_sub(2));
    let (row_window, meter_rows) = pattern_chrome(inner_h);
    let channels = app.channel_count();
    let row_count = app.row_count();
    let visible = track_channels_fit(inner_w, row_count).max(1).min(channels);
    app.reveal_channel(visible);
    let start_ch = app.channel_scroll;
    let end_ch = (start_ch + visible).min(channels);
    let mut lines = Vec::new();
    if inner_h > 0 {
        lines.push(styled(
            pattern_info(app),
            paint(theme.accent, theme.background, false),
        ));
    }
    if inner_h > 1 {
        lines.push(track_header_line(theme, start_ch, end_ch, row_count));
    }
    let row_start = app.row_offset;
    let row_end = (row_start + row_window).min(row_count);
    if let Some(song) = &app.track {
        for row in row_start..row_end {
            lines.push(track_row(song, row, start_ch, end_ch, app, theme, inner_w));
        }
    }
    if meter_rows == 1 {
        seal_pattern_lines(
            &mut lines,
            inner_h,
            track_meter_line(app, theme, start_ch, end_ch),
        );
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn seal_pattern_lines(lines: &mut Vec<Line<'static>>, inner_h: usize, meter: Line<'static>) {
    while lines.len() + 1 < inner_h {
        lines.push(Line::from(""));
    }
    if lines.len() < inner_h {
        lines.push(meter);
    }
}

fn mod_meter_line(app: &App, theme: Theme) -> Line<'static> {
    let mut spans = vec![Span::styled(row_gutter_blank(app.row_count()), theme.dim())];
    for channel in 0..CHANNELS {
        if channel > 0 {
            spans.push(Span::styled(" | ".to_string(), theme.dim()));
        }
        spans.extend(channel_meter_spans(app, channel, 10, theme));
    }
    Line::from(spans)
}

fn track_meter_line(app: &App, theme: Theme, start: usize, end: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(row_gutter_blank(app.row_count()), theme.dim())];
    for channel in start..end {
        if channel > start {
            spans.push(Span::styled(" ".to_string(), theme.dim()));
        }
        spans.extend(channel_meter_spans(app, channel, TRACK_CELL - 1, theme));
    }
    Line::from(spans)
}

fn channel_meter_spans(
    app: &App,
    channel: usize,
    width: usize,
    theme: Theme,
) -> Vec<Span<'static>> {
    let (level, _) = app.viz.meter(channel);
    let color = theme.channels[channel % theme.channels.len()];
    let (filled, rest) = viz::meter_bar(level, width);
    let mut spans = Vec::new();
    if !filled.is_empty() {
        spans.push(Span::styled(filled, paint(color, theme.background, false)));
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest, theme.dim()));
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), theme.fill()));
    }
    spans
}

fn track_header_line(theme: Theme, start: usize, end: usize, row_count: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(row_gutter_blank(row_count), theme.dim())];
    for channel in start..end {
        if channel > start {
            spans.push(Span::styled(" ".to_string(), theme.dim()));
        }
        let label = format!(
            "{:<width$}",
            format!("Ch{}", channel + 1),
            width = TRACK_CELL - 1
        );
        let color = theme.channels[channel % theme.channels.len()];
        spans.push(Span::styled(label, paint(color, theme.background, false)));
    }
    Line::from(spans)
}

fn track_row(
    song: &crate::Song,
    row: usize,
    start_ch: usize,
    end_ch: usize,
    app: &App,
    theme: Theme,
    inner_w: usize,
) -> Line<'static> {
    let on_row = row == app.row;
    let line_bg = if on_row && app.playing {
        theme.play_bg
    } else if on_row {
        theme.row_bg
    } else {
        theme.background
    };
    let row_count = app.row_count();
    let gutter = row_gutter_text(row, row_count);
    debug_assert_eq!(gutter.chars().count(), track_cell_x(row_count, 0));
    let mut spans = vec![Span::styled(gutter, paint(theme.accent, line_bg, false))];
    let pattern = song.patterns.get(app.view_pattern);
    let mut width = track_cell_x(row_count, 0);
    for (visible_index, channel) in (start_ch..end_ch).enumerate() {
        if visible_index > 0 {
            spans.push(Span::styled(
                " ".to_string(),
                paint(theme.dim, line_bg, false),
            ));
            width += 1;
        }
        debug_assert_eq!(width, track_cell_x(row_count, visible_index));
        let cell = pattern
            .and_then(|pattern| pattern.rows.get(row))
            .and_then(|row| row.get(channel))
            .copied()
            .unwrap_or_else(crate::track::Cell::empty);
        let active = on_row && channel == app.channel && app.focus == Focus::Pattern;
        let text = track_cell_text(song, cell);
        let padded = format!("{:<width$}", text, width = TRACK_CELL - 1);
        let (fg, bg, bold) = if active {
            (theme.cursor_fg, theme.cursor_bg, true)
        } else if cell.note == 0 {
            (theme.dim, line_bg, false)
        } else {
            (theme.note, line_bg, false)
        };
        spans.push(Span::styled(padded, paint(fg, bg, bold)));
        width += TRACK_CELL - 1;
    }
    let pad = inner_w.saturating_sub(width);
    if on_row && pad > 0 {
        spans.push(Span::styled(
            " ".repeat(pad),
            paint(theme.text, line_bg, false),
        ));
    }
    Line::from(spans)
}

fn track_cell_text(song: &crate::Song, cell: crate::track::Cell) -> String {
    let instrument = if cell.instrument == 0 {
        "--".to_string()
    } else if cell.instrument > 99 {
        format!("{:02X}", cell.instrument)
    } else {
        format!("{:02}", cell.instrument)
    };
    let volume = if cell.has_volume {
        format!("{:02X}", cell.volume)
    } else {
        "--".to_string()
    };
    format!(
        "{}{instrument}{volume}{}",
        crate::track::format_note(cell.note),
        crate::track::format_effect(song.format, cell.effect, cell.param)
    )
}

fn draw_samples(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    if app.track.is_some() {
        draw_track_samples(frame, area, app, theme);
        return;
    }
    let focused = app.focus == Focus::Samples;
    let block = pane("Samples", focused, theme);
    let inner_w = usize::from(area.width.saturating_sub(2));
    let inner_h = usize::from(area.height.saturating_sub(2));
    let mut lines = Vec::new();
    if focused && inner_h > 2 && inner_w > 0 {
        let sample = &app.module.samples[app.sample];
        lines.push(styled(
            waveform_row(&sample.data, inner_w),
            paint(theme.waveform, theme.background, false),
        ));
        let mut points = Vec::new();
        if sample.loops() {
            let start = usize::from(sample.loop_start) * 2;
            let end = start.saturating_add(usize::from(sample.loop_length) * 2);
            points.push(start);
            points.push(end.min(sample.data.len()));
        }
        lines.push(styled(
            marker_row(sample.data.len(), inner_w, &points),
            paint(theme.effect, theme.background, false),
        ));
    }
    let sample_window = inner_h.saturating_sub(lines.len() + 1);

    let name_cols = mod_name_cols(inner_w);
    if inner_h > lines.len() {
        lines.push(styled(sample_header(name_cols), theme.dim()));
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
        let mut text = sample_row(&app.module.samples[index], index + 1, name_cols);
        let pad = inner_w.saturating_sub(text.chars().count());
        if selected && pad > 0 {
            text.push_str(&" ".repeat(pad));
        }
        lines.push(styled(text, style));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_track_samples(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let focused = app.focus == Focus::Samples;
    let block = pane("Samples", focused, theme);
    let inner_w = usize::from(area.width.saturating_sub(2));
    let inner_h = usize::from(area.height.saturating_sub(2));
    let Some(song) = &app.track else {
        return;
    };
    let mut lines = Vec::new();
    let name_cols = track_name_cols(inner_w);
    if inner_h > 0 {
        lines.push(styled(
            format!(
                "{:>3} {:<name_cols$} {:>7} {:>3} {}",
                "#",
                "Name",
                "Frames",
                "Vol",
                "Loop",
                name_cols = name_cols,
            ),
            theme.dim(),
        ));
    }
    let window = inner_h.saturating_sub(lines.len());
    let total = song.samples.len();
    let start = app.sample_offset.min(total);
    let end = (start + window).min(total);
    for index in start..end {
        let sample = &song.samples[index];
        let selected = index == app.sample;
        let style = if selected && focused {
            paint(theme.cursor_fg, theme.cursor_bg, true)
        } else if selected {
            paint(theme.text, theme.row_bg, false)
        } else {
            theme.text()
        };
        let looped = match sample.loop_kind {
            crate::track::LoopKind::Forward => "fwd",
            crate::track::LoopKind::PingPong => "pp",
            crate::track::LoopKind::None => "-",
        };
        let mut text = format!(
            "{:03} {:<name_cols$} {:7} {:3} {looped}",
            index + 1,
            fit_chars(&sample.name, name_cols),
            sample.pcm.len(),
            sample.volume,
            name_cols = name_cols,
        );
        let pad = inner_w.saturating_sub(text.chars().count());
        if selected && pad > 0 {
            text.push_str(&" ".repeat(pad));
        }
        lines.push(styled(text, style));
    }
    if total == 0 && inner_h > lines.len() {
        lines.push(styled("No samples.".to_string(), theme.dim()));
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
    let header = format!(
        "Pat {}{note}  Pos {} of {}  Row {:02}  Channel {}",
        fmt_num(app.view_pattern),
        app.order_pos,
        app.song_len(),
        app.row,
        app.channel + 1,
    );
    if app.editing {
        format!(
            "{header}  {}  Oct {}  Step {}",
            app.field.label(),
            app.octave,
            app.step
        )
    } else {
        header
    }
}

fn fmt_num(value: usize) -> String {
    if value >= 100 {
        format!("{value:03}")
    } else {
        format!("{value:02}")
    }
}

fn column_header_line(theme: Theme, row_count: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(row_gutter_blank(row_count), theme.dim())];
    for channel in 0..CHANNELS {
        if channel > 0 {
            spans.push(Span::styled(" | ".to_string(), theme.dim()));
        }
        let label = format!("{:<10}", format!("Ch {}", channel + 1));
        spans.push(Span::styled(
            label,
            paint(theme.channels[channel], theme.background, false),
        ));
    }
    Line::from(spans)
}

fn pattern_row(
    pattern: &crate::module::Pattern,
    row: usize,
    app: &App,
    theme: Theme,
    inner_w: usize,
) -> Line<'static> {
    let on_row = row == app.row;
    let line_bg = if on_row && app.playing {
        theme.play_bg
    } else if on_row {
        theme.row_bg
    } else {
        theme.background
    };
    let row_count = app.row_count();
    let gutter = row_gutter_text(row, row_count);
    let mut spans = vec![Span::styled(gutter, paint(theme.accent, line_bg, false))];
    let mut width = row_gutter_width(row_count);
    for channel in 0..CHANNELS {
        if channel > 0 {
            spans.push(Span::styled(" | ", paint(theme.dim, line_bg, false)));
            width += 3;
        }
        let cell = pattern.rows[row][channel];
        let selected = app.cell_selected(row, channel);
        let cell_bg = if selected { theme.block_bg } else { line_bg };
        let active = on_row && channel == app.channel && app.focus == Focus::Pattern;
        push_cell(&mut spans, cell, app, theme, active, cell_bg);
        width += 10;
    }
    let pad = inner_w.saturating_sub(width);
    if on_row && pad > 0 {
        spans.push(Span::styled(
            " ".repeat(pad),
            paint(theme.text, line_bg, false),
        ));
    }
    Line::from(spans)
}

fn push_cell(
    spans: &mut Vec<Span<'static>>,
    cell: Cell,
    app: &App,
    theme: Theme,
    active: bool,
    bg: ratatui::style::Color,
) {
    let glyphs = field_glyphs(cell);
    let pieces: [(Option<Field>, &str); 8] = [
        (Some(Field::Note), glyphs[0].as_str()),
        (None, " "),
        (Some(Field::SampleHigh), glyphs[1].as_str()),
        (Some(Field::SampleLow), glyphs[2].as_str()),
        (None, " "),
        (Some(Field::Effect), glyphs[3].as_str()),
        (Some(Field::ParamHigh), glyphs[4].as_str()),
        (Some(Field::ParamLow), glyphs[5].as_str()),
    ];
    for (field, text) in pieces {
        let hot = if app.editing {
            active && field == Some(app.field)
        } else {
            active
        };
        let (fg, paint_bg, bold) = if hot {
            if app.editing {
                (theme.edit_fg, theme.edit_bg, true)
            } else {
                (theme.cursor_fg, theme.cursor_bg, true)
            }
        } else {
            (glyph_color(field, cell, theme), bg, false)
        };
        spans.push(Span::styled(text.to_string(), paint(fg, paint_bg, bold)));
    }
}

fn field_glyphs(cell: Cell) -> [String; 6] {
    let sample: Vec<char> = sample_field(cell.sample).chars().collect();
    let effect: Vec<char> = effect_field(cell.effect, cell.param).chars().collect();
    [
        format_period(cell.period),
        sample.first().copied().unwrap_or(' ').to_string(),
        sample.get(1).copied().unwrap_or(' ').to_string(),
        effect.first().copied().unwrap_or(' ').to_string(),
        effect.get(1).copied().unwrap_or(' ').to_string(),
        effect.get(2).copied().unwrap_or(' ').to_string(),
    ]
}

fn glyph_color(field: Option<Field>, cell: Cell, theme: Theme) -> ratatui::style::Color {
    match field {
        Some(Field::Note) if cell.period == 0 => theme.dim,
        Some(Field::Note) => theme.note,
        Some(Field::SampleHigh | Field::SampleLow) if cell.sample == 0 => theme.dim,
        Some(Field::SampleHigh | Field::SampleLow) => theme.text,
        Some(_) if cell.effect == 0 && cell.param == 0 => theme.dim,
        Some(_) => theme.effect,
        None => theme.dim,
    }
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

/// Name column that still leaves room for bytes, volume, finetune, and a loop.
fn mod_name_cols(inner_w: usize) -> usize {
    // "NN " + name + " " + bytes(6) + " " + vol(3) + " " + fn(2) + " " + loop(8)
    let fixed = 2 + 1 + 1 + 6 + 1 + 3 + 1 + 2 + 1 + 8;
    inner_w.saturating_sub(fixed).clamp(4, 22)
}

/// Name column that still leaves room for frames, volume, and "Loop".
fn track_name_cols(inner_w: usize) -> usize {
    // "NNN " + name + " " + frames(7) + " " + vol(3) + " " + loop(4)
    let fixed = 3 + 1 + 1 + 7 + 1 + 3 + 1 + 4;
    inner_w.saturating_sub(fixed).clamp(4, 22)
}

fn sample_header(name_cols: usize) -> String {
    format!(
        "{:>2} {:<name_cols$} {:>6} {:>3} {:>2} {}",
        "#",
        "Name",
        "Bytes",
        "Vol",
        "Fn",
        "Loop",
        name_cols = name_cols,
    )
}

fn sample_row(sample: &Sample, number: usize, name_cols: usize) -> String {
    format!(
        "{number:02} {:<name_cols$} {:6} {:3} {:>2} {}",
        fit_chars(&sample.display_name(), name_cols),
        sample.byte_length(),
        sample.volume,
        format_finetune(sample.finetune_raw),
        format_loop(sample),
        name_cols = name_cols,
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

fn plain(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_control()).collect()
}

fn fit_chars(text: &str, width: usize) -> String {
    let mut out: String = plain(text).chars().take(width).collect();
    let count = out.chars().count();
    if count < width {
        out.push_str(&" ".repeat(width - count));
    }
    out
}

fn transport_text(app: &App) -> String {
    if let Some(error) = &app.audio_error {
        return error.clone();
    }
    let state = if app.playing { "Play" } else { "Stop" };
    let mode = if app.editing { "EDIT" } else { "VIEW" };
    let dirty = if app.is_dirty() { "*" } else { "" };
    let mut channels = String::new();
    if app.channel_count() > 4 {
        channels = format!("{}ch", app.channel_count());
    } else {
        for (index, muted) in app.muted.iter().enumerate() {
            if index > 0 {
                channels.push(' ');
            }
            let mark = if *muted { "off" } else { "on" };
            channels.push_str(&format!("{}:{mark}", index + 1));
        }
    }
    let viz = match app.viz_mode {
        VizMode::Off => "",
        VizMode::Panel => "  Viz",
        VizMode::Scope => "  Scope",
    };
    format!(
        "{mode}{dirty} {state}  Ord {:02}/{:02}  Row {:02}  Spd {:02}  Tmp {:03}  {channels}{viz}",
        app.order_pos,
        app.song_len(),
        app.row,
        app.speed,
        app.tempo,
    )
}

/// Shortcut hints for the focused pane. Each list fits a 76-column row.
fn status_hints(focus: Focus, editing: bool) -> &'static [&'static str] {
    match (focus, editing) {
        (Focus::Pattern, true) => &[
            "Ctrl-Left/Right pat",
            "Z-M notes",
            "Del clear",
            "Ctrl-Z",
            "Space",
            "Ctrl-R",
            "Esc",
            "?",
        ],
        (Focus::Pattern, false) => &[
            "Ctrl-Left/Right pat",
            "[ ] order",
            "Space play",
            "Ctrl-R",
            "Enter",
            "Tab focus",
            "F5",
            "?",
        ],
        (Focus::Samples, _) => &[
            "R rename",
            "i import",
            "o export",
            "v volume",
            "p preview",
            "Space",
            "Tab",
            "?",
        ],
        (Focus::Order, _) => &[
            "Up/Down pattern",
            "Ins/Del entry",
            "+/- length",
            "N new",
            "Space",
            "Ctrl-R",
            "Tab",
            "?",
        ],
    }
}

/// One status-bar hint row. Whole hints are dropped when `width` runs out,
/// so a narrow terminal never slices a binding in half or wraps the row.
fn hint_line(app: &App, width: usize) -> String {
    fit_hints(status_hints(app.focus, app.editing), width)
}

fn fit_hints(hints: &[&str], width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut chosen: Vec<&str> = Vec::new();
    let mut used = 0usize;
    for hint in hints {
        if hint.is_empty() {
            continue;
        }
        let len = hint.chars().count();
        let need = if chosen.is_empty() { len } else { len + 2 };
        if used + need > width {
            break;
        }
        chosen.push(*hint);
        used += need;
    }
    if chosen.is_empty() {
        let first = hints
            .iter()
            .find(|hint| !hint.is_empty())
            .copied()
            .unwrap_or("");
        return clip(first.to_string(), u16::try_from(width).unwrap_or(u16::MAX));
    }
    chosen.join("  ")
}

fn status_text(app: &App) -> String {
    if let Some(message) = &app.message {
        if !message.is_empty() {
            return message.clone();
        }
    }
    if let Some(notice) = &app.notice {
        return notice.clone();
    }
    if let Some(song) = &app.track {
        let cell = song
            .cell(app.view_pattern, app.row, app.channel)
            .unwrap_or_else(crate::track::Cell::empty);
        let volume = if cell.has_volume {
            format!("{:02X}", cell.volume)
        } else {
            "--".to_string()
        };
        return format!(
            "{}  inst {:02}  {volume}  {}  read-only",
            crate::track::format_note(cell.note),
            cell.instrument,
            crate::track::format_effect(song.format, cell.effect, cell.param),
        );
    }
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
        let value = if let Some(song) = &app.track {
            song.orders.get(index).copied().unwrap_or(0)
        } else {
            app.module.order[index]
        };
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
    let max = if let Some(song) = &app.track {
        song.orders
            .iter()
            .take(song_len)
            .copied()
            .max()
            .unwrap_or(0)
    } else {
        app.module.order[..song_len]
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
    };
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

fn draw_path_prompt(frame: &mut Frame, area: Rect, prompt: &PathPrompt, theme: Theme) {
    let title = match prompt.kind {
        PathKind::ImportWav => "Import WAV",
        PathKind::ExportSample => "Export sample",
        PathKind::ExportSong => "Render song",
        PathKind::OpenModule => "Open module",
        PathKind::SaveModule => "Save module",
    };
    let mut lines = Vec::new();
    let list_rows = 8usize;
    let start = window_start_simple(prompt.selected, prompt.entries.len(), list_rows);
    if prompt.entries.is_empty() {
        lines.push("(empty directory)".to_string());
    } else {
        for (offset, row) in prompt
            .entries
            .iter()
            .enumerate()
            .skip(start)
            .take(list_rows)
        {
            let mark = if offset == prompt.selected { ">" } else { " " };
            let slash = if row.is_dir { "/" } else { "" };
            lines.push(format!("{mark} {}{slash}", row.name));
        }
    }
    while lines.len() < list_rows {
        lines.push(String::new());
    }
    lines.push(format!("Path: {}", prompt.buffer));
    if prompt.kind == PathKind::ExportSample {
        let editing = if prompt.rate_focus { " (editing)" } else { "" };
        let rate = if prompt.rate.is_empty() {
            "C-2"
        } else {
            prompt.rate.as_str()
        };
        lines.push(format!(
            "Rate: {rate} Hz{editing}   blank = C-2, Tab switches"
        ));
    }
    lines.push("Enter opens or confirms. Esc cancels.".to_string());
    draw_dialog(frame, area, title, &lines, prompt.error.as_deref(), theme);
}

fn draw_import_prompt(frame: &mut Frame, area: Rect, prompt: &ImportPrompt, theme: Theme) {
    let nibble = if prompt.finetune < 0 {
        u8::try_from(prompt.finetune + 16).unwrap_or(0)
    } else {
        u8::try_from(prompt.finetune).unwrap_or(0)
    };
    let note_rate = rate_for_note(prompt.note, nibble);
    let rate_line = if let Some(text) = &prompt.rate_text {
        format!("Rate        {text} Hz custom (r, digits; backspace clears)")
    } else {
        format!("Rate        {note_rate} Hz from the note (r to type a rate)")
    };
    let lines = [
        format!("File        {}", prompt.path.display()),
        format!(
            "Base note   {}    left/right",
            format_period(crate::notes::period_at(prompt.note))
        ),
        format!("Finetune    {:+}     up/down", prompt.finetune),
        format!(
            "Normalize   {}     n",
            if prompt.normalize { "on" } else { "off" }
        ),
        format!(
            "Dither      {}     d",
            if prompt.dither { "on" } else { "off" }
        ),
        rate_line,
        "Enter imports into the selected slot. Esc cancels.".to_string(),
    ];
    draw_dialog(
        frame,
        area,
        "Import options",
        &lines,
        prompt.error.as_deref(),
        theme,
    );
}

fn draw_field_prompt(frame: &mut Frame, area: Rect, prompt: &FieldPrompt, theme: Theme) {
    let (title, hint) = match prompt.kind {
        FieldKind::Volume => ("Volume", "0..=64"),
        FieldKind::Finetune => ("Finetune", "-8..=7"),
        FieldKind::Loop => ("Loop", "start length, in bytes; length 0 turns it off"),
        FieldKind::Trim => ("Trim", "start end, end exclusive, even bytes"),
        FieldKind::CopyTo => ("Copy sample", "destination slot 1..=31"),
    };
    let lines = [
        hint.to_string(),
        format!("> {}", prompt.buffer),
        "Enter applies. Esc cancels.".to_string(),
    ];
    draw_dialog(frame, area, title, &lines, prompt.error.as_deref(), theme);
}

fn draw_dialog(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: &[impl AsRef<str>],
    error: Option<&str>,
    theme: Theme,
) {
    let mut body: Vec<Line<'static>> = lines
        .iter()
        .map(|line| styled(line.as_ref().to_string(), theme.text()))
        .collect();
    if let Some(error) = error {
        body.push(styled(
            error.to_string(),
            paint(theme.error, theme.background, false),
        ));
    }
    let height = u16::try_from(body.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(1))
        .max(3);
    // Full width, so a pattern row number cannot peek out beside the border.
    let rect = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: area.width,
        height,
    };
    frame.render_widget(ratatui::widgets::Clear, rect);
    let block = Block::bordered()
        .title(Span::styled(format!(" {title} "), theme.title()))
        .border_style(paint(theme.border_focus, theme.background, false))
        .style(theme.fill());
    frame.render_widget(Paragraph::new(body).block(block), rect);
}

fn window_start_simple(selected: usize, len: usize, window: usize) -> usize {
    if window == 0 || len <= window {
        return 0;
    }
    selected.saturating_sub(window / 2).min(len - window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::Field;
    use crate::module::{Cell, Module, Sample, Tag};
    use crate::tui::app::{command_for, App, Command, Focus, Key, Overlay};
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
        assert_has(&screen, "Ctrl-Left/Right pat");
        assert_has(&screen, "Stop");
        assert_has(&screen, "Spd 06");
        assert_has(&screen, "Tmp 125");
        assert_has(&screen, "1:on");
        assert_has(&screen, "* Pattern");
        assert!(!screen.contains("too small"), "{screen}");

        let (x, y) = find_sequence(&buf, &["C", "-", "1"]).expect("note");
        assert_eq!(buf[(x, y)].bg, Color::Yellow);
        assert_ne!(buf[(x - 1, y)].bg, Color::Rgb(16, 92, 48));
    }

    #[test]
    fn playback_paints_the_current_row_and_the_transport() {
        let mut app = demo();
        app.playing = true;
        app.speed = 6;
        app.tempo = 125;
        app.muted[1] = true;
        app.follow(1, 0, 6, 125);
        let buf = render(&mut app, 100, 40);
        let screen = text_of(&buf);
        assert_has(&screen, "Play");
        assert_has(&screen, "Ord 01/02");
        assert_has(&screen, "2:off");
        assert_has(&screen, "Pat 01");
        let (x, y) = find_sequence(&buf, &["C", "-", "2"]).expect("playing note");
        assert_eq!(buf[(x - 1, y)].bg, Color::Rgb(16, 92, 48));
        assert_eq!(buf[(x, y)].bg, Color::Yellow);
    }

    #[test]
    fn keys_move_the_cursor_across_the_pattern_and_samples() {
        let mut app = demo();
        let down = command_for(&app, Key::Down).unwrap();
        app.apply(down);
        let screen = text_of(&render(&mut app, 100, 40));
        assert_has(&screen, "Row 01");
        assert!(!screen.contains("Row 00"), "{screen}");

        app.apply(command_for(&app, Key::Right).unwrap());
        let screen = text_of(&render(&mut app, 100, 40));
        assert_has(&screen, "Channel 2");
        assert!(!screen.contains("Channel 1"), "{screen}");

        app.apply(command_for(&app, Key::Char('.')).unwrap());
        let screen = text_of(&render(&mut app, 100, 40));
        assert_has(&screen, "Pat 01 (order has pat 00)");
        assert_has(&screen, "Pos 0 of 2");
        assert_has(&screen, "C-2");

        app.apply(command_for(&app, Key::Char(']')).unwrap());
        let screen = text_of(&render(&mut app, 100, 40));
        assert_has(&screen, "Pos 1 of 2");
        assert!(!screen.contains("order has pat"), "{screen}");

        app.apply(Command::FirstRow);
        app.apply(command_for(&app, Key::Tab).unwrap());
        let buf = render(&mut app, 100, 40);
        let screen = text_of(&buf);
        assert_has(&screen, "* Samples");
        assert!(!screen.contains("* Pattern"), "{screen}");
        let (x, y) = find_sequence(&buf, &["k", "i", "c", "k"]).expect("kick");
        assert_eq!(buf[(x, y)].bg, Color::Yellow);

        app.apply(command_for(&app, Key::Down).unwrap());
        let buf = render(&mut app, 100, 40);
        let (x, y) = find_sequence(&buf, &["s", "n", "a", "r", "e"]).expect("snare");
        assert_eq!(buf[(x, y)].bg, Color::Yellow);
        assert_has(&text_of(&buf), "-2");
        assert_has(&text_of(&buf), "2+8");
    }

    #[test]
    fn song_pane_border_follows_focus() {
        let omarchy = Theme::from_palette(
            &crate::omarchy::palette_from_colors_toml(
                r##"
background = "#1a1b26"
foreground = "#a9b1d6"
accent = "#7aa2f7"
muted = "#414868"
bright_foreground = "#c0caf5"
red = "#f7768e"
green = "#9ece6a"
yellow = "#e0af68"
blue = "#7aa2f7"
cyan = "#449dab"
magenta = "#ad8ee6"
lighter_background = "#24283b"
"##,
            )
            .expect("palette"),
        );
        for theme in [
            Theme::protracker(),
            Theme::phosphor(),
            Theme::terminal(),
            omarchy,
        ] {
            assert_ne!(
                theme.border, theme.border_focus,
                "{} border and focus border are the same color",
                theme.id
            );
            let mut app = demo();
            app.set_theme(theme, theme.id);
            assert_eq!(app.focus, Focus::Pattern);

            let pattern = render(&mut app, 100, 40);
            let pattern_screen = text_of(&pattern);
            assert!(!pattern_screen.contains("* Song"), "{pattern_screen}");
            assert_has(&pattern_screen, "* Pattern");
            assert_song_border(&pattern, theme.border, theme.id);
            assert_eq!(pane_border_fg(&pattern, "Pattern"), theme.border_focus);
            assert_eq!(pane_border_fg(&pattern, "Samples"), theme.border);
            assert_eq!(pane_border_fg(&pattern, "Spectrum"), theme.border);

            app.apply(command_for(&app, Key::Tab).unwrap());
            assert_eq!(app.focus, Focus::Samples);
            let samples = render(&mut app, 100, 40);
            let samples_screen = text_of(&samples);
            assert!(!samples_screen.contains("* Song"), "{samples_screen}");
            assert_has(&samples_screen, "* Samples");
            assert!(!samples_screen.contains("* Pattern"), "{samples_screen}");
            assert_song_border(&samples, theme.border, theme.id);
            assert_eq!(pane_border_fg(&samples, "Pattern"), theme.border);
            assert_eq!(pane_border_fg(&samples, "Samples"), theme.border_focus);
            assert_eq!(pane_border_fg(&samples, "Spectrum"), theme.border);

            app.apply(command_for(&app, Key::Tab).unwrap());
            assert_eq!(app.focus, Focus::Order);
            let song = render(&mut app, 100, 40);
            let song_screen = text_of(&song);
            assert_has(&song_screen, "* Song");
            assert!(!song_screen.contains("* Pattern"), "{song_screen}");
            assert!(!song_screen.contains("* Samples"), "{song_screen}");
            assert_song_border(&song, theme.border_focus, theme.id);
            assert_ne!(
                pane_border_fg(&song, "Song"),
                pane_border_fg(&pattern, "Song"),
                "{} song border did not change when focused",
                theme.id
            );
            assert_eq!(pane_border_fg(&song, "Pattern"), theme.border);
            assert_eq!(pane_border_fg(&song, "Samples"), theme.border);
            // Spectrum and scope are not focus stops. They stay on the plain border.
            assert_eq!(pane_border_fg(&song, "Spectrum"), theme.border);

            app.viz_mode = VizMode::Scope;
            let scope = render(&mut app, 100, 40);
            assert_has(&text_of(&scope), "* Song");
            assert_song_border(&scope, theme.border_focus, theme.id);
            assert_eq!(pane_border_fg(&scope, "Scope"), theme.border);
            assert!(!text_of(&scope).contains("* Scope"), "{}", text_of(&scope));
        }
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
        let header = sample_header(22);
        let row = sample_row(&sample, 1, 22);
        assert_eq!(header.find("Name"), row.find("kick"));
        assert!(row.contains("-8"), "{row}");
        assert!(row.contains("    2"), "{row}");
    }

    #[test]
    fn edit_mode_highlights_one_field_and_help_lists_the_chords() {
        let mut app = demo();
        app.editing = true;
        app.field = Field::Note;
        let buf = render(&mut app, 100, 40);
        let screen = text_of(&buf);
        assert_has(&screen, "* EDIT");
        assert_has(&screen, "EDIT");
        assert_has(&screen, "Oct 2");
        assert_has(&screen, "Step 1");
        let (x, y) = find_sequence(&buf, &["C", "-", "1"]).expect("note");
        assert_eq!(buf[(x, y)].bg, Color::White);
        assert_eq!(buf[(x + 7, y)].symbol(), "C");
        assert_ne!(buf[(x + 7, y)].bg, Color::White);

        app.field = Field::Effect;
        let buf = render(&mut app, 100, 40);
        let (x, y) = find_sequence(&buf, &["C", "-", "1"]).expect("note");
        assert_ne!(buf[(x, y)].bg, Color::White);
        assert_eq!(buf[(x + 7, y)].bg, Color::White);

        app.editing = false;
        app.overlay = Overlay::Help;
        let screen = text_of(&render(&mut app, 80, 24));
        assert_has(&screen, "Ctrl-S save");
        assert_has(&screen, "Ctrl-R rewinds");
        assert_has(&screen, "stopped only moves");
        assert_has(&screen, "r types a note");
        assert_has(&screen, "Ctrl-Left/Right edits the slot's pattern");
        assert_has(&screen, "Ctrl-Right adds one");
        assert_has(&screen, "Tab changes channel");
        assert_has(&screen, "Ctrl-Z undo");
        assert_has(&screen, "Z S X D C V G B H N J M");
        assert_has(&screen, "unsaved");
        assert_has(&screen, "import WAV");
        assert_has(&screen, "R still renames");
        assert_has(&screen, "F5 cycles spectrum, scope, off");
        assert_has(&screen, "Column meters scroll with the pattern");
        assert_has(&screen, "default_view");

        let mut app = demo();
        app.apply(Command::EnterNote(0));
        let screen = text_of(&render(&mut app, 80, 24));
        assert_has(&screen, "Demo Tune *");
        assert_has(&screen, "VIEW*");
        assert_has(&screen, "Ctrl-R");
    }

    #[test]
    fn status_hints_follow_focus_and_truncate() {
        let mut app = demo();
        let pattern = render(&mut app, 80, 24);
        let hint = row_text(&pattern, 23);
        assert!(hint.contains("Ctrl-Left/Right pat"), "{hint}");
        assert!(hint.contains("[ ] order"), "{hint}");
        let status = row_text(&pattern, 22);
        assert!(
            status.contains("C-1") || status.contains("period"),
            "{status}"
        );

        app.focus = Focus::Samples;
        let samples = render(&mut app, 80, 24);
        let hint = row_text(&samples, 23);
        assert!(hint.contains("R rename"), "{hint}");
        assert!(!hint.contains("Ctrl-Left"), "{hint}");

        app.focus = Focus::Order;
        let song = render(&mut app, 80, 24);
        let hint = row_text(&song, 23);
        assert!(hint.contains("Up/Down pattern"), "{hint}");
        assert!(hint.contains("N new"), "{hint}");
        assert!(!hint.contains("Ctrl-Left"), "{hint}");

        app.focus = Focus::Pattern;
        app.editing = true;
        let editing = render(&mut app, 80, 24);
        let hint = row_text(&editing, 23);
        assert!(hint.contains("Z-M notes"), "{hint}");
        assert!(!hint.contains("[ ] order"), "{hint}");

        app.editing = false;
        app.set_message("Saved demo.mod");
        let with_message = render(&mut app, 80, 24);
        let status = row_text(&with_message, 22);
        let hint = row_text(&with_message, 23);
        assert!(status.contains("Saved demo.mod"), "{status}");
        assert!(hint.contains("Ctrl-Left/Right pat"), "{hint}");
        assert!(!status.contains("Ctrl-Left"), "{status}");

        let long = "status message that is definitely wider than a narrow tracker row and must not wrap into the hint line or the pattern";
        app.set_message(long);
        let narrow = render(&mut app, 76, 20);
        let screen = text_of(&narrow);
        assert!(!screen.contains("too small"), "{screen}");
        let status = row_text(&narrow, 18);
        let hint = row_text(&narrow, 19);
        assert_eq!(status.chars().count(), 76, "{status}");
        assert_eq!(hint.chars().count(), 76, "{hint}");
        assert!(status.starts_with("status message"), "{status}");
        assert!(!status.contains("Ctrl-Left"), "{status}");
        assert!(hint.contains("Ctrl-Left/Right pat"), "{hint}");

        assert_eq!(
            fit_hints(status_hints(Focus::Pattern, false), 28),
            "Ctrl-Left/Right pat"
        );
        assert_eq!(
            fit_hints(status_hints(Focus::Pattern, false), 30),
            "Ctrl-Left/Right pat  [ ] order"
        );
        assert_eq!(
            fit_hints(status_hints(Focus::Pattern, false), 10),
            "Ctrl-Left/"
        );
        assert_eq!(fit_hints(status_hints(Focus::Pattern, false), 0), "");
        for (focus, editing) in [
            (Focus::Pattern, false),
            (Focus::Pattern, true),
            (Focus::Samples, false),
            (Focus::Order, false),
        ] {
            let full = fit_hints(status_hints(focus, editing), 76);
            assert!(
                full.chars().count() <= 76,
                "{focus:?} editing={editing} is {} columns: {full}",
                full.chars().count()
            );
            assert_eq!(full, status_hints(focus, editing).join("  "));
        }
    }

    #[test]
    fn the_spectrum_shares_the_sample_row_and_the_scope_stays_beside_it() {
        let mut app = demo();
        assert_eq!(app.viz_mode(), VizMode::Panel);
        app.message = None;
        peg_channels(&mut app, &[8_192, 0, 2_000, 0]);
        for &(width, height) in &[(80u16, 24u16), (80, 30), (100, 40), (160, 50)] {
            let buf = render(&mut app, width, height);
            let screen = text_of(&buf);
            assert_has(&screen, "Demo Tune");
            assert_has(&screen, "C-1");
            assert!(!screen.contains("too small"), "{width}x{height}\n{screen}");
            assert_has(&screen, "Viz");
            assert_side_by_side(&buf, "Samples", "Spectrum");
            assert_closed_boxes(&buf);
            assert_meter_under_channel(&buf, "Ch 1", '█', Color::Rgb(255, 214, 102));
            assert_meter_under_channel(&buf, "Ch 2", '▁', Color::Rgb(140, 150, 180));
            assert_spectrum_is_bars_only(&buf);
            assert_eq!(
                meter_rows_above_border(&buf),
                1,
                "{width}x{height}\n{screen}"
            );
        }

        app.viz_mode = VizMode::Off;
        let off = render(&mut app, 100, 40);
        let off_text = text_of(&off);
        assert!(!off_text.contains("Spectrum"), "{off_text}");
        assert!(!off_text.contains("Scope"), "{off_text}");
        assert_has(&off_text, "kickdrum");
        assert_full_width_samples(&off);
        assert_meter_under_channel(&off, "Ch 1", '█', Color::Rgb(255, 214, 102));

        app.viz_mode = VizMode::Scope;
        let scope = render(&mut app, 100, 40);
        let scope_text = text_of(&scope);
        assert_side_by_side(&scope, "Samples", "Scope");
        assert_has(&scope_text, "Pat 00");
        assert_has(&scope_text, "kickdrum");
        assert!(
            scope_text
                .chars()
                .any(|ch| ('\u{2800}'..='\u{28FF}').contains(&ch)),
            "expected braille in:\n{scope_text}"
        );
        assert_has(&scope_text, "Stop");
        assert_meter_under_channel(&scope, "Ch 1", '█', Color::Rgb(255, 214, 102));
    }

    #[test]
    fn meters_follow_scrolled_columns_and_fall_when_the_channel_does() {
        let mut app = demo();
        app.install_track(wide_song(12), std::path::PathBuf::from("wide.xm"));
        let mut hot = vec![0u16; 12];
        hot[0] = 8_192;
        hot[11] = 8_192;
        peg_channels(&mut app, &hot);

        let narrow = render(&mut app, 80, 30);
        let narrow_text = text_of(&narrow);
        assert_has(&narrow_text, "XM 12ch");
        assert!(header_row(&narrow).contains("Ch1 "), "{narrow_text}");
        assert!(!header_row(&narrow).contains("Ch12"), "{narrow_text}");
        assert_meter_under_channel(&narrow, "Ch1 ", '█', Color::Rgb(255, 214, 102));
        assert_meter_under_channel(&narrow, "Ch2 ", '▁', Color::Rgb(140, 150, 180));

        app.channel = 11;
        let scrolled = render(&mut app, 80, 30);
        let scrolled_text = text_of(&scrolled);
        assert!(header_row(&scrolled).contains("Ch12"), "{scrolled_text}");
        assert!(!header_row(&scrolled).contains("Ch1 "), "{scrolled_text}");
        assert_meter_under_channel(&scrolled, "Ch12", '█', Color::Rgb(255, 160, 196));
        assert_meter_under_channel(&scrolled, "Ch7 ", '▁', Color::Rgb(140, 150, 180));

        let before = meter_eighths(&scrolled, "Ch12");
        peg_channels(&mut app, &[0; 12]);
        let fallen = render(&mut app, 80, 30);
        let after = meter_eighths(&fallen, "Ch12");
        assert!(
            after < before,
            "meter did not fall: {before} -> {after}\n{}",
            text_of(&fallen)
        );
    }

    #[test]
    fn real_modules_keep_meters_with_the_visible_columns() {
        let xm = load_track("tests/data/blue_intermission_congusbongus_CC0.xm");
        let mut app = demo();
        app.install_track(xm, std::path::PathBuf::from("blue.xm"));
        peg_channels(&mut app, &[8_192, 0, 0, 0, 0, 8_192]);
        let buf = render(&mut app, 100, 40);
        let screen = text_of(&buf);
        assert_has(&screen, "XM 6ch");
        assert_side_by_side(&buf, "Samples", "Spectrum");
        assert_meter_under_channel(&buf, "Ch1 ", '█', Color::Rgb(255, 214, 102));
        assert_meter_under_channel(&buf, "Ch6 ", '█', Color::Rgb(170, 255, 170));
        assert!(header_row(&buf).contains("Ch6 "), "{screen}");
        assert!(!header_row(&buf).contains("Ch7"), "{screen}");

        let it = load_track("tests/data/jingle_bells_drmccoy_CC0.it");
        app.install_track(it, std::path::PathBuf::from("bells.it"));
        peg_channels(&mut app, &[8_192, 0, 0, 0, 0, 0, 0, 8_192]);
        let wide = render(&mut app, 160, 50);
        assert_has(&text_of(&wide), "IT 8ch");
        assert!(header_row(&wide).contains("Ch8 "), "{}", text_of(&wide));
        assert_meter_under_channel(&wide, "Ch1 ", '█', Color::Rgb(255, 214, 102));
        assert_meter_under_channel(&wide, "Ch8 ", '█', Color::Rgb(255, 160, 196));
        let cramped = render(&mut app, 80, 30);
        assert!(
            header_row(&cramped).contains("Ch1 "),
            "{}",
            text_of(&cramped)
        );
        assert!(
            header_row(&cramped).contains("Ch6 "),
            "{}",
            text_of(&cramped)
        );
        assert!(
            !header_row(&cramped).contains("Ch8"),
            "{}",
            text_of(&cramped)
        );
        assert_meter_under_channel(&cramped, "Ch1 ", '█', Color::Rgb(255, 214, 102));
    }

    #[test]
    fn a_64_row_pattern_keeps_the_two_digit_gutter() {
        let mut module = demo();
        let rendered = render(&mut module, 100, 40);
        let header = content_line(&rendered, header_y(&rendered));
        let row = content_line(&rendered, find_pattern_line(&rendered, "00"));
        assert!(header.starts_with("   Ch 1"), "{header}");
        assert!(row.starts_with("00 C-1"), "{row}");
        assert_eq!(header.find("Ch 1"), row.find("C-1"));
        let cell_x = find_on_row(&rendered, find_pattern_line(&rendered, "00"), "C-1").unwrap();
        assert_eq!(usize::from(cell_x) - 1, track_cell_x(64, 0));
        assert_eq!(
            buf_bg(&rendered, cell_x, find_pattern_line(&rendered, "00")),
            Color::Yellow
        );
        assert_ne!(
            buf_bg(&rendered, cell_x - 1, find_pattern_line(&rendered, "00")),
            Color::Yellow
        );

        let mut app = demo();
        app.install_track(wide_song(8), std::path::PathBuf::from("wide.xm"));
        let buf = render(&mut app, 100, 40);
        let header = content_line(&buf, header_y(&buf));
        let y = find_pattern_line(&buf, "00");
        let row = content_line(&buf, y);
        assert!(header.starts_with("   Ch1 "), "{header}");
        assert!(row.starts_with("00 ---"), "{row}");
        assert!(!row.starts_with("000"), "{row}");
        assert_eq!(header.find("Ch1"), row.find("---"));
        let cell_x = find_on_row(&buf, y, "---").unwrap();
        assert_eq!(usize::from(cell_x) - 1, track_cell_x(64, 0));
        assert_eq!(track_channel_at_x(usize::from(cell_x) - 1, 64, 8), Some(0));
        assert_eq!(track_channel_at_x(usize::from(cell_x) - 2, 64, 8), None);
        assert_eq!(buf[(cell_x, y)].bg, Color::Yellow);
        for offset in 0..10 {
            assert_eq!(buf[(cell_x + offset, y)].bg, Color::Yellow);
        }
        assert_ne!(buf[(cell_x + 10, y)].bg, Color::Yellow);
    }

    #[test]
    fn rows_past_99_share_one_gutter_with_the_header_cursor_and_meters() {
        assert_eq!(row_gutter_digits(64), 2);
        assert_eq!(row_gutter_digits(100), 2);
        assert_eq!(row_gutter_digits(101), 3);
        assert_eq!(row_gutter_digits(114), 3);
        assert_eq!(row_gutter_digits(256), 3);
        assert_eq!(row_gutter_digits(1024), 4);
        assert_eq!(row_gutter_text(99, 100), "99 ");
        assert_eq!(row_gutter_text(99, 114), "099 ");
        assert_eq!(row_gutter_text(100, 114), "100 ");
        assert_eq!(row_gutter_text(255, 256), "255 ");
        assert_eq!(row_gutter_text(1023, 1024), "1023 ");

        let mut hundred = demo();
        hundred.install_track(
            song_rows(100, 8, crate::track::Format::Xm),
            std::path::PathBuf::from("hundred.xm"),
        );
        hundred.row = 99;
        let buf = render(&mut hundred, 100, 40);
        let header = content_line(&buf, header_y(&buf));
        let row = content_line(&buf, find_pattern_line(&buf, "99"));
        assert!(header.starts_with("   Ch1 "), "{header}");
        assert!(row.starts_with("99 "), "{row}");
        assert!(!row.starts_with("099"), "{row}");

        let mut boundary = demo();
        boundary.install_track(
            song_rows(101, 8, crate::track::Format::Xm),
            std::path::PathBuf::from("boundary.xm"),
        );
        boundary.row = 100;
        let buf = render(&mut boundary, 160, 50);
        assert!(content_line(&buf, header_y(&buf)).starts_with("    Ch1 "));
        let y99 = find_pattern_line(&buf, "099");
        let y100 = find_pattern_line(&buf, "100");
        assert_eq!(
            find_on_row(&buf, y99, "---"),
            find_on_row(&buf, y100, "---")
        );

        for &(width, height) in &[(100u16, 40u16), (160, 50)] {
            let mut app = demo();
            let mut song = song_rows(114, 16, crate::track::Format::It);
            song.patterns[0].rows[99][0] = crate::track::Cell::tone(60);
            let mut loud = crate::track::Cell::tone(62);
            loud.instrument = 1;
            loud.effect = 0x0A;
            loud.param = 0x0C;
            song.patterns[0].rows[100][0] = loud;
            app.install_track(song, std::path::PathBuf::from("long.it"));
            app.row = 100;
            peg_channels(&mut app, &[8_192]);
            let buf = render(&mut app, width, height);
            let screen = text_of(&buf);
            assert_has(&screen, "Row 100");
            assert_has(&screen, "IT 16ch");
            let header_y = header_y(&buf);
            let header = content_line(&buf, header_y);
            assert!(
                header.starts_with("    Ch1 "),
                "{width}x{height} header {header}"
            );
            let y99 = find_pattern_line(&buf, "099");
            let y100 = find_pattern_line(&buf, "100");
            let row99 = content_line(&buf, y99);
            let row100 = content_line(&buf, y100);
            assert!(row99.starts_with("099 C-5"), "{width}x{height}\n{row99}");
            assert!(
                row100.starts_with("100 D-501--J0C"),
                "{width}x{height}\n{row100}"
            );
            let x99 = find_on_row(&buf, y99, "C-5").unwrap();
            let x100 = find_on_row(&buf, y100, "D-5").unwrap();
            let x_header = find_on_row(&buf, header_y, "Ch1 ").unwrap();
            assert_eq!(x99, x100, "{width}x{height}\n{row99}\n{row100}");
            assert_eq!(x99, x_header, "{header}");
            assert_eq!(usize::from(x100) - 1, track_cell_x(114, 0));
            assert_eq!(track_channel_at_x(usize::from(x100) - 1, 114, 8), Some(0));
            assert_eq!(track_channel_at_x(usize::from(x100) - 2, 114, 8), None);
            assert_eq!(buf[(x100, y100)].bg, Color::Yellow, "cursor on row 100");
            assert_ne!(buf[(x100 - 1, y100)].bg, Color::Yellow, "gutter stays put");
            for offset in 0..10 {
                assert_eq!(
                    buf[(x100 + offset, y100)].bg,
                    Color::Yellow,
                    "cursor column {offset} at {width}x{height}"
                );
            }
            assert_ne!(buf[(x100 + 10, y100)].bg, Color::Yellow);
            assert_ne!(buf[(x99, y99)].bg, Color::Yellow);
            assert_meter_under_channel(&buf, "Ch1 ", '█', Color::Rgb(255, 214, 102));
        }

        let mut xm = demo();
        xm.install_track(
            song_rows(256, 8, crate::track::Format::Xm),
            std::path::PathBuf::from("long.xm"),
        );
        xm.row = 255;
        let buf = render(&mut xm, 160, 50);
        let y255 = find_pattern_line(&buf, "255");
        let y254 = find_pattern_line(&buf, "254");
        assert_eq!(
            find_on_row(&buf, y255, "---"),
            find_on_row(&buf, y254, "---"),
            "XM 256-row gutter drifted\n{}",
            text_of(&buf)
        );
        assert!(content_line(&buf, header_y(&buf)).starts_with("    Ch1 "));
        assert!(content_line(&buf, y255).starts_with("255 "));

        let mut it = demo();
        it.install_track(
            song_rows(1024, 4, crate::track::Format::It),
            std::path::PathBuf::from("max.it"),
        );
        it.row = 1023;
        let buf = render(&mut it, 160, 50);
        let y_last = find_pattern_line(&buf, "1023");
        let y_prev = find_pattern_line(&buf, "1022");
        assert_eq!(
            find_on_row(&buf, y_last, "---"),
            find_on_row(&buf, y_prev, "---")
        );
        assert!(content_line(&buf, header_y(&buf)).starts_with("     Ch1 "));
        assert_eq!(
            usize::from(find_on_row(&buf, y_last, "---").unwrap()) - 1,
            track_cell_x(1024, 0)
        );
    }

    #[test]
    fn column_fit_and_hit_testing_use_the_same_gutter() {
        assert_eq!(track_cell_x(64, 0), 3);
        assert_eq!(track_cell_x(100, 0), 3);
        assert_eq!(track_cell_x(101, 0), 4);
        assert_eq!(track_cell_x(114, 1), 4 + TRACK_CELL);
        assert_eq!(track_channels_fit(80, 64), 7);
        assert_eq!(track_channels_fit(80, 100), 7);
        assert_eq!(track_channels_fit(80, 101), 6);
        assert_eq!(track_channels_fit(80, 256), 6);
        assert_eq!(track_channel_at_x(2, 64, 7), None);
        assert_eq!(track_channel_at_x(3, 64, 7), Some(0));
        assert_eq!(track_channel_at_x(3, 114, 6), None);
        assert_eq!(track_channel_at_x(4, 114, 6), Some(0));
        assert_eq!(track_channel_at_x(13, 114, 6), Some(0));
        assert_eq!(track_channel_at_x(14, 114, 6), None);
        assert_eq!(track_channel_at_x(15, 114, 6), Some(1));

        let mut app = demo();
        app.install_track(
            song_rows(64, 16, crate::track::Format::Xm),
            std::path::PathBuf::from("wide.xm"),
        );
        let narrow = render(&mut app, 82, 30);
        let header = header_row(&narrow);
        assert!(header.contains("Ch7 "), "{header}");
        assert!(!header.contains("Ch8"), "{header}");

        app.channel = 15;
        let scrolled = render(&mut app, 82, 30);
        let header = header_row(&scrolled);
        assert!(header.contains("Ch10"), "{header}");
        assert!(header.contains("Ch16"), "{header}");
        assert!(!header.contains("Ch9"), "{header}");
        assert_eq!(app.channel_scroll, 9);

        app.install_track(
            song_rows(114, 16, crate::track::Format::It),
            std::path::PathBuf::from("long.it"),
        );
        let wide_gutter = render(&mut app, 82, 30);
        let header = header_row(&wide_gutter);
        assert!(header.contains("Ch6 "), "{header}");
        assert!(!header.contains("Ch7"), "{header}");

        app.channel = 15;
        let mut hot = vec![0u16; 16];
        hot[15] = 8_192;
        peg_channels(&mut app, &hot);
        let scrolled = render(&mut app, 82, 30);
        let header = header_row(&scrolled);
        assert!(header.contains("Ch11"), "{header}");
        assert!(header.contains("Ch16"), "{header}");
        assert!(!header.contains("Ch10"), "{header}");
        assert_eq!(app.channel_scroll, 10);
        let y = find_pattern_line(&scrolled, "000");
        let cell_x = find_on_row(&scrolled, y, "---").unwrap();
        assert_eq!(usize::from(cell_x) - 1, track_cell_x(114, 0));
        assert_eq!(
            track_channel_at_x(usize::from(cell_x) - 1, 114, 6)
                .map(|index| index + app.channel_scroll),
            Some(10)
        );
        assert_meter_under_channel(&scrolled, "Ch16", '█', Color::Rgb(255, 160, 196));
    }

    #[test]
    fn synthetic_long_modules_keep_row_99_aligned_with_row_100() {
        let it = load_track("tests/data/long_rows_synthetic.it");
        assert_eq!(it.format, crate::track::Format::It);
        assert_eq!(it.row_count(0), 114);
        assert_eq!(it.channels, 16);
        assert_eq!(
            it.cell(0, 99, 0).map(|cell| cell.note),
            Some(crate::track::Cell::tone(60).note)
        );
        let mut app = demo();
        app.install_track(it, std::path::PathBuf::from("long_rows_synthetic.it"));
        app.row = 100;
        peg_channels(&mut app, &[8_192]);
        for &(width, height) in &[(100u16, 40u16), (160, 50)] {
            let buf = render(&mut app, width, height);
            let y99 = find_pattern_line(&buf, "099");
            let y100 = find_pattern_line(&buf, "100");
            let x99 = find_on_row(&buf, y99, "C-5").expect("row 99 note");
            let x100 = find_on_row(&buf, y100, "D-5").expect("row 100 note");
            let header = find_on_row(&buf, header_y(&buf), "Ch1 ").expect("Ch1");
            assert_eq!(x99, x100, "{width}x{height}\n{}", text_of(&buf));
            assert_eq!(x99, header);
            assert_eq!(usize::from(x99) - 1, track_cell_x(114, 0));
            assert_meter_under_channel(&buf, "Ch1 ", '█', Color::Rgb(255, 214, 102));
            assert!(
                content_line(&buf, y100).starts_with("100 D-5"),
                "{}",
                content_line(&buf, y100)
            );
        }

        let xm = load_track("tests/data/long_rows_synthetic.xm");
        assert_eq!(xm.format, crate::track::Format::Xm);
        assert_eq!(xm.row_count(0), 256);
        assert_eq!(xm.channels, 8);
        app.install_track(xm, std::path::PathBuf::from("long_rows_synthetic.xm"));
        app.row = 255;
        let buf = render(&mut app, 160, 50);
        let y255 = find_pattern_line(&buf, "255");
        let y254 = find_pattern_line(&buf, "254");
        assert_eq!(
            find_on_row(&buf, y255, "---"),
            find_on_row(&buf, y254, "---")
        );
        assert!(content_line(&buf, header_y(&buf)).starts_with("    Ch1 "));
        assert!(content_line(&buf, y255).starts_with("255 "));
    }

    fn peg_channels(app: &mut App, peaks: &[u16]) {
        let mut four = [0u16; 4];
        for (slot, peak) in four.iter_mut().zip(peaks) {
            *slot = *peak;
        }
        let mut stereo = [0i16; crate::viz::WINDOW * 2];
        for index in 0..crate::viz::WINDOW {
            let sample = ((index as f32 * 0.15).sin() * 14_000.0) as i16;
            stereo[index * 2] = sample;
            stereo[index * 2 + 1] = sample / 2;
        }
        for gen in 1..=3 {
            app.tick_viz(
                Some(&crate::viz::VizSnapshot {
                    stereo,
                    peaks: four,
                    rate: 44_100,
                    gen,
                }),
                0.08,
            );
            app.viz.push_extra_peaks(peaks, 0.08);
        }
    }

    fn wide_song(channels: usize) -> crate::Song {
        let mut song = song_rows(64, channels, crate::track::Format::Xm);
        song.title = "Wide".to_string();
        song
    }

    fn song_rows(rows: usize, channels: usize, format: crate::track::Format) -> crate::Song {
        crate::Song {
            title: format!("Rows {rows}"),
            tracker: String::new(),
            format,
            channels,
            orders: vec![0],
            restart: 0,
            patterns: vec![crate::track::Pattern::empty(rows, channels)],
            instruments: Vec::new(),
            samples: Vec::new(),
            linear: true,
            initial_speed: 6,
            initial_tempo: 125,
            initial_global_volume: 64,
            global_volume_max: 64,
            initial_pan: vec![128; channels],
            initial_channel_volume: vec![64; channels],
            initial_mute: vec![false; channels],
            instrument_mode: true,
            old_effects: false,
            compatible_gxx: false,
        }
    }

    fn load_track(rel: &str) -> crate::Song {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        match crate::open_path(&path).unwrap_or_else(|err| panic!("load {rel}: {err}")) {
            crate::Opened::Track(song) => song,
            crate::Opened::Mod(_) => panic!("{rel} parsed as a module"),
        }
    }

    fn content_line(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        let mut out = String::new();
        let end = buf.area.width.saturating_sub(1);
        for x in 1..end {
            out.push_str(buf[(x, y)].symbol());
        }
        out
    }

    fn find_pattern_line(buf: &ratatui::buffer::Buffer, label: &str) -> u16 {
        let header = header_y(buf);
        let prefix = format!("{label} ");
        for y in (header + 1)..buf.area.height {
            if buf[(0, y)].symbol() != "│" {
                continue;
            }
            if content_line(buf, y).starts_with(&prefix) {
                return y;
            }
        }
        panic!(
            "no pattern row {label:?} under the channel header\n{}",
            text_of(buf)
        );
    }

    fn buf_bg(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> Color {
        buf[(x, y)].bg
    }

    fn row_text(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        let mut out = String::new();
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out
    }

    fn find_row(buf: &ratatui::buffer::Buffer, needle: &str) -> u16 {
        for y in 0..buf.area.height {
            if row_text(buf, y).contains(needle) {
                return y;
            }
        }
        panic!("missing {needle:?} in:\n{}", text_of(buf));
    }

    fn find_on_row(buf: &ratatui::buffer::Buffer, y: u16, needle: &str) -> Option<u16> {
        let chars: Vec<char> = needle.chars().collect();
        let width = usize::from(buf.area.width);
        if chars.is_empty() || chars.len() > width {
            return None;
        }
        for x in 0..=width - chars.len() {
            let matched = chars
                .iter()
                .enumerate()
                .all(|(index, ch)| buf[(x as u16 + index as u16, y)].symbol().starts_with(*ch));
            if matched {
                return Some(x as u16);
            }
        }
        None
    }

    fn header_y(buf: &ratatui::buffer::Buffer) -> u16 {
        let samples = find_row(buf, "Samples");
        for y in 0..samples {
            let text = row_text(buf, y);
            if text.contains("Ch1") || text.contains("Ch 1") {
                return y;
            }
        }
        panic!("no channel header in:\n{}", text_of(buf));
    }

    fn header_row(buf: &ratatui::buffer::Buffer) -> String {
        row_text(buf, header_y(buf))
    }

    fn meter_row(buf: &ratatui::buffer::Buffer) -> u16 {
        find_row(buf, "Samples").saturating_sub(2)
    }

    fn assert_meter_under_channel(
        buf: &ratatui::buffer::Buffer,
        label: &str,
        glyph: char,
        color: Color,
    ) {
        let x = find_on_row(buf, header_y(buf), label).unwrap_or_else(|| {
            panic!(
                "missing {label:?} on the header\n{}\n{}",
                header_row(buf),
                text_of(buf)
            )
        });
        let y = meter_row(buf);
        let cell = &buf[(x, y)];
        assert_eq!(
            cell.symbol(),
            glyph.to_string(),
            "meter under {label} at ({x},{y})\n{}",
            text_of(buf)
        );
        assert_eq!(cell.fg, color, "meter color under {label}");
        if y > header_y(buf) + 1 {
            let above = buf[(x, y - 1)].symbol();
            assert!(
                above != "█" && above != "▁" && above != "▇",
                "meter strip is taller than one row under {label}: {above}"
            );
        }
    }

    fn meter_eighths(buf: &ratatui::buffer::Buffer, label: &str) -> usize {
        let x = find_on_row(buf, header_y(buf), label).expect(label);
        let y = meter_row(buf);
        (0..10u16)
            .map(|offset| eighths_of(buf[(x + offset, y)].symbol()))
            .sum()
    }

    fn eighths_of(symbol: &str) -> usize {
        match symbol {
            "▏" => 1,
            "▎" => 2,
            "▍" => 3,
            "▌" => 4,
            "▋" => 5,
            "▊" => 6,
            "▉" => 7,
            "█" => 8,
            _ => 0,
        }
    }

    fn assert_side_by_side(buf: &ratatui::buffer::Buffer, left_title: &str, right_title: &str) {
        let y = find_row(buf, left_title);
        let row = row_text(buf, y);
        let left = row.find(left_title).expect(&row);
        let right = row.find(right_title).unwrap_or_else(|| panic!("{row}"));
        assert!(left < right, "{row}");
        assert_eq!(buf[(0, y)].symbol(), "┌", "{row}");
        let end = buf.area.width - 1;
        assert_eq!(buf[(end, y)].symbol(), "┐", "{row}");
        let mid = usize::from(buf.area.width) / 2;
        let split = (mid.saturating_sub(1)..=mid + 1)
            .find(|x| buf[(*x as u16, y)].symbol() == "┌")
            .unwrap_or_else(|| panic!("no split on {row}"));
        assert_eq!(buf[(split as u16 - 1, y)].symbol(), "┐", "{row}");
        let spectrum_w = usize::from(buf.area.width) - split;
        assert!(
            (split as i32 - spectrum_w as i32).abs() <= 1,
            "left {split} right {spectrum_w} on {row}"
        );
        let bottom = find_row(buf, "Stop") - 1;
        assert_eq!(buf[(0, bottom)].symbol(), "└", "{}", row_text(buf, bottom));
        assert_eq!(buf[(end, bottom)].symbol(), "┘");
        assert_eq!(buf[(split as u16, bottom)].symbol(), "└");
    }

    fn assert_full_width_samples(buf: &ratatui::buffer::Buffer) {
        let y = find_row(buf, "Samples");
        let row = row_text(buf, y);
        assert_eq!(buf[(0, y)].symbol(), "┌", "{row}");
        assert_eq!(buf[(buf.area.width - 1, y)].symbol(), "┐", "{row}");
        assert_eq!(row.chars().filter(|ch| *ch == '┌').count(), 1, "{row}");
    }

    fn assert_closed_boxes(buf: &ratatui::buffer::Buffer) {
        for title in ["Song", "Pattern", "Samples"] {
            let y = find_box_row(buf, title);
            assert_eq!(buf[(0, y)].symbol(), "┌", "{title}");
            assert_eq!(
                buf[(buf.area.width - 1, y)].symbol(),
                "┐",
                "{title} {}",
                row_text(buf, y)
            );
        }
    }

    fn pane_corner(buf: &ratatui::buffer::Buffer, title: &str) -> (u16, u16) {
        for y in 0..buf.area.height {
            let mut word = String::new();
            let mut word_x = 0u16;
            for x in 0..=buf.area.width {
                let symbol = if x == buf.area.width {
                    " "
                } else {
                    buf[(x, y)].symbol()
                };
                let alphanumeric = symbol.len() == 1
                    && symbol
                        .chars()
                        .next()
                        .is_some_and(|ch| ch.is_ascii_alphanumeric());
                if alphanumeric {
                    if word.is_empty() {
                        word_x = x;
                    }
                    word.push_str(symbol);
                    continue;
                }
                if word == title {
                    if let Some(corner) = (0..word_x).rev().find(|&cx| buf[(cx, y)].symbol() == "┌")
                    {
                        return (corner, y);
                    }
                }
                word.clear();
            }
        }
        panic!("no {title} pane in:\n{}", text_of(buf));
    }

    fn pane_border_fg(buf: &ratatui::buffer::Buffer, title: &str) -> Color {
        let (x, y) = pane_corner(buf, title);
        let fg = buf[(x, y)].fg;
        assert_eq!(buf[(x, y)].symbol(), "┌", "{title}");
        assert_eq!(buf[(x, y + 1)].symbol(), "│", "{title}");
        assert_eq!(
            buf[(x, y + 1)].fg,
            fg,
            "{title} side border differs from the corner"
        );
        fg
    }

    fn assert_song_border(buf: &ratatui::buffer::Buffer, expected: Color, label: &str) {
        assert_eq!(pane_border_fg(buf, "Song"), expected, "{label}");
        let (_, y) = pane_corner(buf, "Song");
        let right = buf.area.width - 1;
        assert_eq!(buf[(right, y)].symbol(), "┐", "{label}");
        assert_eq!(buf[(right, y)].fg, expected, "{label} right corner");
    }

    fn find_box_row(buf: &ratatui::buffer::Buffer, title: &str) -> u16 {
        for y in 0..buf.area.height {
            if buf[(0, y)].symbol() != "┌" {
                continue;
            }
            let text = row_text(buf, y);
            let named = text
                .split(|ch: char| !ch.is_ascii_alphanumeric())
                .any(|word| word == title);
            if named {
                return y;
            }
        }
        panic!("no {title} box in:\n{}", text_of(buf));
    }

    fn assert_spectrum_is_bars_only(buf: &ratatui::buffer::Buffer) {
        let y = find_row(buf, "Spectrum");
        let mid = usize::from(buf.area.width) / 2;
        let left = (mid.saturating_sub(1)..=mid + 1)
            .find(|x| buf[(*x as u16, y)].symbol() == "┌")
            .unwrap_or_else(|| panic!("{}", row_text(buf, y))) as u16;
        let right = buf.area.width - 1;
        let bottom = find_row(buf, "Stop") - 1;
        let mut blocks = 0usize;
        for row in (y + 1)..bottom {
            for x in (left + 1)..right {
                let symbol = buf[(x, row)].symbol();
                assert!(
                    matches!(symbol, " " | "▁" | "▂" | "▃" | "▄" | "▅" | "▆" | "▇" | "█"),
                    "spectrum glyph {symbol:?} at ({x},{row})\n{}",
                    text_of(buf)
                );
                if matches!(symbol, "▁" | "▂" | "▃" | "▄" | "▅" | "▆" | "▇" | "█") {
                    blocks += 1;
                }
            }
        }
        assert!(blocks > 0, "spectrum pane was empty\n{}", text_of(buf));
    }

    fn meter_rows_above_border(buf: &ratatui::buffer::Buffer) -> usize {
        let border = find_row(buf, "Samples") - 1;
        let meter = meter_row(buf);
        assert_eq!(buf[(0, border)].symbol(), "└");
        assert_eq!(meter + 1, border);
        1
    }
}
