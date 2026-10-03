//! `omatrack <file.mod>` — read-only ProTracker viewer.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

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

enum MainError {
    Help(String),
    Usage(String),
    Failed(String),
}

fn run() -> Result<(), MainError> {
    let mut args = std::env::args().skip(1);
    let Some(arg) = args.next() else {
        return Err(MainError::Usage(usage()));
    };
    if args.next().is_some() {
        return Err(MainError::Usage(usage()));
    }
    if matches!(arg.as_str(), "-h" | "--help" | "help") {
        return Err(MainError::Help(help()));
    }

    let path = PathBuf::from(&arg);
    let module = Module::load(&path).map_err(|err| MainError::Failed(annotate(&path, err)))?;
    omatrack::tui::run(module).map_err(|err| MainError::Failed(err.to_string()))?;
    Ok(())
}

fn annotate(path: &Path, err: Error) -> String {
    match err {
        Error::Io { .. } | Error::Terminal(_) => err.to_string(),
        other => format!("{}: {other}", path.display()),
    }
}

fn usage() -> String {
    "usage: omatrack <file.mod>\n       omatrack --help\n".to_string()
}

fn help() -> String {
    "\
Omatrack — read-only ProTracker module viewer

Usage:
    omatrack <file.mod>
    omatrack --help

Opens a 31-sample, 4-channel .mod file (M.K., M!K!, FLT4, or 4CHN).
Playback and editing are later milestones. The terminal needs about 76×20.

Keys:
    q, Esc, Ctrl-C   quit
    Tab              switch between the pattern and the sample list
    Up/Down, j/k     move the cursor
    Left/Right, h/l  change channel (pattern view)
    PgUp/PgDn        page
    Home/End         first or last row, or sample
    [ ]              previous / next order position
    , .              previous / next pattern
"
    .to_string()
}
