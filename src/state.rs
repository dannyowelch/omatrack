//! Last module path.
//!
//! This is session state, not configuration. It lives at
//! `$XDG_STATE_HOME/omatrack/last_file`, or `~/.local/state/omatrack/last_file`
//! when `XDG_STATE_HOME` is unset. The file the user edits
//! (`config.toml`) only says whether startup should read it.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::module::{Module, Tag};
use crate::track::{self, Song};

/// Where [`read`] and [`write`] look when the caller does not pass a path.
pub fn default_path() -> PathBuf {
    let root = crate::config::xdg_state_home()
        .unwrap_or_else(|| crate::config::home_dir().join(".local").join("state"));
    root.join("omatrack").join("last_file")
}

/// The path stored in `path`, if the file exists and names something.
///
/// A missing file, an empty file, or a file that cannot be read is `None`.
/// This never creates directories and never touches any path except `path`.
pub fn read(path: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(path).ok()?;
    let line = first_line(&bytes);
    if line.is_empty() {
        None
    } else {
        Some(PathBuf::from(os_from_bytes(line)))
    }
}

/// Remember `module` in `path`, creating parent directories when needed.
///
/// The stored path is canonical when the file exists, so a later launch does
/// not depend on the working directory. The write is a temp file in the same
/// directory followed by a rename.
pub fn write(path: &Path, module: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let stored = absolute_path(module);
    let tmp = temporary(path);
    std::fs::write(&tmp, path_bytes(&stored))?;
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

/// Forget the stored path. A missing file is success.
pub fn clear(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// What to open before the terminal starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchPlan {
    /// A path from the command line. Failure exits.
    File(PathBuf),
    /// The stored path. Failure becomes an empty module and a notice.
    Reopen(PathBuf),
    /// No argument and nothing to reopen.
    Empty,
}

/// A command-line path wins. `reopen` is false when the config or `--no-reopen` says so.
pub fn launch_plan(
    argument: Option<PathBuf>,
    reopen: bool,
    remembered: Option<PathBuf>,
) -> LaunchPlan {
    if let Some(path) = argument {
        LaunchPlan::File(path)
    } else if reopen {
        match remembered {
            Some(path) => LaunchPlan::Reopen(path),
            None => LaunchPlan::Empty,
        }
    } else {
        LaunchPlan::Empty
    }
}

/// The module startup settled on.
#[derive(Debug)]
pub struct Launched {
    /// Song to show. Empty when [`Self::track`] is set.
    pub module: Module,
    /// XM or IT song. `.mod` leaves this empty.
    pub track: Option<Song>,
    /// Empty when the song is untitled.
    pub path: PathBuf,
    /// Shown once, in the error color. Set when a reopen failed.
    pub notice: Option<String>,
    /// Persist this path. `None` leaves the state file as it is.
    pub remember: Option<PathBuf>,
}

/// Load `plan`. A bad remembered file does not return [`Err`].
pub fn launch(plan: LaunchPlan) -> Result<Launched, Error> {
    match plan {
        LaunchPlan::File(path) => load_launched(&path),
        LaunchPlan::Reopen(path) => match load_launched(&path) {
            Ok(launched) => Ok(launched),
            Err(err) => Ok(Launched {
                module: Module::new(Tag::Mk),
                track: None,
                path: PathBuf::new(),
                notice: Some(reopen_notice(&path, &err)),
                remember: None,
            }),
        },
        LaunchPlan::Empty => Ok(Launched {
            module: Module::new(Tag::Mk),
            track: None,
            path: PathBuf::new(),
            notice: None,
            remember: None,
        }),
    }
}

fn load_launched(path: &Path) -> Result<Launched, Error> {
    match track::open_path(path)? {
        track::Opened::Mod(module) => Ok(Launched {
            module,
            track: None,
            path: path.to_path_buf(),
            notice: None,
            remember: Some(path.to_path_buf()),
        }),
        track::Opened::Track(song) => Ok(Launched {
            module: Module::new(Tag::Mk),
            track: Some(song),
            path: path.to_path_buf(),
            notice: None,
            remember: Some(path.to_path_buf()),
        }),
    }
}

fn reopen_notice(path: &Path, err: &Error) -> String {
    let why = match err {
        Error::Io { source, .. } if source.kind() == io::ErrorKind::NotFound => "missing",
        Error::Io { .. } => "unreadable",
        _ => "not a valid module",
    };
    format!(
        "Could not reopen {} ({why}); starting empty",
        path.display()
    )
}

fn absolute_path(module: &Path) -> PathBuf {
    if let Ok(canon) = std::fs::canonicalize(module) {
        return canon;
    }
    if module.is_absolute() {
        module.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(module))
            .unwrap_or_else(|_| module.to_path_buf())
    }
}

fn temporary(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_else(|| OsString::from("last_file"));
    name.push(".tmp");
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

fn first_line(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(bytes.len());
    let line = &bytes[..end];
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut bytes = path.as_os_str().as_bytes().to_vec();
        bytes.push(b'\n');
        bytes
    }
    #[cfg(not(unix))]
    {
        let mut text = path.to_string_lossy().into_owned();
        text.push('\n');
        text.into_bytes()
    }
}

fn os_from_bytes(bytes: &[u8]) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(bytes).to_os_string()
    }
    #[cfg(not(unix))]
    {
        let text = String::from_utf8_lossy(bytes);
        OsString::from(text.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "omatrack-state-{label}-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn round_trip_stays_inside_the_given_directory() {
        let dir = scratch("round");
        let state = dir.join("omatrack").join("last_file");
        let module = dir.join("songs").join("tune.mod");
        std::fs::create_dir_all(module.parent().unwrap()).unwrap();
        std::fs::write(&module, b"x").unwrap();

        write(&state, &module).unwrap();
        let got = read(&state).expect("stored path");
        assert_eq!(got, std::fs::canonicalize(&module).unwrap());
        assert!(state.starts_with(&dir));

        clear(&state).unwrap();
        assert!(read(&state).is_none());
        clear(&state).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_state_file_is_empty_and_unreadable_bytes_are_ignored() {
        let dir = scratch("missing");
        let state = dir.join("last_file");
        assert!(read(&state).is_none());
        std::fs::write(&state, b"\n").unwrap();
        assert!(read(&state).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_argument_wins_and_no_reopen_skips_the_stored_path() {
        let song = PathBuf::from("song.mod");
        let old = PathBuf::from("old.mod");
        assert_eq!(
            launch_plan(Some(song.clone()), true, Some(old.clone())),
            LaunchPlan::File(song)
        );
        assert_eq!(
            launch_plan(None, false, Some(old.clone())),
            LaunchPlan::Empty
        );
        assert_eq!(
            launch_plan(None, true, Some(old.clone())),
            LaunchPlan::Reopen(old)
        );
        assert_eq!(launch_plan(None, true, None), LaunchPlan::Empty);
    }

    #[test]
    fn a_missing_or_invalid_reopen_starts_empty_and_names_the_file() {
        let dir = scratch("reopen");
        let missing = dir.join("gone.mod");
        let launched = launch(LaunchPlan::Reopen(missing)).unwrap();
        assert!(launched.path.as_os_str().is_empty());
        assert!(launched.remember.is_none());
        let notice = launched.notice.expect("notice");
        assert!(notice.contains("gone.mod"), "{notice}");
        assert!(notice.contains("missing"), "{notice}");
        assert!(notice.contains("starting empty"), "{notice}");

        let bad = dir.join("bad.mod");
        std::fs::write(&bad, b"this is not a module").unwrap();
        let launched = launch(LaunchPlan::Reopen(bad)).unwrap();
        let notice = launched.notice.expect("notice");
        assert!(notice.contains("not a valid module"), "{notice}");
        assert!(launched.path.as_os_str().is_empty());

        let err = launch(LaunchPlan::File(dir.join("nope.mod"))).unwrap_err();
        assert!(err.to_string().contains("nope.mod") || matches!(err, Error::Io { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_module_reopens_from_either_plan() {
        let dir = scratch("ok");
        let path = dir.join("ok.mod");
        let mut module = Module::new(Tag::Mk);
        module.set_title("Kept").unwrap();
        module.save(&path).unwrap();

        let launched = launch(LaunchPlan::Reopen(path.clone())).unwrap();
        assert_eq!(launched.module.display_title(), "Kept");
        assert_eq!(launched.path, path);
        assert!(launched.notice.is_none());
        assert_eq!(launched.remember.as_deref(), Some(path.as_path()));

        let launched = launch(LaunchPlan::Empty).unwrap();
        assert!(launched.path.as_os_str().is_empty());
        assert!(launched.notice.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
