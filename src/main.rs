//! `omatrack [file.mod]` — ProTracker viewer and player.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use omatrack::config::{self, ThemeRequest};
use omatrack::player::{Interpolation, PlayerConfig};
use omatrack::state::{self, LaunchPlan};
use omatrack::tui::{self, Session};
use omatrack::{Error, Module};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(MainError::Help(text)) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(MainError::Usage(text)) => {
            eprintln!("{text}");
            ExitCode::from(2)
        }
        Err(MainError::Failed(text)) => {
            eprintln!("omatrack: {text}");
            ExitCode::from(1)
        }
    }
}

#[derive(Debug)]
enum MainError {
    Help(String),
    Usage(String),
    Failed(String),
}

struct Options {
    module: Option<PathBuf>,
    render: Option<RenderOptions>,
    theme: Option<ThemeRequest>,
    config: Option<PathBuf>,
    no_reopen: bool,
}

struct RenderOptions {
    wav: PathBuf,
    sample_rate: Option<u32>,
    max_seconds: Option<f64>,
    interpolation: Option<Interpolation>,
    separation: Option<u8>,
}

fn run() -> Result<(), MainError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = parse(&args)?;
    let config_path = options.config.clone().unwrap_or_else(config::default_path);
    let loaded = config::load(&config_path, options.config.is_none());
    let mut settings = loaded.config;
    if let Some(theme) = options.theme {
        settings.theme = theme;
    }
    let reopen = settings.reopen_last && !options.no_reopen;
    let state_path = state::default_path();

    if let Some(render) = options.render {
        if let Some(warning) = &loaded.warning {
            eprintln!("omatrack: {warning}");
        }
        let Some(path) = options.module else {
            return Err(MainError::Usage(format!(
                "--render needs a module file\n{}",
                usage()
            )));
        };
        let module = Module::load(&path).map_err(|err| MainError::Failed(annotate(&path, err)))?;
        let _ = state::write(&state_path, &path);
        let player = merge_player(settings.audio, &render);
        let max_seconds = render.max_seconds.unwrap_or(settings.max_seconds);
        render_song(&module, &render.wav, player, max_seconds)?;
        return Ok(());
    }

    let remembered = if reopen {
        state::read(&state_path)
    } else {
        None
    };
    let plan = state::launch_plan(options.module, reopen, remembered);
    let argument = match &plan {
        LaunchPlan::File(path) => Some(path.clone()),
        LaunchPlan::Reopen(_) | LaunchPlan::Empty => None,
    };
    let launched = state::launch(plan).map_err(|err| match argument {
        Some(path) => MainError::Failed(annotate(&path, err)),
        None => MainError::Failed(err.to_string()),
    })?;
    if let Some(path) = &launched.remember {
        let _ = state::write(&state_path, path);
    }
    let module = launched.module;
    let path = launched.path;
    let depth = tui::detect_color_depth();
    let home = config::home_dir();
    let state = config::xdg_state_home();
    let loaded_theme = tui::load_theme(settings.theme, depth, &home, state.as_deref());
    let mut notice = Vec::new();
    if let Some(warning) = loaded.warning {
        notice.push(warning);
    }
    if let Some(warning) = loaded_theme.warning.clone() {
        notice.push(warning);
    }
    if let Some(warning) = launched.notice {
        notice.push(warning);
    }
    let session = Session {
        module,
        path,
        theme: loaded_theme.theme,
        theme_label: loaded_theme.label,
        theme_watch: loaded_theme.watch,
        theme_request: settings.theme,
        color_depth: depth,
        player: settings.audio,
        max_seconds: settings.max_seconds,
        octave: settings.octave,
        step: settings.step,
        default_view: settings.default_view,
        notice: if notice.is_empty() {
            None
        } else {
            Some(notice.join(" "))
        },
        state_path,
    };
    tui::run(session).map_err(|err| MainError::Failed(err.to_string()))?;
    Ok(())
}

fn merge_player(mut player: PlayerConfig, render: &RenderOptions) -> PlayerConfig {
    if let Some(rate) = render.sample_rate {
        player.sample_rate = rate;
    }
    if let Some(interpolation) = render.interpolation {
        player.interpolation = interpolation;
    }
    if let Some(separation) = render.separation {
        player.stereo_separation = separation;
    }
    player
}

fn render_song(
    module: &Module,
    wav: &Path,
    config: PlayerConfig,
    max_seconds: f64,
) -> Result<(), MainError> {
    let max_frames = (max_seconds * f64::from(config.sample_rate))
        .round()
        .clamp(1.0, u32::MAX as f64) as usize;
    let stats = omatrack::player::render_to_wav(module, wav, config, max_frames)
        .map_err(|err| MainError::Failed(annotate(wav, err)))?;
    let seconds = stats.frames as f64 / f64::from(stats.sample_rate);
    let why = if stats.halted {
        "halted by F00"
    } else if stats.looped {
        "stopped at the song loop"
    } else {
        "stopped at the time limit"
    };
    eprintln!(
        "wrote {} ({seconds:.2}s, {} Hz, {} frames, {why})",
        wav.display(),
        stats.sample_rate,
        stats.frames
    );
    Ok(())
}

fn parse(args: &[String]) -> Result<Options, MainError> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        return Err(MainError::Help(help()));
    }
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-V" | "--version"))
    {
        return Err(MainError::Help(format!(
            "omatrack {}",
            env!("CARGO_PKG_VERSION")
        )));
    }

    let mut module = None;
    let mut wav = None;
    let mut sample_rate = None;
    let mut max_seconds = None;
    let mut interpolation = None;
    let mut separation = None;
    let mut theme = None;
    let mut config = None;
    let mut no_reopen = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        match arg {
            "--render" => {
                wav = Some(PathBuf::from(next_value(args, &mut index, "--render")?));
            }
            "--rate" => {
                let text = next_value(args, &mut index, "--rate")?;
                sample_rate = Some(
                    text.parse::<u32>()
                        .ok()
                        .filter(|rate| *rate > 0)
                        .ok_or_else(|| {
                            MainError::Usage(format!(
                                "--rate expects a sample rate above 0, got {text}\n{}",
                                usage()
                            ))
                        })?,
                );
            }
            "--max-seconds" => {
                let text = next_value(args, &mut index, "--max-seconds")?;
                max_seconds = Some(
                    text.parse::<f64>()
                        .ok()
                        .filter(|seconds| *seconds > 0.0)
                        .ok_or_else(|| {
                            MainError::Usage(format!(
                                "--max-seconds expects a duration above 0, got {text}\n{}",
                                usage()
                            ))
                        })?,
                );
            }
            "--interpolate" => {
                let text = next_value(args, &mut index, "--interpolate")?;
                interpolation = Some(match text {
                    "linear" => Interpolation::Linear,
                    "nearest" => Interpolation::Nearest,
                    other => {
                        return Err(MainError::Usage(format!(
                            "--interpolate expects linear or nearest, got {other}\n{}",
                            usage()
                        )))
                    }
                });
            }
            "--separation" => {
                let text = next_value(args, &mut index, "--separation")?;
                separation = Some(
                    text.parse::<u8>()
                        .ok()
                        .filter(|value| *value <= 100)
                        .ok_or_else(|| {
                            MainError::Usage(format!(
                                "--separation expects 0..=100, got {text}\n{}",
                                usage()
                            ))
                        })?,
                );
            }
            "--theme" => {
                let text = next_value(args, &mut index, "--theme")?;
                theme = Some(ThemeRequest::parse(text).ok_or_else(|| {
                    MainError::Usage(format!(
                        "--theme expects auto, omarchy, protracker, phosphor, or terminal, got {text}\n{}",
                        usage()
                    ))
                })?);
            }
            "--config" => {
                config = Some(PathBuf::from(next_value(args, &mut index, "--config")?));
            }
            "--no-reopen" => {
                no_reopen = true;
            }
            other if other.starts_with('-') => {
                return Err(MainError::Usage(format!(
                    "unknown option {other}\n{}",
                    usage()
                )));
            }
            other => {
                if module.is_some() {
                    return Err(MainError::Usage(usage()));
                }
                module = Some(PathBuf::from(other));
            }
        }
        index += 1;
    }

    let render = wav.map(|wav| RenderOptions {
        wav,
        sample_rate,
        max_seconds,
        interpolation,
        separation,
    });
    Ok(Options {
        module,
        render,
        theme,
        config,
        no_reopen,
    })
}

fn next_value<'a>(args: &'a [String], index: &mut usize, flag: &str) -> Result<&'a str, MainError> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .ok_or_else(|| MainError::Usage(format!("{flag} needs a value\n{}", usage())))
}

fn annotate(path: &Path, err: Error) -> String {
    match err {
        Error::Io { .. } | Error::Terminal(_) => err.to_string(),
        other => format!("{}: {other}", path.display()),
    }
}

fn usage() -> String {
    "\
usage: omatrack [options] [file.mod]
       omatrack --render <out.wav> [options] <file.mod>
       omatrack --help
       omatrack --version
"
    .to_string()
}

fn help() -> String {
    "\
Omatrack — ProTracker module viewer and player

Usage:
    omatrack [options] [file.mod]
    omatrack --render <out.wav> [options] <file.mod>
    omatrack --help
    omatrack --version

With no file, omatrack reopens the last module (unless --no-reopen is set
or reopen_last is false). A missing or invalid remembered file starts an
empty module and says why. Ctrl-F opens the file menu (new, open, save,
save as). A * after the title means unsaved edits; quit, new, and open ask
before discarding them.

Opens a 31-sample, 4-channel .mod file (M.K., M!K!, FLT4, or 4CHN).
The terminal needs about 76×20. Space plays from the cursor. Enter
toggles edit mode. Ctrl-S writes the file, or asks for a path when the
module is untitled. ? lists every key. If no audio device is available
the error stays on the transport bar.

    --theme <name>         auto, omarchy, protracker, phosphor, or terminal
    --config <path>        config file (default ~/.config/omatrack/config.toml)
    --no-reopen            do not load the last module on this launch
    --render <out.wav>     mix the song to a 16-bit stereo WAV and exit
    --rate <hz>            WAV sample rate (config, or 44100)
    --max-seconds <n>      safety cap for songs that do not loop (config, or 600)
    --interpolate <mode>   linear or nearest (config, or linear)
    --separation <0-100>   100 is hard Amiga panning, 0 is mono
    --version              print the version

The theme is read from the active Omarchy palette when --theme is auto
or omarchy. SIGUSR1 reloads it, and so does a change to the theme file.
protracker and phosphor are built in. terminal uses ANSI colors so the
terminal's own theme shows through.

The tracker opens on the spectrum. default_view in the config file is
spectrum, scope, or off. A terminal too short for the spectrum panel
keeps the pattern and does not report an error.

Keys:
    Enter                edit / browse
    space                play / stop
    Ctrl-R               rewind to order 0, row 0
                         (playing continues; stopped only moves the cursor)
    ?                    key list
    Ctrl-F               file menu: n new, o open, s save, a save as
    Ctrl-S               save (save as, when the module is untitled)
    Ctrl-Z / Ctrl-Y      undo / redo
    q, Esc, Ctrl-Q       quit (asks when the song is modified)
    Tab                  pattern, samples, order
    1 2 3 4              mute channel (Alt-1..4 in edit mode)
    Up/Down, j/k         move the cursor
    Left/Right, h/l      change channel (pattern view)
    F5                   cycle visualization: spectrum, scope, off
                         (startup view is spectrum; config default_view)
    PgUp/PgDn            page
    Home/End             first or last row, or sample
    [ ]                  previous / next order position
    , .                  previous / next pattern

Sample list (R still renames; w reverses, so the keys do not clash):
    i / o                import a WAV / export this sample
    Ctrl-G               render the song to a WAV
    v / f                volume / finetune
    l / /                loop points / toggle loop
    t                    trim to a byte range
    n / w                normalize / reverse
    a / z                fade in / fade out
    c                    clear the sample data
    y                    copy this sample to another slot
    p                    preview at the note from - and =
    u                    undo (same stack as Ctrl-Z)
"
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn render_options_parse_in_either_order() {
        let options = parse(&args(&[
            "--render", "out.wav", "--rate", "48000", "song.mod",
        ]))
        .unwrap();
        assert_eq!(options.module, Some(PathBuf::from("song.mod")));
        let render = options.render.expect("render");
        assert_eq!(render.wav, PathBuf::from("out.wav"));
        assert_eq!(render.sample_rate, Some(48_000));
        assert_eq!(render.interpolation, None);
        assert_eq!(render.separation, None);

        let options = parse(&args(&[
            "song.mod",
            "--render",
            "a.wav",
            "--interpolate",
            "nearest",
            "--separation",
            "0",
            "--max-seconds",
            "12",
            "--theme",
            "phosphor",
        ]))
        .unwrap();
        let render = options.render.expect("render");
        assert_eq!(render.interpolation, Some(Interpolation::Nearest));
        assert_eq!(render.separation, Some(0));
        assert_eq!(render.max_seconds, Some(12.0));
        assert_eq!(options.theme, Some(ThemeRequest::Phosphor));
    }

    #[test]
    fn help_version_and_unknown_flags_are_not_files() {
        assert!(matches!(parse(&args(&["--help"])), Err(MainError::Help(_))));
        assert!(matches!(
            parse(&args(&["--version"])),
            Err(MainError::Help(text)) if text.starts_with("omatrack ")
        ));
        assert!(matches!(
            parse(&args(&["song.mod", "--nope"])),
            Err(MainError::Usage(_))
        ));
        let empty = parse(&args(&[])).unwrap();
        assert!(empty.module.is_none());
        assert!(empty.render.is_none());
        assert!(!empty.no_reopen);
        let skipped = parse(&args(&["--no-reopen", "song.mod"])).unwrap();
        assert!(skipped.no_reopen);
        assert_eq!(skipped.module, Some(PathBuf::from("song.mod")));
        assert!(matches!(
            parse(&args(&["song.mod", "--theme", "nope"])),
            Err(MainError::Usage(_))
        ));
        let help = help();
        assert!(help.contains("default_view"));
        assert!(help.contains("spectrum, scope, or off"));
        assert!(help.contains("Ctrl-R"));
        assert!(help.contains("order 0, row 0"));
    }

    #[test]
    fn no_module_is_fine_until_render_needs_one() {
        let options = parse(&args(&["--theme", "terminal"])).unwrap();
        assert_eq!(options.theme, Some(ThemeRequest::Terminal));
        assert!(options.module.is_none());
        let err = run_render_without_file();
        assert!(matches!(err, Err(MainError::Usage(_))));
    }

    fn run_render_without_file() -> Result<(), MainError> {
        let options = parse(&args(&["--render", "out.wav"])).unwrap();
        if options.render.is_some() && options.module.is_none() {
            Err(MainError::Usage(usage()))
        } else {
            Ok(())
        }
    }
}
