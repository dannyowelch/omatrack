//! Tracker colors.
//!
//! Built-in palettes are [`Theme::protracker`] and [`Theme::phosphor`].
//! [`Theme::terminal`] uses ANSI indexes so a themed terminal paints them.
//! [`Theme::from_palette`] maps an Omarchy palette onto the same roles.
//! [`Theme::adapt`] keeps truecolor when the terminal advertises it and
//! otherwise quantizes to the xterm cube or the 16 ANSI colors.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ratatui::style::{Color, Modifier, Style};

use crate::config::ThemeRequest;
use crate::omarchy::{self, Palette, Rgb};

/// Palette for the tracker view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Stable name: `protracker`, `phosphor`, `terminal`, or `omarchy`.
    pub id: &'static str,
    /// Window background.
    pub background: Color,
    /// Primary text.
    pub text: Color,
    /// Empty cells and secondary labels.
    pub dim: Color,
    /// Titles and the focused border.
    pub title: Color,
    /// Note names.
    pub note: Color,
    /// Effect digits.
    pub effect: Color,
    /// Row numbers and column headers.
    pub accent: Color,
    /// Foreground on the cursor.
    pub cursor_fg: Color,
    /// Background on the cursor cell or selected sample.
    pub cursor_bg: Color,
    /// Background on the cursor row.
    pub row_bg: Color,
    /// Background on the row the replayer is currently mixing.
    pub play_bg: Color,
    /// Background on the selected order entry.
    pub order_bg: Color,
    /// Foreground of the edit-mode field cursor.
    pub edit_fg: Color,
    /// Background of the edit-mode field cursor.
    pub edit_bg: Color,
    /// Background of a selected block.
    pub block_bg: Color,
    /// Unfocused pane border.
    pub border: Color,
    /// Focused pane border.
    pub border_focus: Color,
    /// Channel header colors, channel 1 first.
    pub channels: [Color; 4],
    /// Sample waveform, and the scope trace.
    pub waveform: Color,
    /// Spectrum bar body.
    pub spectrum: Color,
    /// Spectrum peak mark and meter peak tick.
    pub spectrum_peak: Color,
    /// Audio and file errors.
    pub error: Color,
}

impl Theme {
    /// Deep blue, in the neighborhood of the ProTracker screen.
    pub const fn protracker() -> Self {
        Self {
            id: "protracker",
            background: Color::Rgb(0, 0, 90),
            text: Color::White,
            dim: Color::Rgb(140, 150, 180),
            title: Color::Yellow,
            note: Color::Rgb(170, 255, 170),
            effect: Color::Rgb(255, 214, 102),
            accent: Color::Cyan,
            cursor_fg: Color::Black,
            cursor_bg: Color::Yellow,
            row_bg: Color::Rgb(24, 36, 150),
            play_bg: Color::Rgb(16, 92, 48),
            order_bg: Color::Rgb(210, 170, 40),
            edit_fg: Color::Black,
            edit_bg: Color::White,
            block_bg: Color::Rgb(72, 48, 140),
            border: Color::Rgb(80, 100, 170),
            border_focus: Color::Yellow,
            channels: [
                Color::Rgb(255, 214, 102),
                Color::Rgb(170, 255, 170),
                Color::Cyan,
                Color::Rgb(255, 160, 196),
            ],
            waveform: Color::Rgb(170, 255, 170),
            spectrum: Color::Rgb(80, 220, 255),
            spectrum_peak: Color::Rgb(255, 214, 102),
            error: Color::Rgb(255, 96, 96),
        }
    }

    /// Green phosphor on a black tube.
    pub const fn phosphor() -> Self {
        Self {
            id: "phosphor",
            background: Color::Rgb(0, 12, 0),
            text: Color::Rgb(166, 255, 166),
            dim: Color::Rgb(48, 120, 64),
            title: Color::Rgb(204, 255, 120),
            note: Color::Rgb(210, 255, 180),
            effect: Color::Rgb(140, 220, 80),
            accent: Color::Rgb(96, 255, 160),
            cursor_fg: Color::Rgb(0, 20, 0),
            cursor_bg: Color::Rgb(170, 255, 80),
            row_bg: Color::Rgb(0, 40, 12),
            play_bg: Color::Rgb(0, 72, 24),
            order_bg: Color::Rgb(190, 255, 90),
            edit_fg: Color::Rgb(0, 16, 0),
            edit_bg: Color::Rgb(230, 255, 220),
            block_bg: Color::Rgb(0, 64, 28),
            border: Color::Rgb(32, 96, 40),
            border_focus: Color::Rgb(150, 255, 110),
            channels: [
                Color::Rgb(220, 255, 140),
                Color::Rgb(120, 255, 170),
                Color::Rgb(80, 220, 255),
                Color::Rgb(255, 220, 120),
            ],
            waveform: Color::Rgb(120, 255, 160),
            spectrum: Color::Rgb(80, 255, 140),
            spectrum_peak: Color::Rgb(230, 255, 160),
            error: Color::Rgb(255, 88, 64),
        }
    }

    /// Named ANSI colors. The terminal theme decides the actual RGB.
    pub const fn terminal() -> Self {
        Self {
            id: "terminal",
            background: Color::Reset,
            text: Color::Reset,
            dim: Color::DarkGray,
            title: Color::Yellow,
            note: Color::Green,
            effect: Color::Yellow,
            accent: Color::Cyan,
            cursor_fg: Color::Black,
            cursor_bg: Color::Yellow,
            row_bg: Color::DarkGray,
            play_bg: Color::Blue,
            order_bg: Color::Yellow,
            edit_fg: Color::Black,
            edit_bg: Color::White,
            block_bg: Color::Magenta,
            border: Color::DarkGray,
            border_focus: Color::Yellow,
            channels: [Color::Yellow, Color::Green, Color::Cyan, Color::Magenta],
            waveform: Color::Cyan,
            spectrum: Color::Cyan,
            spectrum_peak: Color::Yellow,
            error: Color::Red,
        }
    }

    /// Map an Omarchy palette onto the tracker roles.
    ///
    /// Background and foreground are the theme's own. The cursor uses
    /// `bright_foreground`, which is what Omarchy's terminal templates use.
    /// Edit mode uses the accent so it stays distinct from that cursor.
    /// The playback row mixes the background toward green. Channel headers
    /// take red, yellow, green, and blue. The waveform and the spectrum use
    /// cyan, the spectrum peak uses yellow, and errors use red.
    pub fn from_palette(palette: &Palette) -> Self {
        let background = pick(palette, &["background", "color0"], Rgb { r: 0, g: 0, b: 0 });
        let text = pick(
            palette,
            &["foreground", "color7"],
            Rgb {
                r: 220,
                g: 220,
                b: 220,
            },
        );
        let accent = pick(palette, &["accent", "blue", "color4"], text);
        let muted = pick(palette, &["muted", "dark_foreground", "color8"], text);
        let bright = pick(palette, &["bright_foreground", "cursor", "color15"], text);
        let green = pick(palette, &["green", "color2"], accent);
        let yellow = pick(palette, &["yellow", "color3"], accent);
        let red = pick(palette, &["red", "color1"], accent);
        let cyan = pick(palette, &["cyan", "color6"], accent);
        let blue = pick(palette, &["blue", "color4"], accent);
        let magenta = pick(palette, &["magenta", "color5"], accent);
        let lighter = pick(
            palette,
            &["lighter_background", "selection", "color8"],
            omarchy::mix(background, text, 0.12),
        );
        let rgb = color_rgb;
        Self {
            id: "omarchy",
            background: rgb(background),
            text: rgb(text),
            dim: rgb(muted),
            title: rgb(accent),
            note: rgb(green),
            effect: rgb(yellow),
            accent: rgb(cyan),
            cursor_fg: rgb(background),
            cursor_bg: rgb(bright),
            row_bg: rgb(lighter),
            play_bg: rgb(omarchy::mix(background, green, 0.35)),
            order_bg: rgb(accent),
            edit_fg: rgb(background),
            edit_bg: rgb(accent),
            block_bg: rgb(omarchy::mix(background, magenta, 0.45)),
            border: rgb(muted),
            border_focus: rgb(accent),
            channels: [rgb(red), rgb(yellow), rgb(green), rgb(blue)],
            waveform: rgb(cyan),
            spectrum: rgb(cyan),
            spectrum_peak: rgb(yellow),
            error: rgb(red),
        }
    }

    /// Quantize truecolor roles when the terminal cannot show them.
    ///
    /// ANSI colors, including every role of [`Self::terminal`], are left alone
    /// so they keep tracking the terminal theme.
    pub fn adapt(self, depth: ColorDepth) -> Self {
        if depth == ColorDepth::Truecolor {
            return self;
        }
        let map = |color| adapt_color(color, depth);
        Self {
            id: self.id,
            background: map(self.background),
            text: map(self.text),
            dim: map(self.dim),
            title: map(self.title),
            note: map(self.note),
            effect: map(self.effect),
            accent: map(self.accent),
            cursor_fg: map(self.cursor_fg),
            cursor_bg: map(self.cursor_bg),
            row_bg: map(self.row_bg),
            play_bg: map(self.play_bg),
            order_bg: map(self.order_bg),
            edit_fg: map(self.edit_fg),
            edit_bg: map(self.edit_bg),
            block_bg: map(self.block_bg),
            border: map(self.border),
            border_focus: map(self.border_focus),
            channels: self.channels.map(map),
            waveform: map(self.waveform),
            spectrum: map(self.spectrum),
            spectrum_peak: map(self.spectrum_peak),
            error: map(self.error),
        }
    }

    /// Default text on the window background.
    pub fn text(self) -> Style {
        Style::default().fg(self.text).bg(self.background)
    }

    /// Secondary text on the window background.
    pub fn dim(self) -> Style {
        Style::default().fg(self.dim).bg(self.background)
    }

    /// Bold title text on the window background.
    pub fn title(self) -> Style {
        Style::default()
            .fg(self.title)
            .bg(self.background)
            .add_modifier(Modifier::BOLD)
    }

    /// Fill style for pane interiors.
    pub fn fill(self) -> Style {
        self.text()
    }
}

fn pick(palette: &Palette, keys: &[&str], fallback: Rgb) -> Rgb {
    for key in keys {
        if let Some(color) = palette.get(key) {
            return color;
        }
    }
    fallback
}

fn color_rgb(color: Rgb) -> Color {
    Color::Rgb(color.r, color.g, color.b)
}

/// How much color the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// 24-bit `ESC[38;2;…m`.
    Truecolor,
    /// The xterm 256-color cube.
    Ansi256,
    /// The 16 named ANSI colors.
    Ansi16,
}

/// `COLORTERM` and `TERM`, with `OMATRACK_COLOR=truecolor|256|16` as an override.
pub fn detect_color_depth() -> ColorDepth {
    if let Ok(value) = std::env::var("OMATRACK_COLOR") {
        match value.to_ascii_lowercase().as_str() {
            "truecolor" | "24bit" | "true" => return ColorDepth::Truecolor,
            "256" | "ansi256" => return ColorDepth::Ansi256,
            "16" | "ansi" | "ansi16" => return ColorDepth::Ansi16,
            _ => {}
        }
    }
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    let colorterm = colorterm.to_ascii_lowercase();
    if colorterm == "truecolor" || colorterm == "24bit" {
        return ColorDepth::Truecolor;
    }
    let term = std::env::var("TERM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if term.contains("truecolor")
        || term.contains("ghostty")
        || term.contains("alacritty")
        || term.contains("kitty")
    {
        return ColorDepth::Truecolor;
    }
    if term.contains("256color") || term.contains("256colour") {
        return ColorDepth::Ansi256;
    }
    if term.is_empty() || term == "dumb" || term.contains("linux") {
        return ColorDepth::Ansi16;
    }
    ColorDepth::Ansi16
}

/// A palette plus the paths whose change should repaint it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedTheme {
    /// Colors to draw with.
    pub theme: Theme,
    /// Shown in the song header. An Omarchy `theme.name`, or the built-in id.
    pub label: String,
    /// Files to stat for a live reload.
    pub watch: ThemeWatch,
    /// Set when a requested Omarchy theme could not be read.
    pub warning: Option<String>,
}

/// mtimes captured the last time the palette was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeWatch {
    /// Theme file, theme directory, and `theme.name`.
    pub paths: Vec<PathBuf>,
    stamps: Vec<Option<SystemTime>>,
}

impl ThemeWatch {
    /// Remember the current mtimes. Missing paths are stored as absent.
    pub fn new(paths: Vec<PathBuf>) -> Self {
        let stamps = paths.iter().map(|path| file_stamp(path)).collect();
        Self { paths, stamps }
    }

    /// True when a watched path appeared, disappeared, or changed mtime.
    pub fn changed(&self) -> bool {
        self.paths
            .iter()
            .zip(&self.stamps)
            .any(|(path, stamp)| file_stamp(path) != *stamp)
    }
}

fn file_stamp(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// Resolve `request`, quantizing to `depth`.
///
/// `auto` and `omarchy` read the active Omarchy theme. Anything else is a
/// built-in palette. A missing Omarchy theme falls back to ProTracker; only
/// an explicit `omarchy` request reports that.
pub fn resolve(
    request: ThemeRequest,
    depth: ColorDepth,
    home: &Path,
    xdg_state_home: Option<&Path>,
) -> LoadedTheme {
    match request {
        ThemeRequest::Protracker => builtin(Theme::protracker(), depth),
        ThemeRequest::Phosphor => builtin(Theme::phosphor(), depth),
        ThemeRequest::Terminal => builtin(Theme::terminal(), depth),
        ThemeRequest::Omarchy | ThemeRequest::Auto => {
            let found = omarchy::discover(home, xdg_state_home);
            match found {
                Some(found) => LoadedTheme {
                    theme: Theme::from_palette(&found.palette).adapt(depth),
                    label: found.label,
                    watch: ThemeWatch::new(found.watch),
                    warning: None,
                },
                None => {
                    let warning = if request == ThemeRequest::Omarchy {
                        Some(
                            "No Omarchy theme found under ~/.local/state/omarchy/current/theme or ~/.config/omarchy/current/theme; using protracker."
                                .to_string(),
                        )
                    } else {
                        None
                    };
                    LoadedTheme {
                        theme: Theme::protracker().adapt(depth),
                        label: "protracker".to_string(),
                        watch: ThemeWatch::new(omarchy::candidate_dirs(home, xdg_state_home)),
                        warning,
                    }
                }
            }
        }
    }
}

fn builtin(theme: Theme, depth: ColorDepth) -> LoadedTheme {
    LoadedTheme {
        label: theme.id.to_string(),
        theme: theme.adapt(depth),
        watch: ThemeWatch::new(Vec::new()),
        warning: None,
    }
}

pub(crate) fn paint(fg: Color, bg: Color, bold: bool) -> Style {
    let style = Style::default().fg(fg).bg(bg);
    if bold {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn adapt_color(color: Color, depth: ColorDepth) -> Color {
    let Color::Rgb(red, green, blue) = color else {
        return color;
    };
    match depth {
        ColorDepth::Truecolor => color,
        ColorDepth::Ansi256 => Color::Indexed(rgb_to_ansi256(red, green, blue)),
        ColorDepth::Ansi16 => nearest_ansi16(red, green, blue),
    }
}

fn rgb_to_ansi256(red: u8, green: u8, blue: u8) -> u8 {
    let cube = |value: u8| -> i32 {
        let value = i32::from(value);
        if value < 48 {
            0
        } else if value < 114 {
            1
        } else {
            ((value - 35) / 40).min(5)
        }
    };
    let steps = [0, 95, 135, 175, 215, 255];
    let ir = cube(red);
    let ig = cube(green);
    let ib = cube(blue);
    let cr = steps[ir as usize];
    let cg = steps[ig as usize];
    let cb = steps[ib as usize];
    let cube_dist = dist(red, green, blue, cr, cg, cb);
    let average = (i32::from(red) + i32::from(green) + i32::from(blue)) / 3;
    let gray_level = if average < 8 {
        0
    } else {
        ((average - 8) / 10).min(23)
    };
    let gray_value = 8 + 10 * gray_level;
    let gray_dist = dist(red, green, blue, gray_value, gray_value, gray_value);
    if gray_dist < cube_dist {
        (232 + gray_level) as u8
    } else {
        (16 + 36 * ir + 6 * ig + ib) as u8
    }
}

fn dist(r: u8, g: u8, b: u8, cr: i32, cg: i32, cb: i32) -> i32 {
    let dr = i32::from(r) - cr;
    let dg = i32::from(g) - cg;
    let db = i32::from(b) - cb;
    dr * dr + dg * dg + db * db
}

fn nearest_ansi16(red: u8, green: u8, blue: u8) -> Color {
    const ANSI: [(Color, u8, u8, u8); 16] = [
        (Color::Black, 0, 0, 0),
        (Color::Red, 170, 0, 0),
        (Color::Green, 0, 170, 0),
        (Color::Yellow, 170, 170, 0),
        (Color::Blue, 0, 0, 170),
        (Color::Magenta, 170, 0, 170),
        (Color::Cyan, 0, 170, 170),
        (Color::Gray, 170, 170, 170),
        (Color::DarkGray, 85, 85, 85),
        (Color::LightRed, 255, 85, 85),
        (Color::LightGreen, 85, 255, 85),
        (Color::LightYellow, 255, 255, 85),
        (Color::LightBlue, 85, 85, 255),
        (Color::LightMagenta, 255, 85, 255),
        (Color::LightCyan, 85, 255, 255),
        (Color::White, 255, 255, 255),
    ];
    ANSI.into_iter()
        .min_by_key(|(_, r, g, b)| {
            dist(
                red,
                green,
                blue,
                i32::from(*r),
                i32::from(*g),
                i32::from(*b),
            )
        })
        .map(|(color, _, _, _)| color)
        .unwrap_or(Color::White)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThemeRequest;

    #[test]
    fn omarchy_roles_follow_the_palette() {
        let palette = omarchy::palette_from_colors_toml(
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
        .unwrap();
        let theme = Theme::from_palette(&palette);
        assert_eq!(theme.background, Color::Rgb(0x1a, 0x1b, 0x26));
        assert_eq!(theme.text, Color::Rgb(0xa9, 0xb1, 0xd6));
        assert_eq!(theme.cursor_bg, Color::Rgb(0xc0, 0xca, 0xf5));
        assert_eq!(theme.cursor_fg, theme.background);
        assert_eq!(theme.edit_bg, Color::Rgb(0x7a, 0xa2, 0xf7));
        assert_eq!(theme.error, Color::Rgb(0xf7, 0x76, 0x8e));
        assert_eq!(theme.waveform, Color::Rgb(0x44, 0x9d, 0xab));
        assert_eq!(theme.channels[0], theme.error);
        assert_eq!(theme.note, Color::Rgb(0x9e, 0xce, 0x6a));
        assert_ne!(theme.play_bg, theme.background);
    }

    #[test]
    fn explicit_omarchy_without_a_theme_falls_back_and_auto_stays_quiet() {
        let home = std::env::temp_dir().join(format!("omatrack-notheme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let forced = resolve(ThemeRequest::Omarchy, ColorDepth::Truecolor, &home, None);
        assert_eq!(forced.theme.id, "protracker");
        assert!(forced.warning.is_some());
        let auto = resolve(ThemeRequest::Auto, ColorDepth::Truecolor, &home, None);
        assert_eq!(auto.theme.id, "protracker");
        assert!(auto.warning.is_none());
        assert!(!auto.watch.paths.is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn ansi_adaptation_leaves_named_colors_and_quantizes_rgb() {
        let terminal = Theme::terminal().adapt(ColorDepth::Ansi16);
        assert_eq!(terminal.background, Color::Reset);
        assert_eq!(terminal.cursor_bg, Color::Yellow);
        let adapted = Theme::protracker().adapt(ColorDepth::Ansi256);
        assert!(matches!(adapted.background, Color::Indexed(_)));
        let sixteen = Theme::protracker().adapt(ColorDepth::Ansi16);
        assert!(matches!(
            sixteen.cursor_bg,
            Color::Yellow | Color::LightYellow
        ));
    }

    #[test]
    fn a_changed_mtime_is_visible_to_the_watch() {
        let path = std::env::temp_dir().join(format!("omatrack-watch-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let watch = ThemeWatch::new(vec![path.clone()]);
        assert!(!watch.changed());
        std::fs::write(&path, "background = \"#000000\"\n").unwrap();
        assert!(watch.changed());
        let watch = ThemeWatch::new(vec![path.clone()]);
        assert!(!watch.changed());
        let _ = std::fs::remove_file(&path);
    }
}
