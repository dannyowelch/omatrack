//! Terminal UI.
//!
//! [`App`] holds the cursor and the edit commands. [`draw`] paints them.
//! [`run`] owns the terminal and, while playback is on, the audio stream.
//! [`Theme`](theme::Theme) is a built-in palette or the active Omarchy theme.

mod app;
mod render;
mod sample;
mod theme;
mod viz;

pub use app::{command_for, App, Command, Focus, Key, Outcome};
pub use render::draw;
pub use theme::{detect_color_depth, ColorDepth, Theme};

use std::path::Path;

/// Palette chosen for this launch, plus the files to watch.
pub struct ThemeChoice {
    /// Colors.
    pub theme: Theme,
    /// Song-header label.
    pub label: String,
    /// Paths passed to [`Session::theme_watch`].
    pub watch: Vec<PathBuf>,
    /// Set when an explicit Omarchy theme could not be read.
    pub warning: Option<String>,
}

/// Resolve `request` the same way the running tracker does.
pub fn load_theme(
    request: ThemeRequest,
    depth: ColorDepth,
    home: &Path,
    xdg_state_home: Option<&Path>,
) -> ThemeChoice {
    let loaded = theme::resolve(request, depth, home, xdg_state_home);
    ThemeChoice {
        theme: loaded.theme,
        label: loaded.label,
        watch: loaded.watch.paths,
        warning: loaded.warning,
    }
}

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::audio::AudioOutput;
use crate::config::{self, ThemeRequest};
use crate::error::Error;
use crate::module::Module;
use crate::player::PlayerConfig;
use crate::viz::VizMode;

use self::app::{Followup, Key as AppKey};
use self::sample::PathKind;
use self::theme::{LoadedTheme, ThemeWatch};

/// Everything the terminal loop needs besides the module itself.
pub struct Session {
    /// Song to show. An empty module is a new file, or the placeholder under [`Self::track`].
    pub module: Module,
    /// XM or IT song. `.mod` leaves this empty.
    pub track: Option<crate::Song>,
    /// Where Ctrl-S writes. Empty until the user picks a path.
    pub path: PathBuf,
    /// Palette already resolved for this launch.
    pub theme: Theme,
    /// Song-header label.
    pub theme_label: String,
    /// Files whose mtime should reload the palette.
    pub theme_watch: Vec<PathBuf>,
    /// Request to resolve again on SIGUSR1 or a theme-file change.
    pub theme_request: ThemeRequest,
    /// Truecolor, 256, or 16, chosen at startup.
    pub color_depth: ColorDepth,
    /// Mixer knobs.
    pub player: PlayerConfig,
    /// Cap for the in-app song render.
    pub max_seconds: f64,
    /// Piano octave.
    pub octave: u8,
    /// Edit step.
    pub step: u8,
    /// Visualization shown when the tracker opens.
    pub default_view: VizMode,
    /// Shown once, in the error color. Config and theme warnings land here.
    pub notice: Option<String>,
    /// `last_file` path. Open, save, and new update it.
    pub state_path: PathBuf,
}

/// Show `session` until the user quits.
///
/// Space starts playback from the cursor. Ctrl-R rewinds to order 0, row 0.
/// Enter toggles edit mode. If no
/// output device can be opened, the transport bar shows the error and the
/// view stays up. Note preview is skipped when that happens. The terminal is
/// restored on quit and on panic. SIGUSR1, or a change to the watched theme
/// files, reloads the palette.
pub fn run(session: Session) -> Result<(), Error> {
    install_panic_hook();
    let reload = install_reload_flag();
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(Error::Terminal)?;
    terminal.clear().map_err(Error::Terminal)?;

    let Session {
        module,
        track,
        path,
        theme,
        theme_label,
        theme_watch,
        theme_request,
        color_depth,
        player,
        max_seconds,
        octave,
        step,
        default_view,
        notice,
        state_path,
    } = session;
    let mut app = App::open(module, path);
    if let Some(song) = track {
        let path = app.path.clone();
        app.install_track(song, path);
    }
    app.set_theme(theme, theme_label);
    app.set_preferences(octave, step, player, max_seconds, default_view);
    if let Some(notice) = notice {
        app.set_error(notice);
    }
    let mut audio = AudioOutput::new();
    audio.set_preferences(app.player);
    let home = config::home_dir();
    let state = config::xdg_state_home();
    let mut watch = ThemeWatch::new(theme_watch);
    let mut last_draw = Instant::now();
    loop {
        let now = Instant::now();
        let dt = now.saturating_duration_since(last_draw).as_secs_f32();
        last_draw = now;
        // Software playback publishes the mix before the analyzer reads it.
        // A real device is already publishing from its own callback.
        audio.pump_software();
        if app.viz_mode != VizMode::Off {
            let snapshot = audio.visualization();
            app.tick_viz(snapshot.as_ref(), dt);
            if let Some(peaks) = audio.track_peaks() {
                app.viz.push_extra_peaks(&peaks, dt);
            }
        }
        if reload.swap(false, Ordering::Relaxed) || watch.changed() {
            let loaded = theme::resolve(theme_request, color_depth, &home, state.as_deref());
            apply_theme(&mut app, &mut watch, loaded);
        }
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
        let wait = if app.playing {
            20
        } else if app.viz_mode == VizMode::Scope {
            33
        } else {
            200
        };
        if event::poll(Duration::from_millis(wait)).map_err(Error::Terminal)? {
            match event::read().map_err(Error::Terminal)? {
                Event::Key(key) => {
                    if let Some(command) = map_key(key).and_then(|key| command_for(&app, key)) {
                        handle_command(&mut app, &mut audio, command, &state_path);
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        if app.should_quit() {
            break;
        }
    }
    audio.stop();
    Ok(())
}

fn apply_theme(app: &mut App, watch: &mut ThemeWatch, loaded: LoadedTheme) {
    let changed = loaded.label != app.theme_label || loaded.theme != app.theme;
    let label = loaded.label.clone();
    app.set_theme(loaded.theme, label);
    *watch = loaded.watch;
    if let Some(warning) = loaded.warning {
        app.set_error(warning);
    } else if changed {
        app.set_message(format!("Theme {}", app.theme_label));
    }
}

fn install_reload_flag() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        // A theme switcher can `kill -USR1` this process. Missing the signal
        // still leaves the mtime watch, so a theme swap is noticed either way.
        let _ = signal_hook::flag::register(signal_hook::consts::SIGUSR1, Arc::clone(&flag));
    }
    flag
}

fn handle_command(app: &mut App, audio: &mut AudioOutput, command: Command, state_path: &Path) {
    let was_playing = app.playing;
    let outcome = app.apply(command);
    match outcome {
        Outcome::Play => {
            if app.playing {
                let started = if let Some(song) = &app.track {
                    audio.start_track(song, app.order_pos, app.row, &app.muted)
                } else {
                    audio.start(&app.module, app.order_pos, app.row, &app.muted)
                };
                if let Err(err) = started {
                    app.fail_audio(err.to_string());
                }
            } else if was_playing {
                audio.stop();
            }
        }
        Outcome::Rewind => {
            // Drop the previous mix before the stream restarts, including
            // when playback stays stopped and no new window is coming.
            audio.clear_visualization();
            if app.playing {
                if let Err(err) = audio.start(&app.module, app.order_pos, app.row, app.muted) {
                    app.fail_audio(err.to_string());
                }
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
        Outcome::Save | Outcome::SaveAndQuit => {
            if persist(app, state_path) {
                apply_followup(app, audio, state_path);
            }
        }
        Outcome::Saved => {
            if !app.path.as_os_str().is_empty() {
                let _ = crate::state::write(state_path, &app.path);
            }
            apply_followup(app, audio, state_path);
        }
        Outcome::DocumentReplaced => {
            audio.stop();
            remember_document(app, state_path);
        }
        Outcome::Audition { slot, period } => {
            if let Err(err) = audio.audition(&app.module, slot, period) {
                app.fail_audio(err.to_string());
            }
        }
        Outcome::None => {}
    }
}

fn remember_document(app: &App, state_path: &Path) {
    if app.path.as_os_str().is_empty() {
        let _ = crate::state::clear(state_path);
    } else {
        let _ = crate::state::write(state_path, &app.path);
    }
}

fn persist(app: &mut App, state_path: &Path) -> bool {
    if app.path.as_os_str().is_empty() {
        app.prompt_path(PathKind::SaveModule);
        return false;
    }
    match app.save() {
        Ok(()) => {
            let _ = crate::state::write(state_path, &app.path);
            let name = app
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("module");
            app.set_message(format!("Saved {name}"));
            true
        }
        Err(message) => {
            let _ = app.take_followup();
            app.set_error(message);
            false
        }
    }
}

fn apply_followup(app: &mut App, audio: &mut AudioOutput, state_path: &Path) {
    match app.take_followup() {
        Some(Followup::Quit) => {
            audio.stop();
            app.quit_now();
        }
        Some(Followup::New) => {
            let _ = app.install_blank();
            audio.stop();
            let _ = crate::state::clear(state_path);
        }
        Some(Followup::Open) => app.prompt_path(PathKind::OpenModule),
        None => {}
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
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
    let _ = disable_raw_mode();
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
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
        let ctrl_r = KeyEvent::new(KeyCode::Char('\u{12}'), KeyModifiers::CONTROL);
        assert_eq!(map_key(ctrl_r), Some(AppKey::Ctrl('r')));

        let alt = KeyEvent::new(KeyCode::Char('K'), KeyModifiers::ALT);
        assert_eq!(map_key(alt), Some(AppKey::Alt('k')));
        let alt_up = KeyEvent::new(KeyCode::Up, KeyModifiers::ALT);
        assert_eq!(map_key(alt_up), Some(AppKey::AltUp));
        let insert = KeyEvent::new(KeyCode::Insert, KeyModifiers::CONTROL);
        assert_eq!(map_key(insert), Some(AppKey::CtrlInsert));
    }
}
