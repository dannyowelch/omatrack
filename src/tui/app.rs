//! View state for the read-only tracker.
//!
//! Playback (milestone 2) and editing (milestone 3) should sit beside this:
//! the mixer borrows [`Module`](crate::Module), and edit commands mutate it.
//! [`App`] only remembers what the screen is showing.

use crate::module::{Cell, Module, CHANNELS, ORDER_LEN, ROWS, SAMPLE_COUNT};
use crate::player::{DEFAULT_SPEED, DEFAULT_TEMPO};

/// Which pane receives movement keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The pattern editor.
    Pattern,
    /// The instrument list.
    Samples,
}

/// A key the view understands, after the terminal crate has been translated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable character, including shifted letters.
    Char(char),
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Home.
    Home,
    /// End.
    End,
    /// Tab.
    Tab,
    /// Shift-tab.
    BackTab,
    /// Escape.
    Esc,
    /// Ctrl-C or Ctrl-Q.
    CtrlC,
}

/// One change to the view. Editing commands can grow this enum later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Leave the UI.
    Quit,
    /// Move the pattern row by `delta`, clamped to `0..64`.
    MoveRow(isize),
    /// Move the channel cursor by `delta`, clamped to `0..4`.
    MoveChannel(isize),
    /// Jump to row 0.
    FirstRow,
    /// Jump to row 63.
    LastRow,
    /// Move through the played order list. The viewed pattern follows.
    MoveOrder(isize),
    /// Move to another pattern without changing the order position.
    MovePattern(isize),
    /// Focus the next pane.
    NextFocus,
    /// Focus the previous pane.
    PrevFocus,
    /// Move the sample cursor by `delta`.
    MoveSample(isize),
    /// Jump to instrument 1.
    FirstSample,
    /// Jump to instrument 31.
    LastSample,
    /// Start playback from the cursor, or stop it.
    TogglePlay,
    /// Silence or restore channel `0..4`.
    ToggleMute(usize),
}

const PATTERN_PAGE: isize = 16;
const SAMPLE_PAGE: isize = 8;

/// Read-only viewer state.
#[derive(Debug)]
pub struct App {
    pub(crate) module: Module,
    pub(crate) focus: Focus,
    pub(crate) order_pos: usize,
    pub(crate) view_pattern: usize,
    pub(crate) row: usize,
    pub(crate) channel: usize,
    pub(crate) sample: usize,
    pub(crate) row_offset: usize,
    pub(crate) sample_offset: usize,
    /// The replayer is pulling audio.
    pub(crate) playing: bool,
    /// Ticks per row, from the replayer.
    pub(crate) speed: u8,
    /// CIA tempo, from the replayer.
    pub(crate) tempo: u8,
    /// Per-channel mute. Index 0 is channel 1.
    pub(crate) muted: [bool; CHANNELS],
    /// Last failure from opening the audio device, shown in the transport bar.
    pub(crate) audio_error: Option<String>,
    quit: bool,
}

impl App {
    /// Open `module` on order position 0, row 0, channel 1.
    pub fn new(module: Module) -> Self {
        let view_pattern = usize::from(module.order[0]);
        Self {
            module,
            focus: Focus::Pattern,
            order_pos: 0,
            view_pattern,
            row: 0,
            channel: 0,
            sample: 0,
            row_offset: 0,
            sample_offset: 0,
            playing: false,
            speed: DEFAULT_SPEED,
            tempo: DEFAULT_TEMPO,
            muted: [false; CHANNELS],
            audio_error: None,
            quit: false,
        }
    }

    /// Snap the view to the row the replayer is mixing.
    pub(crate) fn follow(&mut self, order: usize, row: usize, speed: u8, tempo: u8) {
        self.speed = speed;
        self.tempo = tempo;
        let len = self.song_len();
        self.order_pos = order.min(len.saturating_sub(1));
        let pattern = usize::from(self.module.order[self.order_pos]);
        if pattern < self.module.patterns.len() {
            self.view_pattern = pattern;
        }
        self.row = row.min(ROWS - 1);
    }

    /// Remember that the audio device could not be opened, and leave playback stopped.
    pub(crate) fn fail_audio(&mut self, message: String) {
        self.playing = false;
        self.audio_error = Some(message);
    }

    /// Whether the user asked to quit.
    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// Apply one command. Movement past either end sticks.
    pub fn apply(&mut self, command: Command) {
        match command {
            Command::Quit => self.quit = true,
            Command::MoveRow(delta) => self.row = step(self.row, delta, ROWS),
            Command::MoveChannel(delta) => self.channel = step(self.channel, delta, CHANNELS),
            Command::FirstRow => self.row = 0,
            Command::LastRow => self.row = ROWS - 1,
            Command::MoveOrder(delta) => {
                let len = self.song_len();
                self.order_pos = step(self.order_pos, delta, len);
                self.view_pattern = usize::from(self.module.order[self.order_pos]);
            }
            Command::MovePattern(delta) => {
                let len = self.module.patterns.len();
                self.view_pattern = step(self.view_pattern, delta, len);
            }
            Command::NextFocus => self.focus = cycle(self.focus, true),
            Command::PrevFocus => self.focus = cycle(self.focus, false),
            Command::MoveSample(delta) => self.sample = step(self.sample, delta, SAMPLE_COUNT),
            Command::FirstSample => self.sample = 0,
            Command::LastSample => self.sample = SAMPLE_COUNT - 1,
            Command::TogglePlay => {
                self.playing = !self.playing;
                if self.playing {
                    self.audio_error = None;
                }
            }
            Command::ToggleMute(channel) => {
                if let Some(muted) = self.muted.get_mut(channel) {
                    *muted = !*muted;
                }
            }
        }
    }

    /// Keep the cursors inside the rows the panes can show.
    pub(crate) fn reconcile_scroll(&mut self, pattern_window: usize, sample_window: usize) {
        self.row_offset = window_start(self.row_offset, self.row, ROWS, pattern_window);
        self.sample_offset =
            window_start(self.sample_offset, self.sample, SAMPLE_COUNT, sample_window);
    }

    /// Played order length, at least 1 so the cursor always has a slot.
    pub(crate) fn song_len(&self) -> usize {
        usize::from(self.module.song_length).clamp(1, ORDER_LEN)
    }

    /// Pattern number stored at the current order position.
    pub(crate) fn order_pattern(&self) -> u8 {
        let last = self.song_len().saturating_sub(1);
        self.module.order[self.order_pos.min(last)]
    }

    /// Cell under the pattern cursor, if that pattern exists.
    pub(crate) fn current_cell(&self) -> Option<Cell> {
        let pattern = self.module.patterns.get(self.view_pattern)?;
        Some(pattern.rows[self.row][self.channel])
    }
}

/// Map a key to a command. Letter keys are case-insensitive.
pub fn command_for(focus: Focus, key: Key) -> Option<Command> {
    let key = match key {
        Key::Char(ch) => Key::Char(ch.to_ascii_lowercase()),
        other => other,
    };
    match key {
        Key::CtrlC | Key::Esc | Key::Char('q') => return Some(Command::Quit),
        Key::Tab => return Some(Command::NextFocus),
        Key::BackTab => return Some(Command::PrevFocus),
        Key::Char('[') => return Some(Command::MoveOrder(-1)),
        Key::Char(']') => return Some(Command::MoveOrder(1)),
        Key::Char(',') => return Some(Command::MovePattern(-1)),
        Key::Char('.') => return Some(Command::MovePattern(1)),
        Key::Char(' ') => return Some(Command::TogglePlay),
        Key::Char('1') => return Some(Command::ToggleMute(0)),
        Key::Char('2') => return Some(Command::ToggleMute(1)),
        Key::Char('3') => return Some(Command::ToggleMute(2)),
        Key::Char('4') => return Some(Command::ToggleMute(3)),
        _ => {}
    }
    match focus {
        Focus::Pattern => match key {
            Key::Up | Key::Char('k') => Some(Command::MoveRow(-1)),
            Key::Down | Key::Char('j') => Some(Command::MoveRow(1)),
            Key::Left | Key::Char('h') => Some(Command::MoveChannel(-1)),
            Key::Right | Key::Char('l') => Some(Command::MoveChannel(1)),
            Key::PageUp => Some(Command::MoveRow(-PATTERN_PAGE)),
            Key::PageDown => Some(Command::MoveRow(PATTERN_PAGE)),
            Key::Home => Some(Command::FirstRow),
            Key::End => Some(Command::LastRow),
            _ => None,
        },
        Focus::Samples => match key {
            Key::Up | Key::Char('k') => Some(Command::MoveSample(-1)),
            Key::Down | Key::Char('j') => Some(Command::MoveSample(1)),
            Key::PageUp => Some(Command::MoveSample(-SAMPLE_PAGE)),
            Key::PageDown => Some(Command::MoveSample(SAMPLE_PAGE)),
            Key::Home => Some(Command::FirstSample),
            Key::End => Some(Command::LastSample),
            _ => None,
        },
    }
}

fn step(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let current = isize::try_from(current.min(len - 1)).unwrap_or(0);
    let next = current.saturating_add(delta);
    if next <= 0 {
        0
    } else {
        usize::try_from(next).unwrap_or(len - 1).min(len - 1)
    }
}

fn window_start(offset: usize, cursor: usize, len: usize, window: usize) -> usize {
    if window == 0 {
        return 0;
    }
    let max_offset = len.saturating_sub(window);
    let mut offset = offset.min(max_offset);
    if cursor < offset {
        offset = cursor;
    } else if cursor >= offset.saturating_add(window) {
        offset = cursor + 1 - window;
    }
    offset.min(max_offset)
}

fn cycle(focus: Focus, forward: bool) -> Focus {
    const PANES: [Focus; 2] = [Focus::Pattern, Focus::Samples];
    let index = PANES.iter().position(|pane| *pane == focus).unwrap_or(0);
    let len = isize::try_from(PANES.len()).unwrap_or(1);
    let delta = if forward { 1 } else { -1 };
    let next = (isize::try_from(index).unwrap_or(0) + delta).rem_euclid(len);
    let next = usize::try_from(next).unwrap_or(0);
    PANES[next.min(PANES.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::Tag;

    fn app_with_patterns(count: usize) -> App {
        let mut module = Module::new(Tag::Mk);
        module.song_length = u8::try_from(count.clamp(1, 128)).unwrap_or(1);
        for (index, slot) in module.order.iter_mut().enumerate().take(count) {
            *slot = u8::try_from(index).unwrap_or(0);
        }
        module.resize_patterns();
        App::new(module)
    }

    #[test]
    fn movement_clamps_and_order_follows_the_pattern() {
        let mut app = app_with_patterns(3);
        app.apply(Command::MoveRow(-1));
        assert_eq!(app.row, 0);
        app.apply(Command::MoveRow(16));
        assert_eq!(app.row, 16);
        app.apply(Command::LastRow);
        assert_eq!(app.row, 63);
        app.apply(Command::MoveRow(1));
        assert_eq!(app.row, 63);
        app.apply(Command::FirstRow);
        assert_eq!(app.row, 0);

        app.apply(Command::MoveChannel(-1));
        assert_eq!(app.channel, 0);
        app.apply(Command::MoveChannel(10));
        assert_eq!(app.channel, 3);

        app.apply(Command::MoveOrder(-1));
        assert_eq!(app.order_pos, 0);
        assert_eq!(app.view_pattern, 0);
        app.apply(Command::MoveOrder(1));
        assert_eq!(app.order_pos, 1);
        assert_eq!(app.view_pattern, 1);
        app.apply(Command::MovePattern(1));
        assert_eq!(app.order_pos, 1);
        assert_eq!(app.view_pattern, 2);
        app.apply(Command::MovePattern(5));
        assert_eq!(app.view_pattern, 2);
        app.apply(Command::MovePattern(-1));
        assert_eq!(app.view_pattern, 1);
    }

    #[test]
    fn sample_focus_moves_samples_not_rows() {
        let mut app = app_with_patterns(1);
        app.apply(command_for(app.focus, Key::Tab).unwrap());
        assert_eq!(app.focus, Focus::Samples);
        app.apply(command_for(app.focus, Key::Down).unwrap());
        app.apply(command_for(app.focus, Key::Char('j')).unwrap());
        assert_eq!(app.sample, 2);
        assert_eq!(app.row, 0);
        app.apply(Command::LastSample);
        assert_eq!(app.sample, 30);
        app.apply(Command::MoveSample(1));
        assert_eq!(app.sample, 30);
        app.apply(command_for(app.focus, Key::BackTab).unwrap());
        assert_eq!(app.focus, Focus::Pattern);
    }

    #[test]
    fn keys_quit_and_ignore_unknown() {
        let app = app_with_patterns(1);
        assert_eq!(
            command_for(Focus::Pattern, Key::Char('Q')),
            Some(Command::Quit)
        );
        assert_eq!(command_for(Focus::Samples, Key::Esc), Some(Command::Quit));
        assert_eq!(command_for(Focus::Pattern, Key::CtrlC), Some(Command::Quit));
        assert_eq!(command_for(Focus::Pattern, Key::Char('x')), None);
        assert_eq!(
            command_for(Focus::Pattern, Key::Char(' ')),
            Some(Command::TogglePlay)
        );
        assert_eq!(
            command_for(Focus::Samples, Key::Char('3')),
            Some(Command::ToggleMute(2))
        );
        let mut playing = app;
        playing.apply(Command::TogglePlay);
        assert!(playing.playing);
        playing.apply(Command::ToggleMute(0));
        assert!(playing.muted[0]);
        playing.apply(Command::TogglePlay);
        assert!(!playing.playing);
        assert_eq!(
            command_for(Focus::Pattern, Key::Char(']')),
            Some(Command::MoveOrder(1))
        );
        assert_eq!(
            command_for(Focus::Samples, Key::Char('.')),
            Some(Command::MovePattern(1))
        );
        assert_eq!(
            command_for(Focus::Pattern, Key::Char('l')),
            Some(Command::MoveChannel(1))
        );
        assert_eq!(command_for(Focus::Samples, Key::Right), None);
        let mut quitting = playing;
        quitting.apply(Command::Quit);
        assert!(quitting.should_quit());
    }
}
