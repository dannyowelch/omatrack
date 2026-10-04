//! Sample-pane commands: WAV import and export, and edits on the shared undo stack.
//!
//! Pattern edits and these edits both go through [`Editor`](crate::edit::Editor).
//! `R` on this pane still renames the instrument. Reverse is `w`, so the two
//! do not share a key.

use std::path::{Path, PathBuf};

use crate::convert::{c2_rate, import_pcm, rate_for_note, ImportOptions, DEFAULT_DITHER_SEED};
use crate::error::Error;
use crate::notes::{self, format_period, C2_NOTE};
use crate::player::{self, PlayerConfig};
use crate::sample_edit;
use crate::wav::{decode_wav, write_mono8_wav};
use crate::SAMPLE_COUNT;

use super::app::{App, Command, Outcome};

/// Choosing a WAV to read, or a path to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathPrompt {
    /// Import, export a sample, or render the song.
    pub kind: PathKind,
    /// The path being edited.
    pub buffer: String,
    /// Names in the directory that contains [`Self::buffer`].
    pub entries: Vec<DirRow>,
    /// Highlighted row.
    pub selected: usize,
    /// Directory [`Self::entries`] was listed from.
    pub list_dir: PathBuf,
    /// Missing file, bad directory, or a bad rate.
    pub error: Option<String>,
    /// Export sample only. Empty means the C-2 rate.
    pub rate: String,
    /// Keystrokes edit [`Self::rate`].
    pub rate_focus: bool,
}

/// One row in the file list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirRow {
    /// File or directory name, or `..`.
    pub name: String,
    /// `name` is a directory.
    pub is_dir: bool,
}

/// What confirming the path prompt does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathKind {
    /// Read a WAV into the current sample.
    ImportWav,
    /// Write the current sample as 8-bit mono WAV.
    ExportSample,
    /// Mix the song the way `--render` does.
    ExportSong,
}

/// Options shown after a WAV path is accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportPrompt {
    /// File that exists.
    pub path: PathBuf,
    /// Index into the finetune-0 period table.
    pub note: usize,
    /// Finetune stored on the sample and used for the Amiga rate.
    pub finetune: i8,
    /// Peak-normalize before quantizing.
    pub normalize: bool,
    /// Triangular dither. Off rounds to the nearest code.
    pub dither: bool,
    /// `Some` while a custom rate is being typed.
    pub rate_text: Option<String>,
    /// Shown under the dialog.
    pub error: Option<String>,
}

/// A one-line sample edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FieldPrompt {
    /// Which value Enter writes.
    pub kind: FieldKind,
    /// The text so far.
    pub buffer: String,
    /// Why the last Enter was rejected.
    pub error: Option<String>,
}

/// The sample field a [`FieldPrompt`] edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldKind {
    /// Volume, `0..=64`.
    Volume,
    /// Finetune, `-8..=7`.
    Finetune,
    /// Loop start and length, in bytes.
    Loop,
    /// Trim start and end. The end is exclusive.
    Trim,
    /// Copy onto this 1-based slot.
    CopyTo,
}

/// Keys while the path picker is open. Letters keep their case.
pub(crate) fn path_command(key: super::app::Key) -> Option<Command> {
    use super::app::Key;
    match key {
        Key::Esc => Some(Command::CloseOverlay),
        Key::Enter => Some(Command::PathConfirm),
        Key::Backspace => Some(Command::PathBackspace),
        Key::Ctrl('u') => Some(Command::PathClear),
        Key::Tab => Some(Command::PathFocusRate),
        Key::Up => Some(Command::PathMove(-1)),
        Key::Down => Some(Command::PathMove(1)),
        Key::PageUp => Some(Command::PathMove(-8)),
        Key::PageDown => Some(Command::PathMove(8)),
        Key::Char(ch) if !ch.is_control() => Some(Command::PathPush(ch)),
        _ => None,
    }
}

/// Keys on the import-options dialog.
pub(crate) fn import_command(key: super::app::Key) -> Option<Command> {
    use super::app::Key;
    match key {
        Key::Esc => Some(Command::CloseOverlay),
        Key::Enter => Some(Command::ImportConfirm),
        Key::Left => Some(Command::ImportNudgeNote(-1)),
        Key::Right => Some(Command::ImportNudgeNote(1)),
        Key::Up => Some(Command::ImportNudgeFine(1)),
        Key::Down => Some(Command::ImportNudgeFine(-1)),
        Key::Char('n') | Key::Char('N') => Some(Command::ImportToggleNormalize),
        Key::Char('d') | Key::Char('D') => Some(Command::ImportToggleDither),
        Key::Char('r') | Key::Char('R') => Some(Command::ImportRateArm),
        Key::Backspace => Some(Command::ImportRateBackspace),
        Key::Char(ch) if ch.is_ascii_digit() => Some(Command::ImportRateDigit(ch)),
        _ => None,
    }
}

/// Keys on a volume, loop, trim, or copy prompt.
pub(crate) fn field_command(key: super::app::Key) -> Option<Command> {
    use super::app::Key;
    match key {
        Key::Esc => Some(Command::CloseOverlay),
        Key::Enter => Some(Command::FieldConfirm),
        Key::Backspace => Some(Command::FieldBackspace),
        Key::Ctrl('u') => Some(Command::FieldClear),
        Key::Char(ch) if !ch.is_control() => Some(Command::FieldPush(ch)),
        _ => None,
    }
}

impl App {
    /// Apply a sample-pane command. [`Err`] is a command this pane does not own.
    pub(crate) fn apply_sample(&mut self, command: Command) -> Result<Outcome, Command> {
        match command {
            Command::BeginImport => {
                self.open_path(PathKind::ImportWav);
                Ok(Outcome::None)
            }
            Command::BeginExportSample => {
                self.open_path(PathKind::ExportSample);
                Ok(Outcome::None)
            }
            Command::BeginExportSong => {
                self.open_path(PathKind::ExportSong);
                Ok(Outcome::None)
            }
            Command::PathPush(ch) => {
                self.path_push(ch);
                Ok(Outcome::None)
            }
            Command::PathBackspace => {
                self.path_backspace();
                Ok(Outcome::None)
            }
            Command::PathClear => {
                self.path_clear();
                Ok(Outcome::None)
            }
            Command::PathMove(delta) => {
                self.path_move(delta);
                Ok(Outcome::None)
            }
            Command::PathFocusRate => {
                if let super::app::Overlay::Path(prompt) = &mut self.overlay {
                    if prompt.kind == PathKind::ExportSample {
                        prompt.rate_focus = !prompt.rate_focus;
                    }
                }
                Ok(Outcome::None)
            }
            Command::PathConfirm => {
                self.confirm_path();
                Ok(Outcome::None)
            }
            Command::BeginVolume => {
                self.open_field(FieldKind::Volume);
                Ok(Outcome::None)
            }
            Command::BeginFinetune => {
                self.open_field(FieldKind::Finetune);
                Ok(Outcome::None)
            }
            Command::BeginLoop => {
                self.open_field(FieldKind::Loop);
                Ok(Outcome::None)
            }
            Command::BeginTrim => {
                self.open_field(FieldKind::Trim);
                Ok(Outcome::None)
            }
            Command::BeginCopySample => {
                self.open_field(FieldKind::CopyTo);
                Ok(Outcome::None)
            }
            Command::FieldPush(ch) => {
                if let super::app::Overlay::Field(prompt) = &mut self.overlay {
                    if prompt.buffer.chars().count() < 48 {
                        prompt.buffer.push(ch);
                        prompt.error = None;
                    }
                }
                Ok(Outcome::None)
            }
            Command::FieldBackspace => {
                if let super::app::Overlay::Field(prompt) = &mut self.overlay {
                    prompt.buffer.pop();
                    prompt.error = None;
                }
                Ok(Outcome::None)
            }
            Command::FieldClear => {
                if let super::app::Overlay::Field(prompt) = &mut self.overlay {
                    prompt.buffer.clear();
                    prompt.error = None;
                }
                Ok(Outcome::None)
            }
            Command::FieldConfirm => Ok(self.confirm_field()),
            Command::ToggleSampleLoop => {
                Ok(self.edit_current(sample_edit::toggle_loop, "Toggled loop"))
            }
            Command::NormalizeSample => Ok(self.edit_current(
                |sample| {
                    sample_edit::normalize(sample);
                    Ok(())
                },
                "Normalized",
            )),
            Command::ReverseSample => Ok(self.edit_current(
                |sample| {
                    sample_edit::reverse(sample);
                    Ok(())
                },
                "Reversed",
            )),
            Command::FadeIn => Ok(self.edit_current(
                |sample| {
                    sample_edit::fade_in(sample);
                    Ok(())
                },
                "Fade in",
            )),
            Command::FadeOut => Ok(self.edit_current(
                |sample| {
                    sample_edit::fade_out(sample);
                    Ok(())
                },
                "Fade out",
            )),
            Command::ClearSampleData => Ok(self.edit_current(
                |sample| {
                    sample_edit::clear_data(sample);
                    Ok(())
                },
                "Cleared sample data",
            )),
            Command::AuditionSample => Ok(self.audition()),
            Command::PreviewNote(delta) => {
                self.preview_note = notes::step_note(self.preview_note, isize::from(delta));
                self.message = Some(format!(
                    "Preview {}",
                    format_period(notes::period_at(self.preview_note))
                ));
                Ok(Outcome::None)
            }
            Command::ImportNudgeNote(delta) => {
                if let super::app::Overlay::Import(prompt) = &mut self.overlay {
                    prompt.note = notes::step_note(prompt.note, isize::from(delta));
                    prompt.rate_text = None;
                }
                Ok(Outcome::None)
            }
            Command::ImportNudgeFine(delta) => {
                if let super::app::Overlay::Import(prompt) = &mut self.overlay {
                    let next = i16::from(prompt.finetune) + i16::from(delta);
                    prompt.finetune = next.clamp(-8, 7) as i8;
                    prompt.rate_text = None;
                }
                Ok(Outcome::None)
            }
            Command::ImportToggleNormalize => {
                if let super::app::Overlay::Import(prompt) = &mut self.overlay {
                    prompt.normalize = !prompt.normalize;
                }
                Ok(Outcome::None)
            }
            Command::ImportToggleDither => {
                if let super::app::Overlay::Import(prompt) = &mut self.overlay {
                    prompt.dither = !prompt.dither;
                }
                Ok(Outcome::None)
            }
            Command::ImportRateArm => {
                if let super::app::Overlay::Import(prompt) = &mut self.overlay {
                    if prompt.rate_text.is_none() {
                        prompt.rate_text = Some(String::new());
                    }
                }
                Ok(Outcome::None)
            }
            Command::ImportRateDigit(ch) => {
                if let super::app::Overlay::Import(prompt) = &mut self.overlay {
                    if let Some(text) = &mut prompt.rate_text {
                        if text.len() < 7 {
                            text.push(ch);
                        }
                    }
                }
                Ok(Outcome::None)
            }
            Command::ImportRateBackspace => {
                if let super::app::Overlay::Import(prompt) = &mut self.overlay {
                    if let Some(text) = &mut prompt.rate_text {
                        text.pop();
                        if text.is_empty() {
                            prompt.rate_text = None;
                        }
                    }
                }
                Ok(Outcome::None)
            }
            Command::ImportConfirm => {
                self.confirm_import();
                Ok(Outcome::Edited)
            }
            other => Err(other),
        }
    }

    fn edit_current(
        &mut self,
        edit: impl FnOnce(&mut crate::Sample) -> Result<(), Error>,
        label: &str,
    ) -> Outcome {
        let slot = self.sample;
        match self.editor.edit_sample(&mut self.module, slot, edit) {
            Ok(true) => {
                self.notice = None;
                self.message = Some(label.to_string());
                Outcome::Edited
            }
            Ok(false) => {
                self.message = Some(format!("{label} (no change)"));
                Outcome::None
            }
            Err(err) => {
                self.message = Some(err.to_string());
                Outcome::None
            }
        }
    }

    fn audition(&mut self) -> Outcome {
        let sample = &self.module.samples[self.sample];
        if sample.data.is_empty() {
            self.message = Some("Sample is empty".to_string());
            return Outcome::None;
        }
        if self.playing {
            self.message = Some("Stop the song to preview the sample".to_string());
            return Outcome::None;
        }
        let period = notes::period_at(self.preview_note);
        let boosted = if sample.volume == 0 {
            " at volume 64"
        } else {
            ""
        };
        self.message = Some(format!("Preview {}{boosted}", format_period(period)));
        Outcome::Audition {
            slot: self.sample,
            period,
        }
    }

    fn open_path(&mut self, kind: PathKind) {
        let mut prompt = PathPrompt {
            kind,
            buffer: String::new(),
            entries: Vec::new(),
            selected: 0,
            list_dir: PathBuf::new(),
            error: None,
            rate: String::new(),
            rate_focus: false,
        };
        match kind {
            PathKind::ImportWav => {
                let mut text = self.start_dir().to_string_lossy().into_owned();
                if !text.ends_with('/') {
                    text.push('/');
                }
                prompt.buffer = text;
            }
            PathKind::ExportSample => {
                let mut path = self.start_dir();
                path.push(format!("sample-{:02}.wav", self.sample + 1));
                prompt.buffer = path.to_string_lossy().into_owned();
                prompt.rate = c2_rate(self.module.samples[self.sample].finetune_raw).to_string();
            }
            PathKind::ExportSong => {
                let mut path = self.start_dir();
                path.push("song.wav");
                prompt.buffer = path.to_string_lossy().into_owned();
            }
        }
        prompt.refresh();
        self.overlay = super::app::Overlay::Path(prompt);
    }

    fn start_dir(&self) -> PathBuf {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && parent.is_dir() {
                return parent.to_path_buf();
            }
        }
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    fn path_push(&mut self, ch: char) {
        let super::app::Overlay::Path(prompt) = &mut self.overlay else {
            return;
        };
        if prompt.rate_focus {
            if ch.is_ascii_digit() && prompt.rate.len() < 7 {
                prompt.rate.push(ch);
            }
        } else if prompt.buffer.chars().count() < 512 {
            prompt.buffer.push(ch);
            prompt.refresh();
        }
        if let super::app::Overlay::Path(prompt) = &mut self.overlay {
            prompt.error = None;
        }
    }

    fn path_backspace(&mut self) {
        let super::app::Overlay::Path(prompt) = &mut self.overlay else {
            return;
        };
        if prompt.rate_focus {
            prompt.rate.pop();
        } else {
            prompt.buffer.pop();
            prompt.refresh();
        }
        if let super::app::Overlay::Path(prompt) = &mut self.overlay {
            prompt.error = None;
        }
    }

    fn path_clear(&mut self) {
        let super::app::Overlay::Path(prompt) = &mut self.overlay else {
            return;
        };
        if prompt.rate_focus {
            prompt.rate.clear();
        } else {
            prompt.buffer.clear();
            prompt.refresh();
        }
    }

    fn path_move(&mut self, delta: isize) {
        let super::app::Overlay::Path(prompt) = &mut self.overlay else {
            return;
        };
        prompt.move_selection(delta);
    }

    fn confirm_path(&mut self) {
        let choice = {
            let super::app::Overlay::Path(prompt) = &mut self.overlay else {
                return;
            };
            let kind = prompt.kind;
            let rate = prompt.rate.clone();
            (prompt.confirm(), kind, rate)
        };
        match choice.0 {
            PathChoice::Stay => {}
            PathChoice::Import(path) => {
                let finetune = self.module.samples[self.sample].finetune();
                self.overlay = super::app::Overlay::Import(ImportPrompt {
                    path,
                    note: C2_NOTE,
                    finetune,
                    normalize: true,
                    dither: false,
                    rate_text: None,
                    error: None,
                });
            }
            PathChoice::Write(path) => self.write_path(choice.1, path, &choice.2),
        }
    }

    fn write_path(&mut self, kind: PathKind, path: PathBuf, rate_text: &str) {
        match kind {
            PathKind::ExportSample => {
                let finetune = self.module.samples[self.sample].finetune_raw;
                let rate = match parse_export_rate(rate_text, finetune) {
                    Ok(rate) => rate,
                    Err(message) => {
                        if let super::app::Overlay::Path(prompt) = &mut self.overlay {
                            prompt.error = Some(message);
                        }
                        return;
                    }
                };
                let data = self.module.samples[self.sample].data.clone();
                self.overlay = super::app::Overlay::None;
                match write_mono8_wav(&path, rate, &data) {
                    Ok(()) => {
                        self.message = Some(format!(
                            "Wrote {} ({} Hz, {} bytes)",
                            path.display(),
                            rate,
                            data.len()
                        ));
                    }
                    Err(err) => self.message = Some(err.to_string()),
                }
            }
            PathKind::ExportSong => {
                self.overlay = super::app::Overlay::None;
                let config = PlayerConfig::default();
                let max_frames = usize::try_from(u64::from(config.sample_rate).saturating_mul(600))
                    .unwrap_or(usize::MAX);
                match player::render_to_wav(&self.module, &path, config, max_frames) {
                    Ok(stats) => {
                        let seconds = stats.frames as f64 / f64::from(stats.sample_rate.max(1));
                        self.message = Some(format!(
                            "Wrote {} ({seconds:.2}s, {} Hz)",
                            path.display(),
                            stats.sample_rate
                        ));
                    }
                    Err(err) => self.message = Some(err.to_string()),
                }
            }
            PathKind::ImportWav => {}
        }
    }

    fn confirm_import(&mut self) {
        let super::app::Overlay::Import(prompt) = &self.overlay else {
            return;
        };
        let path = prompt.path.clone();
        let note = prompt.note;
        let finetune = prompt.finetune;
        let normalize = prompt.normalize;
        let dither = prompt.dither;
        let typed = prompt.rate_text.clone();
        let rate = if let Some(text) = typed {
            match text.parse::<u32>() {
                Ok(rate) if rate > 0 => rate,
                _ => {
                    if let super::app::Overlay::Import(prompt) = &mut self.overlay {
                        prompt.error = Some("Rate must be a number above 0".to_string());
                    }
                    return;
                }
            }
        } else {
            rate_for_note(note, finetune_nibble(finetune))
        };
        let options = ImportOptions {
            target_rate: rate,
            normalize,
            dither,
            dither_seed: DEFAULT_DITHER_SEED,
        };
        let imported = match std::fs::read(&path) {
            Err(err) => {
                self.overlay = super::app::Overlay::None;
                self.message = Some(format!("Failed to read {}: {err}", path.display()));
                return;
            }
            Ok(bytes) => match decode_wav(&bytes).and_then(|wav| import_pcm(&wav, &options)) {
                Ok(imported) => imported,
                Err(err) => {
                    self.overlay = super::app::Overlay::None;
                    self.message = Some(err.to_string());
                    return;
                }
            },
        };
        let warning = imported.warning.clone();
        let bytes = imported.data.len();
        let rate = imported.target_rate;
        let slot = self.sample;
        let stem = stem_name(&path);
        let result = self.editor.edit_sample(&mut self.module, slot, |sample| {
            sample.set_data(imported.data)?;
            sample.loop_start = 0;
            sample.loop_length = 1;
            sample_edit::set_finetune(sample, finetune)?;
            if sample.volume == 0 {
                sample.volume = 64;
            }
            if sample.display_name().is_empty() {
                if let Some(name) = &stem {
                    let _ = sample.set_name(name);
                }
            }
            Ok(())
        });
        self.overlay = super::app::Overlay::None;
        match result {
            Ok(_) => {
                let mut text = format!(
                    "Imported {} into sample {:02}, {bytes} bytes at {rate} Hz",
                    path.display(),
                    slot + 1
                );
                if let Some(warning) = warning {
                    text.push_str(". ");
                    text.push_str(&warning);
                    self.notice = Some(warning);
                } else {
                    self.notice = None;
                }
                self.message = Some(text);
            }
            Err(err) => self.message = Some(err.to_string()),
        }
    }

    fn open_field(&mut self, kind: FieldKind) {
        let sample = &self.module.samples[self.sample];
        let buffer = match kind {
            FieldKind::Volume => sample.volume.to_string(),
            FieldKind::Finetune => sample.finetune().to_string(),
            FieldKind::Loop => {
                let start = usize::from(sample.loop_start) * 2;
                let len = if sample.loops() {
                    usize::from(sample.loop_length) * 2
                } else {
                    0
                };
                format!("{start} {len}")
            }
            FieldKind::Trim => format!("0 {}", sample.data.len()),
            FieldKind::CopyTo => String::new(),
        };
        self.overlay = super::app::Overlay::Field(FieldPrompt {
            kind,
            buffer,
            error: None,
        });
    }

    fn confirm_field(&mut self) -> Outcome {
        let (kind, buffer) = match &self.overlay {
            super::app::Overlay::Field(prompt) => (prompt.kind, prompt.buffer.clone()),
            _ => return Outcome::None,
        };
        let slot = self.sample;
        let applied = match kind {
            FieldKind::Volume => parse_u8_range(&buffer, 0, 64, "volume").and_then(|volume| {
                self.editor
                    .edit_sample(&mut self.module, slot, |sample| {
                        sample_edit::set_volume(sample, volume)
                    })
                    .map(|changed| (changed, format!("Volume {volume}")))
            }),
            FieldKind::Finetune => {
                parse_i8_range(&buffer, -8, 7, "finetune").and_then(|finetune| {
                    self.editor
                        .edit_sample(&mut self.module, slot, |sample| {
                            sample_edit::set_finetune(sample, finetune)
                        })
                        .map(|changed| (changed, format!("Finetune {finetune:+}")))
                })
            }
            FieldKind::Loop => {
                parse_pair(&buffer, "loop start and length").and_then(|(start, length)| {
                    self.editor
                        .edit_sample(&mut self.module, slot, |sample| {
                            sample_edit::set_loop_bytes(sample, start, length)
                        })
                        .map(|changed| {
                            let label = if length <= 2 {
                                "Loop off".to_string()
                            } else {
                                format!("Loop {start}+{length}")
                            };
                            (changed, label)
                        })
                })
            }
            FieldKind::Trim => {
                parse_pair(&buffer, "trim start and end").and_then(|(start, end)| {
                    self.editor
                        .edit_sample(&mut self.module, slot, |sample| {
                            sample_edit::trim(sample, start, end)
                        })
                        .map(|changed| (changed, format!("Trimmed to {start}..{end}")))
                })
            }
            FieldKind::CopyTo => {
                parse_u8_range(&buffer, 1, u8::try_from(SAMPLE_COUNT).unwrap_or(31), "slot")
                    .and_then(|dest| {
                        let dest_index = usize::from(dest) - 1;
                        if dest_index == slot {
                            return Err(Error::SampleEdit(
                                "Choose a different slot to copy into".to_string(),
                            ));
                        }
                        self.editor
                            .copy_sample(&mut self.module, slot, dest_index)
                            .map(|changed| {
                                self.sample = dest_index;
                                (
                                    changed,
                                    format!("Copied sample {:02} to {dest:02}", slot + 1),
                                )
                            })
                    })
            }
        };
        match applied {
            Ok((changed, label)) => {
                self.overlay = super::app::Overlay::None;
                self.notice = None;
                self.message = Some(label);
                if changed {
                    Outcome::Edited
                } else {
                    Outcome::None
                }
            }
            Err(err) => {
                if let super::app::Overlay::Field(prompt) = &mut self.overlay {
                    prompt.error = Some(err.to_string());
                }
                Outcome::None
            }
        }
    }
}

enum PathChoice {
    Stay,
    Import(PathBuf),
    Write(PathBuf),
}

impl PathPrompt {
    fn refresh(&mut self) {
        let dir = listing_dir(&self.buffer);
        match read_dir_rows(&dir, self.kind) {
            Ok(entries) => {
                self.list_dir = dir;
                self.entries = entries;
                if self.selected >= self.entries.len() {
                    self.selected = self.entries.len().saturating_sub(1);
                }
                self.sync_selection();
            }
            Err(message) => {
                self.entries.clear();
                self.selected = 0;
                self.error = Some(message);
            }
        }
    }

    fn sync_selection(&mut self) {
        if self.buffer.ends_with('/') {
            return;
        }
        let Some(name) = Path::new(&self.buffer)
            .file_name()
            .and_then(|name| name.to_str())
        else {
            return;
        };
        if let Some(index) = self.entries.iter().position(|row| row.name == name) {
            self.selected = index;
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = step(self.selected, delta, self.entries.len());
        let row = self.entries[self.selected].clone();
        let dir = self.list_dir.clone();
        self.error = None;
        if row.name == ".." {
            if let Some(parent) = dir.parent() {
                let mut text = parent.to_string_lossy().into_owned();
                if text.is_empty() {
                    text = "/".to_string();
                }
                if !text.ends_with('/') {
                    text.push('/');
                }
                self.buffer = text;
            }
            return;
        }
        let path = dir.join(&row.name);
        if row.is_dir {
            let mut text = path.to_string_lossy().into_owned();
            if !text.ends_with('/') {
                text.push('/');
            }
            self.buffer = text;
        } else {
            self.buffer = path.to_string_lossy().into_owned();
        }
    }

    fn confirm(&mut self) -> PathChoice {
        let path = PathBuf::from(self.buffer.trim());
        if path.as_os_str().is_empty() {
            self.error = Some("Enter a path".to_string());
            return PathChoice::Stay;
        }
        if path.is_dir() {
            let mut text = path.to_string_lossy().into_owned();
            if !text.ends_with('/') {
                text.push('/');
            }
            self.buffer = text;
            self.error = None;
            self.refresh();
            return PathChoice::Stay;
        }
        match self.kind {
            PathKind::ImportWav => {
                if !path.is_file() {
                    self.error = Some(format!("No such file: {}", path.display()));
                    return PathChoice::Stay;
                }
                PathChoice::Import(path)
            }
            PathKind::ExportSample | PathKind::ExportSong => {
                if self.buffer.ends_with('/') {
                    self.error = Some("Enter a file name".to_string());
                    return PathChoice::Stay;
                }
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() && !parent.is_dir() {
                        self.error =
                            Some(format!("Directory does not exist: {}", parent.display()));
                        return PathChoice::Stay;
                    }
                }
                PathChoice::Write(path)
            }
        }
    }
}

fn listing_dir(buffer: &str) -> PathBuf {
    let path = Path::new(buffer);
    if buffer.is_empty() {
        return PathBuf::from(".");
    }
    if buffer.ends_with('/') {
        return path.to_path_buf();
    }
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn read_dir_rows(dir: &Path, kind: PathKind) -> Result<Vec<DirRow>, String> {
    let read =
        std::fs::read_dir(dir).map_err(|err| format!("Cannot open {}: {err}", dir.display()))?;
    let mut rows = Vec::new();
    if dir.parent().is_some() {
        rows.push(DirRow {
            name: "..".to_string(),
            is_dir: true,
        });
    }
    let mut files = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        if !is_dir && kind == PathKind::ImportWav && !name.to_ascii_lowercase().ends_with(".wav") {
            continue;
        }
        files.push(DirRow { name, is_dir });
    }
    files.sort_by(|left, right| {
        right.is_dir.cmp(&left.is_dir).then_with(|| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        })
    });
    rows.extend(files);
    Ok(rows)
}

fn step(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let current = isize::try_from(current.min(len - 1)).unwrap_or(0);
    let next = current.saturating_add(delta);
    if next <= 0 {
        0
    } else {
        usize::try_from(next).unwrap_or(len - 1).min(len - 1)
    }
}

fn parse_export_rate(text: &str, finetune_raw: u8) -> Result<u32, String> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("c2") {
        return Ok(c2_rate(finetune_raw));
    }
    text.parse::<u32>()
        .ok()
        .filter(|rate| *rate > 0)
        .ok_or_else(|| format!("Rate must be above 0, or blank for C-2, got {text}"))
}

fn finetune_nibble(finetune: i8) -> u8 {
    if finetune < 0 {
        u8::try_from(finetune + 16).unwrap_or(0)
    } else {
        u8::try_from(finetune).unwrap_or(0)
    }
}

fn stem_name(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    let text: String = stem
        .chars()
        .filter(|ch| u32::from(*ch) <= 0xFF)
        .take(22)
        .collect();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn parse_u8_range(text: &str, min: u8, max: u8, what: &str) -> Result<u8, Error> {
    let value = text
        .trim()
        .parse::<u16>()
        .map_err(|_| Error::SampleEdit(format!("{what} expects a number, got {}", text.trim())))?;
    if value < u16::from(min) || value > u16::from(max) {
        return Err(Error::SampleEdit(format!(
            "{what} {value} is outside {min}..={max}"
        )));
    }
    u8::try_from(value).map_err(|_| Error::SampleEdit(format!("{what} {value} does not fit")))
}

fn parse_i8_range(text: &str, min: i8, max: i8, what: &str) -> Result<i8, Error> {
    let value = text
        .trim()
        .parse::<i16>()
        .map_err(|_| Error::SampleEdit(format!("{what} expects a number, got {}", text.trim())))?;
    if value < i16::from(min) || value > i16::from(max) {
        return Err(Error::SampleEdit(format!(
            "{what} {value} is outside {min}..={max}"
        )));
    }
    i8::try_from(value).map_err(|_| Error::SampleEdit(format!("{what} {value} does not fit")))
}

fn parse_pair(text: &str, what: &str) -> Result<(usize, usize), Error> {
    let mut parts = text.split([' ', ',', '+']).filter(|part| !part.is_empty());
    let start = parts.next();
    let end = parts.next();
    let (Some(start), Some(end)) = (start, end) else {
        return Err(Error::SampleEdit(format!(
            "{what}: enter two numbers, got {}",
            text.trim()
        )));
    };
    if parts.next().is_some() {
        return Err(Error::SampleEdit(format!(
            "{what}: enter two numbers, got {}",
            text.trim()
        )));
    }
    let start = start
        .parse::<usize>()
        .map_err(|_| Error::SampleEdit(format!("{what}: {start} is not a number")))?;
    let end = end
        .parse::<usize>()
        .map_err(|_| Error::SampleEdit(format!("{what}: {end} is not a number")))?;
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    use crate::tui::app::Overlay;
    use crate::tui::{command_for, App, Command, Focus, Key};
    use crate::Module;

    fn focus_samples(app: &mut App) {
        if app.editing {
            app.apply(Command::ToggleEdit);
        }
        if app.focus != Focus::Samples {
            app.apply(Command::NextFocus);
        }
        assert_eq!(app.focus, Focus::Samples);
    }

    #[test]
    fn sample_edits_share_the_note_undo_stack() {
        let mut app = App::new(Module::default());
        app.module.samples[0]
            .set_data(vec![10, 20, 30, 40, 50, 60, 70, 80])
            .unwrap();
        app.apply(Command::ToggleEdit);
        app.apply(Command::EnterNote(0));
        let period = app.module.patterns[0].rows[0][0].period;
        assert_ne!(period, 0);
        focus_samples(&mut app);

        app.apply(command_for(&app, Key::Char('n')).unwrap());
        assert_eq!(app.module.samples[0].data[7] as i8, 127);
        assert_eq!(app.module.patterns[0].rows[0][0].period, period);
        assert!(app.is_dirty());

        app.apply(command_for(&app, Key::Char('u')).unwrap());
        assert_eq!(app.module.samples[0].data[7], 80);
        assert_eq!(app.module.patterns[0].rows[0][0].period, period);

        app.apply(Command::Undo);
        assert_eq!(app.module.patterns[0].rows[0][0].period, 0);
        assert_eq!(app.module.samples[0].data[7], 80);
        app.apply(Command::Redo);
        assert_eq!(app.module.patterns[0].rows[0][0].period, period);
        assert_eq!(app.module.samples[0].data[7], 80);
        app.apply(Command::Redo);
        assert_eq!(app.module.samples[0].data[7] as i8, 127);
        assert!(!app.editor.can_redo());
    }

    #[test]
    fn r_renames_and_w_reverses() {
        let mut app = App::new(Module::default());
        app.module.samples[0].set_data(vec![1, 2, 3, 4]).unwrap();
        focus_samples(&mut app);
        assert!(matches!(
            command_for(&app, Key::Char('R')),
            Some(Command::BeginSampleName)
        ));
        app.apply(command_for(&app, Key::Char('w')).unwrap());
        assert_eq!(app.module.samples[0].data, vec![4, 3, 2, 1]);
        app.apply(Command::Undo);
        assert_eq!(app.module.samples[0].data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn missing_wav_stays_on_the_picker() {
        let mut app = App::new(Module::default());
        focus_samples(&mut app);
        app.apply(Command::BeginImport);
        app.apply(Command::PathClear);
        for ch in "/tmp/omatrack-no-such-sample.wav".chars() {
            app.apply(Command::PathPush(ch));
        }
        app.apply(Command::PathConfirm);
        match &app.overlay {
            Overlay::Path(prompt) => {
                let err = prompt.error.as_deref().unwrap();
                assert!(err.contains("No such file"), "{err}");
            }
            other => panic!("expected the path prompt, got {other:?}"),
        }
        assert!(app.module.samples[0].data.is_empty());
    }
}
