//! Render the tracker into a cell dump a screenshot tool can turn into a PNG.
//!
//! ```text
//! cargo run --example screenshot -- /tmp/omatrack-pattern.cells pattern
//! ```
//!
//! Each line is `x y fg bg symbol`, with colors as `#rrggbb`. The first line
//! is `width height`.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use omatrack::demo;
use omatrack::omarchy;
use omatrack::player::{Playback, PlayerConfig};
use omatrack::tui::{draw, App, Command, Theme};
use omatrack::viz::{VizSnapshot, WINDOW};
use omatrack::Module;
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!(
            "usage: screenshot <out.cells> [pattern|edit|help|file|phosphor|omarchy|viz|scope|viz-matte|viz-module]"
        );
        return ExitCode::from(2);
    };
    let mode = args.next().unwrap_or_else(|| "pattern".to_string());
    match render(&PathBuf::from(path), &mode) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("screenshot: {err}");
            ExitCode::from(1)
        }
    }
}

fn render(path: &PathBuf, mode: &str) -> io::Result<()> {
    let mut app = App::new(demo::showcase());
    match mode {
        "pattern" => {
            app.apply(Command::TogglePlay);
        }
        "edit" => {
            app.apply(Command::ToggleEdit);
            app.apply(Command::EnterNote(0));
        }
        "help" => {
            app.apply(Command::ShowHelp);
        }
        "file" => {
            app.apply(Command::ToggleEdit);
            app.apply(Command::EnterNote(0));
            app.apply(Command::ShowFile);
        }
        "phosphor" => {
            app.set_theme(Theme::phosphor(), "phosphor");
            app.apply(Command::MoveRow(2));
            app.apply(Command::TogglePlay);
        }
        "omarchy" => {
            let palette = omarchy::palette_from_colors_toml(TOKYO).expect("palette");
            app.set_theme(Theme::from_palette(&palette), "tokyo-night");
            app.apply(Command::TogglePlay);
        }
        "viz" => {
            app.apply(Command::TogglePlay);
            app.apply(Command::CycleViz);
            seed_viz(&mut app);
        }
        "scope" => {
            app.apply(Command::TogglePlay);
            app.apply(Command::CycleViz);
            app.apply(Command::CycleViz);
            seed_viz(&mut app);
        }
        "viz-matte" => {
            let palette = omarchy::palette_from_colors_toml(MATTE_BLACK).expect("palette");
            app.set_theme(Theme::from_palette(&palette), "matte-black");
            app.apply(Command::TogglePlay);
            app.apply(Command::CycleViz);
            seed_viz(&mut app);
        }
        "viz-module" => {
            let palette = omarchy::palette_from_colors_toml(MATTE_BLACK).expect("palette");
            let path =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/11thhour_TDK_CCBY.mod");
            let module = Module::load(&path).map_err(io::Error::other)?;
            app = App::new(module.clone());
            app.set_theme(Theme::from_palette(&palette), "matte-black");
            app.apply(Command::TogglePlay);
            app.apply(Command::CycleViz);
            // Order 6 is the passage in the reported screenshot (pattern 4).
            // A couple of seconds so the automatic gain has settled on this passage.
            feed_module(&mut app, &module, 6, 0, 220);
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown view {other}"),
            ));
        }
    }

    let (width, height) = match mode {
        "viz" | "viz-matte" => (110, 40),
        "viz-module" => (120, 42),
        "scope" => (100, 36),
        _ => (100, 32),
    };
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).map_err(io::Error::other)?;
    terminal
        .draw(|frame| draw(frame, &mut app))
        .map_err(io::Error::other)?;
    let buffer = terminal.backend().buffer().clone();
    let mut out = fs::File::create(path)?;
    writeln!(out, "{} {}", buffer.area.width, buffer.area.height)?;
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            let symbol = cell.symbol();
            let symbol = if symbol.is_empty() { " " } else { symbol };
            writeln!(out, "{x} {y} {} {} {symbol}", hex(cell.fg), hex(cell.bg))?;
        }
    }
    Ok(())
}

fn seed_viz(app: &mut App) {
    let mut stereo = [0i16; WINDOW * 2];
    for index in 0..WINDOW {
        let t = index as f32 / 44_100.0;
        let left = (t * 220.0 * std::f32::consts::TAU).sin() * 0.55
            + (t * 440.0 * std::f32::consts::TAU).sin() * 0.28
            + (t * 880.0 * std::f32::consts::TAU).sin() * 0.12;
        let right = (t * 220.0 * std::f32::consts::TAU + 0.7).sin() * 0.40
            + (t * 660.0 * std::f32::consts::TAU).sin() * 0.22;
        stereo[index * 2] = (left.clamp(-1.0, 1.0) * 20_000.0) as i16;
        stereo[index * 2 + 1] = (right.clamp(-1.0, 1.0) * 20_000.0) as i16;
    }
    let mut snap = VizSnapshot {
        stereo,
        peaks: [7600, 2800, 5400, 1400],
        rate: 44_100,
        gen: 1,
    };
    for generation in 1..=4 {
        snap.gen = generation;
        app.tick_viz(Some(&snap), 0.05);
    }
}

/// Play `module` from `order`/`row` and fold each analysis window into `app`.
fn feed_module(app: &mut App, module: &Module, order: usize, row: usize, windows: usize) {
    let mut playback = Playback::new(PlayerConfig::default());
    playback.start(module, order, row);
    let rate = playback.sample_rate().max(1);
    let dt = WINDOW as f32 / rate as f32;
    let mut pcm = vec![0i16; WINDOW * 2];
    for generation in 1..=windows {
        let wrote = playback.render(module, &mut pcm);
        if wrote == 0 {
            break;
        }
        let mut stereo = [0i16; WINDOW * 2];
        let copy = wrote.min(WINDOW) * 2;
        stereo[..copy].copy_from_slice(&pcm[..copy]);
        let snap = VizSnapshot {
            stereo,
            peaks: playback.channel_peaks(),
            rate,
            gen: generation as u64,
        };
        app.tick_viz(Some(&snap), dt);
    }
}

fn hex(color: Color) -> String {
    let (r, g, b) = match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::Red => (170, 0, 0),
        Color::Green => (0, 170, 0),
        Color::Yellow => (255, 255, 85),
        Color::Blue => (0, 0, 170),
        Color::Magenta => (170, 0, 170),
        Color::Cyan => (85, 255, 255),
        Color::Gray => (170, 170, 170),
        Color::DarkGray => (85, 85, 85),
        Color::LightRed => (255, 85, 85),
        Color::LightGreen => (85, 255, 85),
        Color::LightYellow => (255, 255, 85),
        Color::LightBlue => (85, 85, 255),
        Color::LightMagenta => (255, 85, 255),
        Color::LightCyan => (85, 255, 255),
        Color::White => (255, 255, 255),
        Color::Indexed(index) => (index, index, index),
        Color::Reset => (0, 0, 0),
    };
    format!("#{r:02x}{g:02x}{b:02x}")
}

const MATTE_BLACK: &str = "\
accent = \"#e68e0d\"\n\
cursor = \"#eaeaea\"\n\
foreground = \"#bebebe\"\n\
background = \"#121212\"\n\
color0 = \"#333333\"\n\
color1 = \"#D35F5F\"\n\
color2 = \"#FFC107\"\n\
color3 = \"#b91c1c\"\n\
color4 = \"#e68e0d\"\n\
color5 = \"#D35F5F\"\n\
color6 = \"#bebebe\"\n\
color7 = \"#bebebe\"\n\
color8 = \"#8a8a8d\"\n\
color9 = \"#B91C1C\"\n\
color10 = \"#FFC107\"\n\
color11 = \"#b90a0a\"\n\
color12 = \"#f59e0b\"\n\
color13 = \"#B91C1C\"\n\
color14 = \"#eaeaea\"\n\
color15 = \"#ffffff\"\n\
";

const TOKYO: &str = "\
background = \"#1a1b26\"\n\
foreground = \"#a9b1d6\"\n\
accent = \"#7aa2f7\"\n\
muted = \"#414868\"\n\
bright_foreground = \"#c0caf5\"\n\
red = \"#f7768e\"\n\
green = \"#9ece6a\"\n\
yellow = \"#e0af68\"\n\
blue = \"#7aa2f7\"\n\
cyan = \"#449dab\"\n\
magenta = \"#ad8ee6\"\n\
lighter_background = \"#24283b\"\n\
";
