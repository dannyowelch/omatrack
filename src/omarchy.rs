//! The active Omarchy palette.
//!
//! Current Omarchy (the theming documented in basecamp/omarchy `docs/theming.md`)
//! writes the active theme to `$XDG_STATE_HOME/omarchy/current/theme`, which is
//! `~/.local/state/omarchy/current/theme` when `XDG_STATE_HOME` is unset. The
//! canonical palette is `colors.toml`. `omarchy-theme-set` used to publish the
//! same directory at `~/.config/omarchy/current/theme`, and older themes may
//! only have a rendered terminal config (`alacritty.toml`, `kitty.conf`, or
//! `ghostty.conf`) instead of `colors.toml`. This module checks the state
//! directory first, then the config directory, and reads those files in that
//! order.
//!
//! Color keys follow `omarchy-theme-color`: canonical names win over the legacy
//! short names (`bg`, `fg`, …), ANSI `color0`–`color15` fill any semantic name
//! that is still empty, and missing ramps are mixed from the bases.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::{parse_table, strip_comment};

/// One sRGB color from a theme file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    /// Red, 0..=255.
    pub r: u8,
    /// Green, 0..=255.
    pub g: u8,
    /// Blue, 0..=255.
    pub b: u8,
}

impl Rgb {
    /// `#rrggbb`.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// Resolved palette. Missing keys stay missing; derived keys are filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    colors: BTreeMap<String, Rgb>,
}

impl Palette {
    /// The color stored for `key`, after aliases and mixes.
    pub fn get(&self, key: &str) -> Option<Rgb> {
        self.colors.get(key).copied()
    }
}

/// A theme directory that produced a palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundTheme {
    /// Resolved colors.
    pub palette: Palette,
    /// `theme.name` when Omarchy wrote one, otherwise `omarchy`.
    pub label: String,
    /// Files and directories whose mtime should be watched.
    pub watch: Vec<PathBuf>,
}

/// Directories that may hold the active theme, newest layout first.
pub fn candidate_dirs(home: &Path, xdg_state_home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(state) = xdg_state_home {
        if !state.as_os_str().is_empty() {
            dirs.push(state.join("omarchy").join("current").join("theme"));
        }
    }
    dirs.push(
        home.join(".local")
            .join("state")
            .join("omarchy")
            .join("current")
            .join("theme"),
    );
    dirs.push(
        home.join(".config")
            .join("omarchy")
            .join("current")
            .join("theme"),
    );
    dirs.dedup();
    dirs
}

/// The first candidate directory that contains a readable palette.
pub fn discover(home: &Path, xdg_state_home: Option<&Path>) -> Option<FoundTheme> {
    candidate_dirs(home, xdg_state_home)
        .into_iter()
        .find_map(|dir| load_theme_dir(&dir))
}

/// Read `colors.toml`, or a rendered terminal config, from one theme directory.
pub fn load_theme_dir(dir: &Path) -> Option<FoundTheme> {
    let files = [
        ("colors.toml", Kind::Colors),
        ("alacritty.toml", Kind::Alacritty),
        ("kitty.conf", Kind::Kitty),
        ("ghostty.conf", Kind::Ghostty),
    ];
    for (name, kind) in files {
        let path = dir.join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(palette) = palette_from_kind(&text, kind) else {
            continue;
        };
        return Some(FoundTheme {
            palette,
            label: theme_label(dir),
            watch: watch_paths(dir, &path),
        });
    }
    None
}

#[derive(Clone, Copy)]
enum Kind {
    Colors,
    Alacritty,
    Kitty,
    Ghostty,
}

fn palette_from_kind(text: &str, kind: Kind) -> Option<Palette> {
    match kind {
        Kind::Colors => palette_from_colors_toml(text),
        Kind::Alacritty => palette_from_alacritty(text),
        Kind::Kitty => palette_from_kitty(text),
        Kind::Ghostty => palette_from_ghostty(text),
    }
}

/// Resolve a `colors.toml` document.
pub fn palette_from_colors_toml(text: &str) -> Option<Palette> {
    let raw = parse_table(text).ok()?;
    finish(raw)
}

/// Resolve an Alacritty config that uses Omarchy's `[colors.*]` tables.
pub fn palette_from_alacritty(text: &str) -> Option<Palette> {
    let table = parse_table(text).ok()?;
    let mut raw = BTreeMap::new();
    copy(&table, &mut raw, "background", "colors.primary.background");
    copy(&table, &mut raw, "foreground", "colors.primary.foreground");
    copy(
        &table,
        &mut raw,
        "bright_foreground",
        "colors.cursor.cursor",
    );
    copy(
        &table,
        &mut raw,
        "selection_background",
        "colors.selection.background",
    );
    copy(
        &table,
        &mut raw,
        "selection_foreground",
        "colors.selection.text",
    );
    const NORMAL: &[(&str, &str)] = &[
        ("color0", "colors.normal.black"),
        ("color1", "colors.normal.red"),
        ("color2", "colors.normal.green"),
        ("color3", "colors.normal.yellow"),
        ("color4", "colors.normal.blue"),
        ("color5", "colors.normal.magenta"),
        ("color6", "colors.normal.cyan"),
        ("color7", "colors.normal.white"),
    ];
    const BRIGHT: &[(&str, &str)] = &[
        ("color8", "colors.bright.black"),
        ("color9", "colors.bright.red"),
        ("color10", "colors.bright.green"),
        ("color11", "colors.bright.yellow"),
        ("color12", "colors.bright.blue"),
        ("color13", "colors.bright.magenta"),
        ("color14", "colors.bright.cyan"),
        ("color15", "colors.bright.white"),
    ];
    for (dest, src) in NORMAL.iter().chain(BRIGHT) {
        copy(&table, &mut raw, dest, src);
    }
    finish(raw)
}

/// Resolve a Kitty theme (`foreground #rrggbb`, `color0 #rrggbb`).
pub fn palette_from_kitty(text: &str) -> Option<Palette> {
    let mut raw = BTreeMap::new();
    for line in text.lines() {
        let line = strip_comment(line).trim();
        if line.is_empty() || line.starts_with("include") {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else { continue };
        let Some(value) = parts.next() else { continue };
        let dest = match key {
            "background" => "background",
            "foreground" => "foreground",
            "cursor" => "bright_foreground",
            "selection_background" => "selection_background",
            "selection_foreground" => "selection_foreground",
            other if other.starts_with("color") => other,
            _ => continue,
        };
        raw.insert(dest.to_string(), value.to_string());
    }
    finish(raw)
}

/// Resolve a Ghostty theme (`background = #rrggbb`, `palette = 1=#rrggbb`).
pub fn palette_from_ghostty(text: &str) -> Option<Palette> {
    let mut raw = BTreeMap::new();
    for line in text.lines() {
        let line = strip_comment(line).trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if key == "palette" {
            let Some((index, color)) = value.split_once('=') else {
                continue;
            };
            let index = index.trim();
            if index.chars().all(|ch| ch.is_ascii_digit()) {
                raw.insert(format!("color{index}"), color.trim().to_string());
            }
            continue;
        }
        let dest = match key {
            "background" => "background",
            "foreground" => "foreground",
            "cursor-color" => "bright_foreground",
            "selection-background" => "selection_background",
            "selection-foreground" => "selection_foreground",
            _ => continue,
        };
        raw.insert(dest.to_string(), value.to_string());
    }
    finish(raw)
}

fn copy(
    table: &BTreeMap<String, String>,
    raw: &mut BTreeMap<String, String>,
    dest: &str,
    src: &str,
) {
    if let Some(value) = table.get(src) {
        raw.insert(dest.to_string(), value.clone());
    }
}

fn finish(mut raw: BTreeMap<String, String>) -> Option<Palette> {
    resolve_raw(&mut raw);
    let mut colors = BTreeMap::new();
    for (key, value) in &raw {
        if let Some(color) = parse_color(value) {
            colors.insert(key.clone(), color);
        }
    }
    if colors.contains_key("background") || colors.contains_key("foreground") {
        Some(Palette { colors })
    } else {
        None
    }
}

/// Alias and derivation cascade from `bin/omarchy-theme-color`.
fn resolve_raw(raw: &mut BTreeMap<String, String>) {
    const LEGACY: &[(&str, &str)] = &[
        ("background", "bg"),
        ("dark_background", "dark_bg"),
        ("darker_background", "darker_bg"),
        ("lighter_background", "lighter_bg"),
        ("foreground", "fg"),
        ("dark_foreground", "dark_fg"),
        ("light_foreground", "light_fg"),
        ("bright_foreground", "bright_fg"),
    ];
    for (canonical, legacy) in LEGACY {
        alias(raw, canonical, legacy);
    }

    alias(raw, "background", "color0");
    alias(raw, "foreground", "color7");
    if raw.contains_key("background") {
        alias_overwrite(raw, "color0", "background");
    }
    if raw.contains_key("foreground") {
        alias_overwrite(raw, "color7", "foreground");
    }

    const ANSI_TO_NAME: &[(&str, &str)] = &[
        ("red", "color1"),
        ("green", "color2"),
        ("yellow", "color3"),
        ("blue", "color4"),
        ("magenta", "color5"),
        ("cyan", "color6"),
        ("bright_red", "color9"),
        ("bright_green", "color10"),
        ("bright_yellow", "color11"),
        ("bright_blue", "color12"),
        ("bright_magenta", "color13"),
        ("bright_cyan", "color14"),
    ];
    for (name, ansi) in ANSI_TO_NAME {
        alias(raw, name, ansi);
    }
    alias(raw, "magenta", "purple");
    alias(raw, "bright_magenta", "bright_purple");

    alias(raw, "light_foreground", "color7");
    alias(raw, "light_foreground", "foreground");
    alias(raw, "bright_foreground", "color15");
    alias(raw, "bright_foreground", "foreground");
    if let Some(cursor) = raw.get("bright_foreground").cloned() {
        raw.insert("cursor".to_string(), cursor);
    }
    alias(raw, "lighter_background", "color0");
    alias(raw, "lighter_background", "background");
    alias(raw, "dark_foreground", "color8");
    alias(raw, "dark_foreground", "foreground");
    alias(raw, "muted", "color8");
    alias(raw, "muted", "dark_foreground");
    alias(raw, "selection", "selection_background");
    alias(raw, "selection", "color8");
    alias(raw, "selection", "color0");
    alias(raw, "selection", "background");
    alias(raw, "selection_background", "selection");
    alias(raw, "selection_foreground", "bright_foreground");
    alias(raw, "orange", "yellow");
    mix_missing(raw, "brown", "orange", "#000000", 0.5);
    mix_missing(raw, "dark_background", "background", "#000000", 0.25);
    mix_missing(raw, "darker_background", "background", "#000000", 0.5);
    mix_missing(raw, "bright_red", "red", "#ffffff", 0.2);
    mix_missing(raw, "bright_yellow", "yellow", "#ffffff", 0.2);
    mix_missing(raw, "bright_green", "green", "#ffffff", 0.2);
    mix_missing(raw, "bright_cyan", "cyan", "#ffffff", 0.2);
    mix_missing(raw, "bright_blue", "blue", "#ffffff", 0.2);
    mix_missing(raw, "bright_magenta", "magenta", "#ffffff", 0.2);
    alias(raw, "purple", "magenta");
    alias(raw, "bright_purple", "bright_magenta");

    const NAME_TO_ANSI: &[(&str, &str)] = &[
        ("color0", "background"),
        ("color1", "red"),
        ("color2", "green"),
        ("color3", "yellow"),
        ("color4", "blue"),
        ("color5", "magenta"),
        ("color6", "cyan"),
        ("color7", "foreground"),
        ("color8", "muted"),
        ("color9", "bright_red"),
        ("color10", "bright_green"),
        ("color11", "bright_yellow"),
        ("color12", "bright_blue"),
        ("color13", "bright_magenta"),
        ("color14", "bright_cyan"),
        ("color15", "bright_foreground"),
    ];
    for (ansi, name) in NAME_TO_ANSI {
        alias(raw, ansi, name);
    }
    for (canonical, legacy) in LEGACY {
        alias_overwrite(raw, legacy, canonical);
    }
    alias(raw, "accent", "blue");
    alias(raw, "accent", "color4");
}

fn alias(raw: &mut BTreeMap<String, String>, key: &str, fallback: &str) {
    if present(raw, key) {
        return;
    }
    if let Some(value) = raw.get(fallback).cloned() {
        if !value.is_empty() {
            raw.insert(key.to_string(), value);
        }
    }
}

fn alias_overwrite(raw: &mut BTreeMap<String, String>, key: &str, source: &str) {
    if let Some(value) = raw.get(source).cloned() {
        if !value.is_empty() {
            raw.insert(key.to_string(), value);
        }
    }
}

fn present(raw: &BTreeMap<String, String>, key: &str) -> bool {
    raw.get(key).is_some_and(|value| !value.is_empty())
}

fn mix_missing(raw: &mut BTreeMap<String, String>, key: &str, start: &str, end: &str, amount: f32) {
    if present(raw, key) {
        return;
    }
    let Some(start) = raw.get(start).cloned() else {
        return;
    };
    let Some(start) = parse_color(&start) else {
        return;
    };
    let Some(end) = parse_color(end).or_else(|| raw.get(end).and_then(|value| parse_color(value)))
    else {
        return;
    };
    raw.insert(key.to_string(), mix(start, end, amount).to_hex());
}

/// Blend `start` toward `end`. `amount` is 0..=1.
pub fn mix(start: Rgb, end: Rgb, amount: f32) -> Rgb {
    let amount = amount.clamp(0.0, 1.0);
    let channel = |from: u8, to: u8| -> u8 {
        let value = f32::from(from) * (1.0 - amount) + f32::from(to) * amount;
        value.round().clamp(0.0, 255.0) as u8
    };
    Rgb {
        r: channel(start.r, end.r),
        g: channel(start.g, end.g),
        b: channel(start.b, end.b),
    }
}

/// `#rgb`, `#rrggbb`, `#rrggbbaa`, or `rgb(r, g, b)`.
pub fn parse_color(text: &str) -> Option<Rgb> {
    let text = text.trim().trim_matches('"').trim_matches('\'');
    if let Some(hex) = text.strip_prefix('#') {
        return parse_hex(hex);
    }
    if text.len() == 6 && text.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return parse_hex(text);
    }
    let lower = text.to_ascii_lowercase();
    let inner = lower
        .strip_prefix("rgb(")
        .or_else(|| lower.strip_prefix("rgba("))?
        .strip_suffix(')')?;
    let parts: Vec<&str> = inner
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() < 3 {
        return None;
    }
    Some(Rgb {
        r: parse_channel(parts[0])?,
        g: parse_channel(parts[1])?,
        b: parse_channel(parts[2])?,
    })
}

fn parse_channel(text: &str) -> Option<u8> {
    if let Some(percent) = text.strip_suffix('%') {
        let value = percent.parse::<f32>().ok()?;
        return Some((value / 100.0 * 255.0).round().clamp(0.0, 255.0) as u8);
    }
    text.parse::<u8>().ok()
}

fn parse_hex(hex: &str) -> Option<Rgb> {
    let byte = |text: &str| u8::from_str_radix(text, 16).ok();
    let nibble = |ch: u8| {
        let value = byte(std::str::from_utf8(&[ch]).ok()?)?;
        Some(value * 16 + value)
    };
    match hex.len() {
        3 | 4 => Some(Rgb {
            r: nibble(hex.as_bytes()[0])?,
            g: nibble(hex.as_bytes()[1])?,
            b: nibble(hex.as_bytes()[2])?,
        }),
        6 | 8 => Some(Rgb {
            r: byte(&hex[0..2])?,
            g: byte(&hex[2..4])?,
            b: byte(&hex[4..6])?,
        }),
        _ => None,
    }
}

fn theme_label(dir: &Path) -> String {
    let name_file = dir.parent().map(|parent| parent.join("theme.name"));
    if let Some(path) = name_file {
        if let Ok(text) = std::fs::read_to_string(path) {
            let label = text.lines().next().unwrap_or("").trim();
            if !label.is_empty() {
                return label.to_string();
            }
        }
    }
    "omarchy".to_string()
}

fn watch_paths(dir: &Path, source: &Path) -> Vec<PathBuf> {
    let mut paths = vec![source.to_path_buf(), dir.to_path_buf()];
    if let Some(parent) = dir.parent() {
        paths.push(parent.join("theme.name"));
        paths.push(parent.to_path_buf());
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKYO: &str = r##"
mode = "dark"
accent = "#7aa2f7"
selection = "#292e42"
muted = "#414868"
background = "#1a1b26"
dark_background = "#13141c"
darker_background = "#0e0e14"
lighter_background = "#24283b"
foreground = "#a9b1d6"
dark_foreground = "#565f89"
light_foreground = "#b4bee6"
bright_foreground = "#c0caf5"
red = "#f7768e"
yellow = "#e0af68"
green = "#9ece6a"
cyan = "#449dab"
blue = "#7aa2f7"
magenta = "#ad8ee6"
"##;

    #[test]
    fn tokyo_night_colors_toml_keeps_the_canonical_palette() {
        let palette = palette_from_colors_toml(TOKYO).expect("palette");
        assert_eq!(palette.get("background").unwrap().to_hex(), "#1a1b26");
        assert_eq!(palette.get("foreground").unwrap().to_hex(), "#a9b1d6");
        assert_eq!(palette.get("accent").unwrap().to_hex(), "#7aa2f7");
        assert_eq!(
            palette.get("bright_foreground").unwrap().to_hex(),
            "#c0caf5"
        );
        assert_eq!(palette.get("red").unwrap().to_hex(), "#f7768e");
        assert_eq!(palette.get("green").unwrap().to_hex(), "#9ece6a");
        assert_eq!(palette.get("color1").unwrap().to_hex(), "#f7768e");
        assert_eq!(palette.get("cursor").unwrap().to_hex(), "#c0caf5");
        assert_eq!(palette.get("bg").unwrap().to_hex(), "#1a1b26");
    }

    #[test]
    fn legacy_short_names_and_ansi_fill_the_gaps() {
        let palette = palette_from_colors_toml(
            "bg = \"#111111\"\nfg = \"#eeeeee\"\ncolor1 = \"#ff0000\"\ncolor2 = \"#00ff00\"\ncolor4 = \"#0000ff\"\n",
        )
        .expect("palette");
        assert_eq!(palette.get("background").unwrap().to_hex(), "#111111");
        assert_eq!(palette.get("foreground").unwrap().to_hex(), "#eeeeee");
        assert_eq!(palette.get("red").unwrap().to_hex(), "#ff0000");
        assert_eq!(palette.get("green").unwrap().to_hex(), "#00ff00");
        assert_eq!(palette.get("blue").unwrap().to_hex(), "#0000ff");
        assert_eq!(palette.get("accent").unwrap().to_hex(), "#0000ff");
        let bright = palette.get("bright_red").unwrap();
        assert!(bright.r > 250 && bright.g > 40, "{bright:?}");
        assert!(palette.get("dark_background").is_some());
    }

    #[test]
    fn terminal_configs_map_onto_the_same_roles() {
        let alacritty = r##"
[colors.primary]
background = "#1a1b26"
foreground = "#a9b1d6"
[colors.cursor]
text = "#1a1b26"
cursor = "#c0caf5"
[colors.normal]
black = "#1a1b26"
red = "#f7768e"
green = "#9ece6a"
yellow = "#e0af68"
blue = "#7aa2f7"
magenta = "#ad8ee6"
cyan = "#449dab"
white = "#a9b1d6"
[colors.bright]
black = "#414868"
white = "#c0caf5"
"##;
        let from_alacritty = palette_from_alacritty(alacritty).unwrap();
        assert_eq!(
            from_alacritty.get("background").unwrap().to_hex(),
            "#1a1b26"
        );
        assert_eq!(
            from_alacritty.get("bright_foreground").unwrap().to_hex(),
            "#c0caf5"
        );
        assert_eq!(from_alacritty.get("red").unwrap().to_hex(), "#f7768e");
        assert_eq!(from_alacritty.get("muted").unwrap().to_hex(), "#414868");

        let kitty = "\
background #101010\n\
foreground #f0f0f0\n\
cursor #ffffff\n\
color1 #aa0000\n\
color2 #00aa00\n";
        let from_kitty = palette_from_kitty(kitty).unwrap();
        assert_eq!(from_kitty.get("background").unwrap().to_hex(), "#101010");
        assert_eq!(from_kitty.get("cursor").unwrap().to_hex(), "#ffffff");
        assert_eq!(from_kitty.get("green").unwrap().to_hex(), "#00aa00");

        let ghostty = "\
background = #202020\n\
foreground = #dddddd\n\
cursor-color = #ffff00\n\
palette = 1=#cc0000\n\
palette = 4=#0000cc\n";
        let from_ghostty = palette_from_ghostty(ghostty).unwrap();
        assert_eq!(from_ghostty.get("background").unwrap().to_hex(), "#202020");
        assert_eq!(from_ghostty.get("accent").unwrap().to_hex(), "#0000cc");
        assert_eq!(from_ghostty.get("red").unwrap().to_hex(), "#cc0000");
        assert_eq!(
            from_ghostty.get("bright_foreground").unwrap().to_hex(),
            "#ffff00"
        );
    }

    #[test]
    fn state_dir_wins_over_the_older_config_dir() {
        let root = std::env::temp_dir().join(format!("omatrack-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let state = root.join(".local/state/omarchy/current/theme");
        let config = root.join(".config/omarchy/current/theme");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            state.join("colors.toml"),
            "background = \"#010101\"\nforeground = \"#fefefe\"\n",
        )
        .unwrap();
        std::fs::write(state.parent().unwrap().join("theme.name"), "tokyo-night\n").unwrap();
        std::fs::write(
            config.join("colors.toml"),
            "background = \"#ffffff\"\nforeground = \"#000000\"\n",
        )
        .unwrap();

        let found = discover(&root, None).expect("theme");
        assert_eq!(found.label, "tokyo-night");
        assert_eq!(found.palette.get("background").unwrap().to_hex(), "#010101");

        std::fs::remove_dir_all(state.parent().unwrap().parent().unwrap().parent().unwrap())
            .unwrap();
        let found = discover(&root, None).expect("legacy");
        assert_eq!(found.palette.get("background").unwrap().to_hex(), "#ffffff");
        assert_eq!(found.label, "omarchy");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_directory_without_colors_falls_through_to_the_terminal_file() {
        let root = std::env::temp_dir().join(format!("omatrack-kitty-{}", std::process::id()));
        let dir = root.join("theme");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("kitty.conf"),
            "background #123456\nforeground #abcdef\n",
        )
        .unwrap();
        let found = load_theme_dir(&dir).expect("kitty");
        assert_eq!(found.palette.get("background").unwrap().to_hex(), "#123456");
        let _ = std::fs::remove_dir_all(&root);
    }
}
