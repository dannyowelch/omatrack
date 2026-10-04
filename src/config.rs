//! `~/.config/omatrack/config.toml`.
//!
//! A missing file uses the defaults. A file that is not valid TOML, or that
//! this parser cannot read, also uses the defaults and carries a warning.
//! One bad value keeps the other settings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::edit::{clamp_octave, clamp_step, DEFAULT_OCTAVE, DEFAULT_STEP};
use crate::player::{Interpolation, PlayerConfig, DEFAULT_SAMPLE_RATE};
use crate::viz::VizMode;

/// Safety cap for `--render` and the interactive song render, in seconds.
pub const DEFAULT_MAX_SECONDS: f64 = 600.0;

/// Which palette to paint with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeRequest {
    /// Read the active Omarchy theme, or use [`Self::Protracker`] when it is absent.
    Auto,
    /// Read the active Omarchy theme. The ProTracker palette is the fallback.
    Omarchy,
    /// Classic ProTracker blue.
    Protracker,
    /// Green phosphor on black.
    Phosphor,
    /// ANSI colors, so the terminal's own theme shows through.
    Terminal,
}

impl ThemeRequest {
    /// `auto`, `omarchy`, `protracker`, `phosphor`, or `terminal`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "omarchy" => Some(Self::Omarchy),
            "protracker" | "classic" => Some(Self::Protracker),
            "phosphor" | "amber" => Some(Self::Phosphor),
            "terminal" | "ansi" => Some(Self::Terminal),
            _ => None,
        }
    }

    /// Name used by `--theme` and the config file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Omarchy => "omarchy",
            Self::Protracker => "protracker",
            Self::Phosphor => "phosphor",
            Self::Terminal => "terminal",
        }
    }
}

/// Settings that survive a restart.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Palette request.
    pub theme: ThemeRequest,
    /// Mixer knobs. Live playback still follows the device rate when it must.
    pub audio: PlayerConfig,
    /// `--render` and Ctrl-G stop here when the song does not loop.
    pub max_seconds: f64,
    /// Piano octave, 1..=3.
    pub octave: u8,
    /// Rows to advance after a note, 0..=16.
    pub step: u8,
    /// Load the last module when the command line does not name one.
    pub reopen_last: bool,
    /// Visualization shown when the tracker opens.
    pub default_view: VizMode,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeRequest::Auto,
            audio: PlayerConfig::default(),
            max_seconds: DEFAULT_MAX_SECONDS,
            octave: DEFAULT_OCTAVE,
            step: DEFAULT_STEP,
            reopen_last: true,
            default_view: VizMode::Panel,
        }
    }
}

impl Config {
    /// The same text as `packaging/config.toml`, which is the file the package installs.
    pub fn documented() -> &'static str {
        include_str!("../packaging/config.toml")
    }
}

/// A config plus any reason the file was not used as written.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig {
    /// Settings to run with.
    pub config: Config,
    /// Why a file was skipped or a value was ignored. `None` when everything was fine.
    pub warning: Option<String>,
}

/// `$XDG_CONFIG_HOME/omatrack/config.toml`, or `~/.config/omatrack/config.toml`.
pub fn default_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("omatrack").join("config.toml");
        }
    }
    home_dir()
        .join(".config")
        .join("omatrack")
        .join("config.toml")
}

/// `$HOME`, or `.` when it is unset.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `$XDG_STATE_HOME` when it is set and not empty.
pub fn xdg_state_home() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// Read `path`. A missing file is silent when `missing_ok` is set.
pub fn load(path: &Path, missing_ok: bool) -> LoadedConfig {
    match std::fs::read_to_string(path) {
        Ok(text) => match parse_text(&text) {
            Ok(parsed) => LoadedConfig {
                config: parsed.config,
                warning: parsed
                    .warning
                    .map(|warning| format!("{}: {warning}", path.display())),
            },
            Err(err) => LoadedConfig {
                config: Config::default(),
                warning: Some(format!(
                    "{} is not a valid config ({err}); using defaults",
                    path.display()
                )),
            },
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound && missing_ok => LoadedConfig {
            config: Config::default(),
            warning: None,
        },
        Err(err) => LoadedConfig {
            config: Config::default(),
            warning: Some(format!(
                "could not read {} ({err}); using defaults",
                path.display()
            )),
        },
    }
}

#[derive(Debug)]
pub(crate) struct Parsed {
    config: Config,
    warning: Option<String>,
}

/// Parse the subset of TOML the config file uses.
pub(crate) fn parse_text(text: &str) -> Result<Parsed, String> {
    let table = parse_table(text)?;
    let mut config = Config::default();
    let mut warnings = Vec::new();

    if let Some(theme) = table.get("theme") {
        match ThemeRequest::parse(theme) {
            Some(theme) => config.theme = theme,
            None => warnings.push(format!(
                "theme {theme:?} is unknown; want auto, omarchy, protracker, phosphor, or terminal"
            )),
        }
    }

    if let Some(rate) = table.get("audio.sample_rate") {
        match rate.parse::<u32>().ok().filter(|rate| *rate > 0) {
            Some(rate) => config.audio.sample_rate = rate,
            None => warnings.push(format!(
                "audio.sample_rate must be above 0, got {rate}; keeping {DEFAULT_SAMPLE_RATE}"
            )),
        }
    }

    if let Some(mode) = table.get("audio.interpolation") {
        match mode.trim() {
            "linear" => config.audio.interpolation = Interpolation::Linear,
            "nearest" => config.audio.interpolation = Interpolation::Nearest,
            other => warnings.push(format!(
                "audio.interpolation must be linear or nearest, got {other}"
            )),
        }
    }

    if let Some(separation) = table.get("audio.stereo_separation") {
        match separation.parse::<u8>().ok().filter(|value| *value <= 100) {
            Some(value) => config.audio.stereo_separation = value,
            None => warnings.push(format!(
                "audio.stereo_separation must be 0..=100, got {separation}"
            )),
        }
    }

    if let Some(seconds) = table.get("audio.max_seconds") {
        match seconds.parse::<f64>().ok().filter(|value| *value > 0.0) {
            Some(value) => config.max_seconds = value,
            None => warnings.push(format!("audio.max_seconds must be above 0, got {seconds}")),
        }
    }

    if let Some(octave) = table.get("edit.octave") {
        match octave.parse::<i32>() {
            Ok(value) => config.octave = clamp_octave(value),
            Err(_) => warnings.push(format!("edit.octave must be a number, got {octave}")),
        }
    }

    if let Some(step) = table.get("edit.step") {
        match step.parse::<i32>() {
            Ok(value) => config.step = clamp_step(value),
            Err(_) => warnings.push(format!("edit.step must be a number, got {step}")),
        }
    }

    if let Some(view) = table.get("default_view") {
        match VizMode::parse(view) {
            Some(mode) => config.default_view = mode,
            None => warnings.push(format!(
                "default_view must be spectrum, scope, or off, got {view}; keeping spectrum"
            )),
        }
    }

    if let Some(flag) = table.get("reopen_last") {
        match parse_bool(flag) {
            Some(value) => config.reopen_last = value,
            None => warnings.push(format!(
                "reopen_last must be true or false, got {flag}; keeping true"
            )),
        }
    }

    Ok(Parsed {
        config,
        warning: if warnings.is_empty() {
            None
        } else {
            Some(warnings.join("; "))
        },
    })
}

/// Flat `key = value` assignments. `[section]` prefixes later keys with `section.`.
pub(crate) fn parse_table(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut table = BTreeMap::new();
    let mut section = String::new();
    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = parse_section(line, line_no)?;
            continue;
        }
        let (key, value) = split_kv(line, line_no)?;
        let full = if section.is_empty() {
            key
        } else {
            format!("{section}.{key}")
        };
        table.insert(full, value);
    }
    Ok(table)
}

fn parse_section(line: &str, line_no: usize) -> Result<String, String> {
    let Some(inner) = line
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return Err(format!("line {line_no}: expected [section]"));
    };
    let inner = inner.trim();
    if inner.is_empty()
        || !inner
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(format!("line {line_no}: bad section name"));
    }
    Ok(inner.to_string())
}

fn split_kv(line: &str, line_no: usize) -> Result<(String, String), String> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(format!("line {line_no}: expected key = value"));
    };
    let key = key.trim();
    if key.is_empty()
        || !key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        return Err(format!("line {line_no}: bad key"));
    }
    let value = parse_value(value.trim()).map_err(|err| format!("line {line_no}: {err}"))?;
    Ok((key.to_string(), value))
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn parse_value(value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Err("missing value".to_string());
    }
    let mut chars = value.chars();
    match chars.next() {
        Some('"') => parse_basic_string(value),
        Some('\'') => parse_literal_string(value),
        Some(_) => Ok(value.trim().to_string()),
        None => Err("missing value".to_string()),
    }
}

fn parse_basic_string(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    if bytes.first() != Some(&b'"') {
        return Err("expected a quoted string".to_string());
    }
    let mut out = String::new();
    let mut escape = false;
    let mut closed = false;
    for (index, ch) in value.char_indices().skip(1) {
        if escape {
            match ch {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                other => return Err(format!("unknown escape \\{other}")),
            }
            escape = false;
            continue;
        }
        match ch {
            '\\' => escape = true,
            '"' => {
                let rest = value[index + ch.len_utf8()..].trim();
                if !rest.is_empty() {
                    return Err(format!("trailing {rest:?} after a string"));
                }
                closed = true;
                break;
            }
            other => out.push(other),
        }
    }
    if escape || !closed {
        return Err("unterminated string".to_string());
    }
    Ok(out)
}

fn parse_literal_string(value: &str) -> Result<String, String> {
    let Some(rest) = value.strip_prefix('\'') else {
        return Err("expected a quoted string".to_string());
    };
    let Some(end) = rest.find('\'') else {
        return Err("unterminated string".to_string());
    };
    let trailing = rest[end + 1..].trim();
    if !trailing.is_empty() {
        return Err(format!("trailing {trailing:?} after a string"));
    }
    Ok(rest[..end].to_string())
}

/// Drop a `#` comment that is not inside a quoted string.
pub(crate) fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut quote = '"';
    let mut escape = false;
    for (index, ch) in line.char_indices() {
        if in_string {
            if escape {
                escape = false;
                continue;
            }
            if quote == '"' && ch == '\\' {
                escape = true;
                continue;
            }
            if ch == quote {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' | '\'' => {
                in_string = true;
                quote = ch;
            }
            '#' => {
                // `#rgb` / `#rrggbb` is a color, which Kitty and Ghostty write
                // unquoted. A hash that does not start a color is a comment.
                let rest = &line[index + 1..];
                let hexits = rest.chars().take_while(|ch| ch.is_ascii_hexdigit()).count();
                if !matches!(hexits, 3 | 4 | 6 | 8) {
                    return &line[..index];
                }
            }
            _ => {}
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_defaults_parse() {
        let parsed = parse_text(Config::documented()).unwrap();
        assert!(parsed.warning.is_none(), "{:?}", parsed.warning);
        assert_eq!(parsed.config, Config::default());
    }

    #[test]
    fn a_broken_file_is_an_error_and_a_bad_value_is_a_warning() {
        let err = parse_text("this is not toml\n").unwrap_err();
        assert!(err.contains("line"), "{err}");

        let parsed =
            parse_text("theme = \"nope\"\n[audio]\nsample_rate = 48000\nstereo_separation = 200\n")
                .unwrap();
        assert!(parsed.warning.is_some(), "{parsed:?}");
        assert_eq!(parsed.config.theme, ThemeRequest::Auto);
        assert_eq!(parsed.config.audio.sample_rate, 48_000);
        assert_eq!(parsed.config.audio.stereo_separation, 100);
    }

    #[test]
    fn missing_file_keeps_defaults_when_that_is_allowed() {
        let path = std::env::temp_dir().join(format!(
            "omatrack-missing-config-{}-{}.toml",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&path);
        let loaded = load(&path, true);
        assert!(loaded.warning.is_none());
        assert_eq!(loaded.config, Config::default());

        let loaded = load(&path, false);
        assert!(loaded.warning.is_some());
    }

    #[test]
    fn sections_and_comments_do_not_hide_values() {
        let parsed = parse_text(
            r#"
            # palette
            theme = "phosphor" # green
            [audio]
            sample_rate = 22050
            interpolation = "nearest"
            stereo_separation = 0
            max_seconds = 12
            [edit]
            octave = 3
            step = 0
            "#,
        )
        .unwrap();
        assert!(parsed.warning.is_none(), "{:?}", parsed.warning);
        assert_eq!(parsed.config.theme, ThemeRequest::Phosphor);
        assert_eq!(parsed.config.audio.sample_rate, 22_050);
        assert_eq!(parsed.config.audio.interpolation, Interpolation::Nearest);
        assert_eq!(parsed.config.audio.stereo_separation, 0);
        assert_eq!(parsed.config.max_seconds, 12.0);
        assert_eq!(parsed.config.octave, 3);
        assert_eq!(parsed.config.step, 0);
        assert!(parsed.config.reopen_last);
    }

    #[test]
    fn reopen_last_can_be_turned_off_without_dropping_the_rest() {
        let parsed = parse_text("reopen_last = false\ntheme = \"terminal\"\n").unwrap();
        assert!(parsed.warning.is_none(), "{:?}", parsed.warning);
        assert!(!parsed.config.reopen_last);
        assert_eq!(parsed.config.theme, ThemeRequest::Terminal);

        let parsed = parse_text("reopen_last = \"maybe\"\n").unwrap();
        assert!(parsed.warning.is_some());
        assert!(parsed.config.reopen_last);
        assert_eq!(parsed.config.default_view, VizMode::Panel);
    }

    #[test]
    fn default_view_accepts_spectrum_scope_and_off() {
        let parsed = parse_text("default_view = \"scope\"\n").unwrap();
        assert!(parsed.warning.is_none(), "{:?}", parsed.warning);
        assert_eq!(parsed.config.default_view, VizMode::Scope);
        assert_eq!(parsed.config.theme, ThemeRequest::Auto);

        let parsed = parse_text("default_view = \"off\"\ntheme = \"phosphor\"\n").unwrap();
        assert_eq!(parsed.config.default_view, VizMode::Off);
        assert_eq!(parsed.config.theme, ThemeRequest::Phosphor);

        let parsed = parse_text("default_view = \"nope\"\nreopen_last = false\n").unwrap();
        assert!(parsed.warning.unwrap().contains("default_view"));
        assert_eq!(parsed.config.default_view, VizMode::Panel);
        assert!(!parsed.config.reopen_last);
    }
}
