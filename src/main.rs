//! `omatrack <file.mod>` — ProTracker viewer and player.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use omatrack::player::{Interpolation, PlayerConfig, DEFAULT_SAMPLE_RATE};
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
    module: PathBuf,
    render: Option<RenderOptions>,
}

struct RenderOptions {
    wav: PathBuf,
    sample_rate: u32,
    max_seconds: f64,
    interpolation: Interpolation,
    separation: u8,
}

fn run() -> Result<(), MainError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = parse(&args)?;
    let module = Module::load(&options.module)
        .map_err(|err| MainError::Failed(annotate(&options.module, err)))?;
    if let Some(render) = options.render {
        render_song(&module, &render)?;
    } else {
        omatrack::tui::run(module, options.module)
            .map_err(|err| MainError::Failed(err.to_string()))?;
    }
    Ok(())
}

fn render_song(module: &Module, render: &RenderOptions) -> Result<(), MainError> {
    let config = PlayerConfig {
        sample_rate: render.sample_rate,
        interpolation: render.interpolation,
        stereo_separation: render.separation,
    };
    let max_frames = (render.max_seconds * f64::from(render.sample_rate))
        .round()
        .clamp(1.0, u32::MAX as f64) as usize;
    let stats = omatrack::player::render_to_wav(module, &render.wav, config, max_frames)
        .map_err(|err| MainError::Failed(annotate(&render.wav, err)))?;
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
        render.wav.display(),
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
    if args.is_empty() {
        return Err(MainError::Usage(usage()));
    }

    let mut module = None;
    let mut wav = None;
    let mut sample_rate = DEFAULT_SAMPLE_RATE;
    let mut max_seconds = 600.0;
    let mut interpolation = Interpolation::Linear;
    let mut separation = 100u8;
    let mut render_knob = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        match arg {
            "--render" => {
                wav = Some(PathBuf::from(next_value(args, &mut index, "--render")?));
            }
            "--rate" => {
                render_knob = true;
                let text = next_value(args, &mut index, "--rate")?;
                sample_rate = text
                    .parse::<u32>()
                    .ok()
                    .filter(|rate| *rate > 0)
                    .ok_or_else(|| {
                        MainError::Usage(format!(
                            "--rate expects a sample rate above 0, got {text}\n{}",
                            usage()
                        ))
                    })?;
            }
            "--max-seconds" => {
                render_knob = true;
                let text = next_value(args, &mut index, "--max-seconds")?;
                max_seconds = text
                    .parse::<f64>()
                    .ok()
                    .filter(|seconds| *seconds > 0.0)
                    .ok_or_else(|| {
                        MainError::Usage(format!(
                            "--max-seconds expects a duration above 0, got {text}\n{}",
                            usage()
                        ))
                    })?;
            }
            "--interpolate" => {
                render_knob = true;
                let text = next_value(args, &mut index, "--interpolate")?;
                interpolation = match text {
                    "linear" => Interpolation::Linear,
                    "nearest" => Interpolation::Nearest,
                    other => {
                        return Err(MainError::Usage(format!(
                            "--interpolate expects linear or nearest, got {other}\n{}",
                            usage()
                        )))
                    }
                };
            }
            "--separation" => {
                render_knob = true;
                let text = next_value(args, &mut index, "--separation")?;
                separation = text
                    .parse::<u8>()
                    .ok()
                    .filter(|value| *value <= 100)
                    .ok_or_else(|| {
                        MainError::Usage(format!(
                            "--separation expects 0..=100, got {text}\n{}",
                            usage()
                        ))
                    })?;
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

    if wav.is_none() && render_knob {
        return Err(MainError::Usage(format!(
            "--rate, --max-seconds, --interpolate, and --separation are used with --render\n{}",
            usage()
        )));
    }
    let Some(module) = module else {
        return Err(MainError::Usage(usage()));
    };
    let render = wav.map(|wav| RenderOptions {
        wav,
        sample_rate,
        max_seconds,
        interpolation,
        separation,
    });
    Ok(Options { module, render })
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
usage: omatrack <file.mod>
       omatrack --render <out.wav> <file.mod>
       omatrack --help
"
    .to_string()
}

fn help() -> String {
    "\
Omatrack — ProTracker module viewer and player

Usage:
    omatrack <file.mod>
    omatrack --render <out.wav> <file.mod>
    omatrack --help

Opens a 31-sample, 4-channel .mod file (M.K., M!K!, FLT4, or 4CHN).
The terminal needs about 76×20. Space plays from the cursor. Enter
toggles edit mode. Ctrl-S writes the file. ? lists every key. If no
audio device is available the error stays on the transport bar.

    --render <out.wav>     mix the song to a 16-bit stereo WAV and exit
    --rate <hz>            WAV sample rate (default 44100)
    --max-seconds <n>      safety cap for songs that do not loop (default 600)
    --interpolate <mode>   linear (default) or nearest
    --separation <0-100>   100 is hard Amiga panning, 0 is mono

Keys:
    Enter                edit / browse
    space                play / stop
    ?                    key list
    Ctrl-S               save
    Ctrl-Z / Ctrl-Y      undo / redo
    q, Esc, Ctrl-Q       quit (asks when the song is modified)
    Tab                  pattern, samples, order
    1 2 3 4              mute channel (Alt-1..4 in edit mode)
    Up/Down, j/k         move the cursor
    Left/Right, h/l      change channel (pattern view)
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
        assert_eq!(options.module, PathBuf::from("song.mod"));
        let render = options.render.expect("render");
        assert_eq!(render.wav, PathBuf::from("out.wav"));
        assert_eq!(render.sample_rate, 48_000);
        assert_eq!(render.interpolation, Interpolation::Linear);
        assert_eq!(render.separation, 100);

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
        ]))
        .unwrap();
        let render = options.render.expect("render");
        assert_eq!(render.interpolation, Interpolation::Nearest);
        assert_eq!(render.separation, 0);
        assert_eq!(render.max_seconds, 12.0);
    }

    #[test]
    fn help_and_unknown_flags_are_not_files() {
        assert!(matches!(parse(&args(&["--help"])), Err(MainError::Help(_))));
        assert!(matches!(
            parse(&args(&["song.mod", "--nope"])),
            Err(MainError::Usage(_))
        ));
        assert!(matches!(parse(&args(&[])), Err(MainError::Usage(_))));
        assert!(matches!(
            parse(&args(&["song.mod", "--rate", "22050"])),
            Err(MainError::Usage(_))
        ));
    }
}
