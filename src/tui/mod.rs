//! Terminal UI.
//!
//! [`App`] holds the cursor. [`draw`] paints it. [`run`] owns the terminal.
//! Milestone 3 can add edit commands without changing this split, and milestone
//! 5 can replace [`Theme`](theme::Theme).

mod app;
mod render;
mod theme;

pub use app::{command_for, App, Command, Focus, Key};
pub use render::draw;
pub use theme::Theme;

use std::io;
use std::time::Duration;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::error::Error;
use crate::module::Module;

use self::app::Key as AppKey;

/// Show `module` until the user quits. Playback is not part of this milestone.
pub fn run(module: Module) -> Result<(), Error> {
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(Error::Terminal)?;
    terminal.clear().map_err(Error::Terminal)?;

    let mut app = App::new(module);
    loop {
        terminal
            .draw(|frame| draw(frame, &mut app))
            .map_err(Error::Terminal)?;
        if event::poll(Duration::from_millis(200)).map_err(Error::Terminal)? {
            if let Event::Key(key) = event::read().map_err(Error::Terminal)? {
                if let Some(command) = map_key(key).and_then(|key| command_for(app.focus, key)) {
                    app.apply(command);
                }
            }
        }
        if app.should_quit() {
            break;
        }
    }
    Ok(())
}

fn map_key(key: KeyEvent) -> Option<AppKey> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Char('q') | KeyCode::Char('Q') => {
                Some(AppKey::CtrlC)
            }
            _ => None,
        };
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        return None;
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
        _ => None,
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
        assert_eq!(map_key(ctrl), Some(AppKey::CtrlC));

        let alt = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT);
        assert_eq!(map_key(alt), None);
    }
}
