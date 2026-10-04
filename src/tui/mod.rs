//! Terminal UI.
//!
//! [`App`] holds the cursor and the edit commands. [`draw`] paints them.
//! [`run`] owns the terminal and, while playback is on, the audio stream.
//! Milestone 5 can replace [`Theme`](theme::Theme).

mod app;
mod render;
mod theme;

pub use app::{command_for, App, Command, Focus, Key, Outcome};
pub use render::draw;
pub use theme::Theme;

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::audio::AudioOutput;
use crate::error::Error;
use crate::module::Module;

use self::app::Key as AppKey;

/// Show `module` until the user quits. Ctrl-S writes it back to `path`.
///
/// Space starts playback from the cursor. Enter toggles edit mode. If no
/// output device can be opened, the transport bar shows the error and the
/// view stays up. Note preview is skipped when that happens.
pub fn run(module: Module, path: PathBuf) -> Result<(), Error> {
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(Error::Terminal)?;
    terminal.clear().map_err(Error::Terminal)?;

    let mut app = App::open(module, path);
    let mut audio = AudioOutput::new();
    loop {
        if app.playing {
            if let Some(message) = audio.take_error() {
                audio.stop();
                app.fail_audio(message);
            } else if let Some(snapshot) = audio.snapshot() {
                app.set_clock(snapshot.speed, snapshot.tempo);
                if !app.editing {
                    app.follow(snapshot.order, snapshot.row, snapshot.speed, snapshot.tempo);
                }
            }
        } else {
            audio.stop_if_preview_done();
        }
        terminal
            .draw(|frame| draw(frame, &mut app))
            .map_err(Error::Terminal)?;
        let wait = if app.playing { 20 } else { 200 };
        if event::poll(Duration::from_millis(wait)).map_err(Error::Terminal)? {
            if let Event::Key(key) = event::read().map_err(Error::Terminal)? {
                if let Some(command) = map_key(key).and_then(|key| command_for(&app, key)) {
                    handle_command(&mut app, &mut audio, command);
                }
            }
        }
        if app.should_quit() {
            break;
        }
    }
    audio.stop();
    Ok(())
}

fn handle_command(app: &mut App, audio: &mut AudioOutput, command: Command) {
    let was_playing = app.playing;
    let outcome = app.apply(command);
    match outcome {
        Outcome::Play => {
            if app.playing {
                if let Err(err) = audio.start(&app.module, app.order_pos, app.row, app.muted) {
                    app.fail_audio(err.to_string());
                }
            } else if was_playing {
                audio.stop();
            }
        }
        Outcome::Mute(channel) => audio.set_mute(channel, app.muted[channel]),
        Outcome::Preview {
            sample,
            period,
            channel,
        } => {
            if app.playing {
                audio.replace_module(&app.module);
            } else {
                // A missing device must not cover the pattern. Space still
                // reports that failure on the transport bar.
                let _ = audio.preview(&app.module, sample, period, channel);
            }
        }
        Outcome::Edited => {
            if app.playing {
                audio.replace_module(&app.module);
            }
        }
        Outcome::Save => {
            persist(app);
        }
        Outcome::SaveAndQuit => {
            if persist(app) {
                app.quit_now();
            }
        }
        Outcome::None => {}
    }
}

fn persist(app: &mut App) -> bool {
    match app.save() {
        Ok(()) => {
            let name = app
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("module");
            app.set_message(format!("Saved {name}"));
            true
        }
        Err(message) => {
            app.set_message(message);
            false
        }
    }
}

fn map_key(key: KeyEvent) -> Option<AppKey> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if ctrl && alt {
        return None;
    }
    if alt {
        return match key.code {
            KeyCode::Char(ch) => Some(AppKey::Alt(ch.to_ascii_lowercase())),
            KeyCode::Up => Some(AppKey::AltUp),
            KeyCode::Down => Some(AppKey::AltDown),
            KeyCode::Left => Some(AppKey::AltLeft),
            KeyCode::Right => Some(AppKey::AltRight),
            _ => None,
        };
    }
    if ctrl {
        return match key.code {
            KeyCode::Char(ch) => Some(AppKey::Ctrl(ctrl_char(ch))),
            KeyCode::Backspace => Some(AppKey::CtrlBackspace),
            KeyCode::Delete => Some(AppKey::CtrlDelete),
            KeyCode::Insert => Some(AppKey::CtrlInsert),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char(ch) => Some(AppKey::Char(ch)),
        KeyCode::Up => Some(AppKey::Up),
        KeyCode::Down => Some(AppKey::Down),
        KeyCode::Left => Some(AppKey::Left),
        KeyCode::Right => Some(AppKey::Right),
        KeyCode::PageUp => Some(AppKey::PageUp),
        KeyCode::PageDown => Some(AppKey::PageDown),
        KeyCode::Home => Some(AppKey::Home),
        KeyCode::End => Some(AppKey::End),
        KeyCode::Tab => Some(AppKey::Tab),
        KeyCode::BackTab => Some(AppKey::BackTab),
        KeyCode::Esc => Some(AppKey::Esc),
        KeyCode::Enter => Some(AppKey::Enter),
        KeyCode::Backspace => Some(AppKey::Backspace),
        KeyCode::Delete => Some(AppKey::Delete),
        KeyCode::Insert => Some(AppKey::Insert),
        KeyCode::F(n) => Some(AppKey::F(n)),
        _ => None,
    }
}

fn ctrl_char(ch: char) -> char {
    let raw = u32::from(ch);
    if (1..=26).contains(&raw) {
        char::from(b'a' + u8::try_from(raw - 1).unwrap_or(0))
    } else {
        ch.to_ascii_lowercase()
    }
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self, Error> {
        enable_raw_mode().map_err(Error::Terminal)?;
        if let Err(source) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(Error::Terminal(source));
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_events_are_ignored_and_ctrl_c_quits() {
        let press = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty());
        assert_eq!(map_key(press), Some(AppKey::Char('q')));

        let release = KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::empty(),
            KeyEventKind::Release,
        );
        assert_eq!(map_key(release), None);

        let ctrl = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map_key(ctrl), Some(AppKey::Ctrl('c')));
        let ctrl_code = KeyEvent::new(KeyCode::Char('\u{13}'), KeyModifiers::CONTROL);
        assert_eq!(map_key(ctrl_code), Some(AppKey::Ctrl('s')));

        let alt = KeyEvent::new(KeyCode::Char('K'), KeyModifiers::ALT);
        assert_eq!(map_key(alt), Some(AppKey::Alt('k')));
        let alt_up = KeyEvent::new(KeyCode::Up, KeyModifiers::ALT);
        assert_eq!(map_key(alt_up), Some(AppKey::AltUp));
        let insert = KeyEvent::new(KeyCode::Insert, KeyModifiers::CONTROL);
        assert_eq!(map_key(insert), Some(AppKey::CtrlInsert));
    }
}
