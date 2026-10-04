//! Cursor, edit mode, and key bindings.
//!
//! [`crate::edit::Editor`] mutates the module. [`App`] remembers the cursor,
//! the block selection, the clipboard, and whether the song is playing.

use std::path::PathBuf;

use crate::edit::{
    clamp_octave, clamp_step, editable_text, is_name_char, period_at, semitone_from_key, CellRange,
    Clipboard, Editor, Field, PatternCursor, Place, Selection, DEFAULT_OCTAVE, DEFAULT_STEP,
};
use crate::module::{
    Cell, Module, CHANNELS, ORDER_LEN, ROWS, SAMPLE_COUNT, SAMPLE_NAME_LEN, TITLE_LEN,
};
use crate::player::{PlayerConfig, DEFAULT_SPEED, DEFAULT_TEMPO};

use super::sample::PathKind;
use super::theme::Theme;

/// Which pane receives movement keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The pattern editor.
    Pattern,
    /// The instrument list.
    Samples,
    /// The order list in the song header.
    Order,
}

/// A key the view understands, after the terminal crate has been translated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable character, including shifted letters.
    Char(char),
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Home.
    Home,
    /// End.
    End,
    /// Tab.
    Tab,
    /// Shift-tab.
    BackTab,
    /// Escape.
    Esc,
    /// Enter.
    Enter,
    /// Backspace.
    Backspace,
    /// Delete.
    Delete,
    /// Insert.
    Insert,
    /// Function key, starting at 1.
    F(u8),
    /// Ctrl plus a letter.
    Ctrl(char),
    /// Ctrl-Backspace.
    CtrlBackspace,
    /// Ctrl-Delete.
    CtrlDelete,
    /// Ctrl-Insert.
    CtrlInsert,
    /// Alt plus a letter or digit.
    Alt(char),
    /// Alt-Up.
    AltUp,
    /// Alt-Down.
    AltDown,
    /// Alt-Left.
    AltLeft,
    /// Alt-Right.
    AltRight,
}

/// One change to the view or the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Quit, or ask first when the song is modified.
    QuitAsk,
    /// Quit and drop unsaved edits.
    QuitDiscard,
    /// Write the file, then quit.
    SaveAndQuit,
    /// Write the file.
    Save,
    /// Move the pattern row by `delta`, clamped to `0..64`.
    MoveRow(isize),
    /// Move the channel cursor by `delta`, clamped to `0..4`.
    MoveChannel(isize),
    /// Move one cell field left or right, crossing channels.
    MoveField(isize),
    /// Jump to row 0.
    FirstRow,
    /// Jump to row 63.
    LastRow,
    /// Next channel, wrapping, keeping the field.
    NextChannel,
    /// Previous channel, wrapping, keeping the field.
    PrevChannel,
    /// Move through the played order list. The viewed pattern follows.
    MoveOrder(isize),
    /// Move to another pattern without changing the order position.
    MovePattern(isize),
    /// Focus the next pane.
    NextFocus,
    /// Focus the previous pane.
    PrevFocus,
    /// Move the sample cursor by `delta`.
    MoveSample(isize),
    /// Jump to instrument 1.
    FirstSample,
    /// Jump to instrument 31.
    LastSample,
    /// Jump to the first played order position.
    FirstOrder,
    /// Jump to the last played order position.
    LastOrder,
    /// Start playback from the cursor, or stop it.
    TogglePlay,
    /// Silence or restore channel `0..4`.
    ToggleMute(usize),
    /// Enter or leave edit mode.
    ToggleEdit,
    /// Change the piano octave by `delta`.
    Octave(i8),
    /// Change the edit step by `delta`.
    Step(i8),
    /// Enter the piano semitone (`0` is C of the current octave).
    EnterNote(u8),
    /// Enter a sample or effect digit.
    EnterDigit(u8),
    /// Clear the cell, or one digit when the cursor is on a digit.
    ClearUnderCursor,
    /// Clear the cell one edit-step up and move there.
    BackspaceLine,
    /// Insert an empty cell in this channel.
    InsertChannelRow,
    /// Delete this channel's cell and shift the channel up.
    DeleteChannelRow,
    /// Insert an empty row in every channel.
    InsertPatternRow,
    /// Delete the row in every channel.
    DeletePatternRow,
    /// Empty the current channel.
    ClearChannel,
    /// Empty the current pattern.
    ClearPattern,
    /// Start a block at the cursor, or clear it.
    ToggleBlock,
    /// Select every cell in the current pattern.
    SelectAll,
    /// Drop the block highlight.
    ClearBlock,
    /// Copy the block, or the current cell.
    Copy,
    /// Copy the block and clear it.
    Cut,
    /// Paste the clipboard at the cursor.
    Paste,
    /// Transpose the block, or the current cell, by semitones.
    Transpose(i32),
    /// Undo the last edit.
    Undo,
    /// Redo the last undone edit.
    Redo,
    /// Show the key overlay.
    ShowHelp,
    /// Close help, the quit question, or text entry.
    CloseOverlay,
    /// Change the pattern number at the current order position.
    OrderPattern(i32),
    /// Insert an order entry at the cursor.
    InsertOrder,
    /// Delete the current order entry.
    DeleteOrder,
    /// Change the played song length.
    SongLength(i32),
    /// Append an empty pattern and point the current order entry at it.
    NewPattern,
    /// Edit the song title.
    BeginTitle,
    /// Edit the current sample name.
    BeginSampleName,
    /// Append a character to the text prompt.
    TextPush(char),
    /// Drop the last character in the text prompt.
    TextBackspace,
    /// Store the text prompt.
    TextConfirm,
    /// Drop the text prompt.
    TextCancel,
    /// Open the WAV import picker for the current sample.
    BeginImport,
    /// Open the sample WAV export picker.
    BeginExportSample,
    /// Open the song-render picker. Same mix as `--render`.
    BeginExportSong,
    /// Append a character to the path prompt.
    PathPush(char),
    /// Delete the last path character.
    PathBackspace,
    /// Clear the path, or the export rate when that field is focused.
    PathClear,
    /// Move the file-list highlight.
    PathMove(isize),
    /// Switch between the path and the export rate.
    PathFocusRate,
    /// Open a directory, or accept the path.
    PathConfirm,
    /// Ask for a new volume.
    BeginVolume,
    /// Ask for a new finetune.
    BeginFinetune,
    /// Ask for loop start and length, in bytes.
    BeginLoop,
    /// Ask for a trim range.
    BeginTrim,
    /// Ask which slot to copy the current sample onto.
    BeginCopySample,
    /// Append a character to a sample field prompt.
    FieldPush(char),
    /// Delete the last field character.
    FieldBackspace,
    /// Clear the field prompt.
    FieldClear,
    /// Apply the field prompt.
    FieldConfirm,
    /// Loop the whole sample, or turn the loop off.
    ToggleSampleLoop,
    /// Peak-normalize the current sample.
    NormalizeSample,
    /// Reverse the current sample. `R` still renames it.
    ReverseSample,
    /// Fade the current sample in from silence.
    FadeIn,
    /// Fade the current sample out to silence.
    FadeOut,
    /// Drop the PCM and the loop. The name stays.
    ClearSampleData,
    /// Play the current sample at the preview note.
    AuditionSample,
    /// Move the preview note by semitones.
    PreviewNote(i8),
    /// Move the import base note.
    ImportNudgeNote(i8),
    /// Move the import finetune.
    ImportNudgeFine(i8),
    /// Toggle peak normalize on the import dialog.
    ImportToggleNormalize,
    /// Toggle dither on the import dialog.
    ImportToggleDither,
    /// Start typing a custom import rate.
    ImportRateArm,
    /// Type one digit of a custom import rate.
    ImportRateDigit(char),
    /// Delete one digit of a custom import rate.
    ImportRateBackspace,
    /// Convert the chosen WAV into the current sample.
    ImportConfirm,
    /// Open the file menu.
    ShowFile,
    /// New module, asking first when the song is modified.
    NewModule,
    /// Open a module, asking first when the song is modified.
    OpenModule,
    /// Ask for a path and write the module there.
    SaveAs,
    /// Save, then run the pending file action.
    GuardSave,
    /// Drop edits and run the pending file action.
    GuardDiscard,
}

/// What the audio side should do after [`App::apply`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The view changed and the song did not.
    None,
    /// The module changed.
    Edited,
    /// A note was written and should be heard when playback is stopped.
    Preview {
        /// Instrument number, `1..=31`.
        sample: u8,
        /// Finetune-0 period.
        period: u16,
        /// Channel the note was written on.
        channel: usize,
    },
    /// Play or stop.
    Play,
    /// Channel mute flipped.
    Mute(usize),
    /// Write the module to its path.
    Save,
    /// Write the module, then quit if the write worked.
    SaveAndQuit,
    /// Audition the selected sample through the mixer.
    Audition {
        /// Zero-based sample slot.
        slot: usize,
        /// Finetune-0 period.
        period: u16,
    },
    /// The command wrote the module itself. Run any pending file action.
    Saved,
    /// The in-memory module was replaced. Stop the stream.
    DocumentReplaced,
}

/// What to do after a save that was started by new, open, or quit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Followup {
    /// Leave the tracker.
    Quit,
    /// Replace the song with an empty module.
    New,
    /// Ask for a module to load.
    Open,
}

/// What is covering the tracker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Overlay {
    /// The pattern and the lists.
    None,
    /// Key binding list.
    Help,
    /// Ask before discarding edits.
    Quit,
    /// Title or sample name.
    Text {
        /// Which field is being typed.
        target: TextTarget,
        /// Characters typed so far.
        buffer: String,
    },
    /// A directory listing and a path.
    Path(super::sample::PathPrompt),
    /// How to resample a WAV that was just chosen.
    Import(super::sample::ImportPrompt),
    /// Volume, finetune, loop, trim, or a copy destination.
    Field(super::sample::FieldPrompt),
    /// New, open, save, and save as.
    File,
    /// Ask before new, open, or another action that would drop edits.
    Guard(Followup),
}

/// Title or one sample name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextTarget {
    /// The 20-byte song title.
    Title,
    /// A sample name. The index is zero-based.
    Sample(usize),
}

const PATTERN_PAGE: isize = 16;
const SAMPLE_PAGE: isize = 8;

/// Tracker state: the document, the cursor, and the undo stack.
#[derive(Debug)]
pub struct App {
    pub(crate) module: Module,
    pub(crate) path: PathBuf,
    pub(crate) focus: Focus,
    pub(crate) editing: bool,
    pub(crate) field: Field,
    pub(crate) octave: u8,
    pub(crate) step: u8,
    pub(crate) order_pos: usize,
    pub(crate) view_pattern: usize,
    pub(crate) row: usize,
    pub(crate) channel: usize,
    pub(crate) sample: usize,
    pub(crate) row_offset: usize,
    pub(crate) sample_offset: usize,
    /// The replayer is pulling audio.
    pub(crate) playing: bool,
    /// Ticks per row, from the replayer.
    pub(crate) speed: u8,
    /// CIA tempo, from the replayer.
    pub(crate) tempo: u8,
    /// Per-channel mute. Index 0 is channel 1.
    pub(crate) muted: [bool; CHANNELS],
    /// Last failure from opening the audio device, shown in the transport bar.
    pub(crate) audio_error: Option<String>,
    pub(crate) message: Option<String>,
    /// [`Self::message`] is a failure, drawn in the error color.
    pub(crate) message_error: bool,
    /// A conversion warning that stays up after the next key clears [`Self::message`].
    pub(crate) notice: Option<String>,
    /// Colors for this session.
    pub(crate) theme: Theme,
    /// Name shown in the song header.
    pub(crate) theme_label: String,
    /// Mixer knobs for preview, playback, and the in-app render.
    pub(crate) player: PlayerConfig,
    /// Cap for the in-app song render, in seconds.
    pub(crate) max_seconds: f64,
    /// Set while a save should be followed by new, open, or quit.
    pub(crate) followup: Option<Followup>,
    /// Finetune-0 note index used by sample audition. C-2 until `-` or `=` moves it.
    pub(crate) preview_note: usize,
    pub(crate) overlay: Overlay,
    pub(crate) selection: Option<Selection>,
    clipboard: Clipboard,
    pub(crate) editor: Editor,
    quit: bool,
}

impl App {
    /// Open `module` on order position 0, row 0, channel 1, in browse mode.
    pub fn new(module: Module) -> Self {
        Self::open(module, PathBuf::new())
    }

    /// Open `module` and remember `path` for Ctrl-S.
    pub fn open(module: Module, path: PathBuf) -> Self {
        let view_pattern = usize::from(module.order[0]);
        Self {
            module,
            path,
            focus: Focus::Pattern,
            editing: false,
            field: Field::Note,
            octave: DEFAULT_OCTAVE,
            step: DEFAULT_STEP,
            order_pos: 0,
            view_pattern,
            row: 0,
            channel: 0,
            sample: 0,
            row_offset: 0,
            sample_offset: 0,
            playing: false,
            speed: DEFAULT_SPEED,
            tempo: DEFAULT_TEMPO,
            muted: [false; CHANNELS],
            audio_error: None,
            message: None,
            message_error: false,
            notice: None,
            theme: Theme::protracker(),
            theme_label: "protracker".to_string(),
            player: PlayerConfig::default(),
            max_seconds: crate::config::DEFAULT_MAX_SECONDS,
            followup: None,
            preview_note: crate::notes::C2_NOTE,
            overlay: Overlay::None,
            selection: None,
            clipboard: Clipboard::default(),
            editor: Editor::new(),
            quit: false,
        }
    }

    /// Snap the view to the row the replayer is mixing.
    pub(crate) fn follow(&mut self, order: usize, row: usize, speed: u8, tempo: u8) {
        self.set_clock(speed, tempo);
        let len = self.song_len();
        self.order_pos = order.min(len.saturating_sub(1));
        let pattern = usize::from(self.module.order[self.order_pos]);
        if pattern != self.view_pattern {
            self.selection = None;
            if pattern < self.module.patterns.len() {
                self.view_pattern = pattern;
            }
        }
        self.row = row.min(ROWS - 1);
    }

    /// Remember speed and tempo without moving the cursor.
    pub(crate) fn set_clock(&mut self, speed: u8, tempo: u8) {
        self.speed = speed;
        self.tempo = tempo;
    }

    /// Remember that the audio device could not be opened, and leave playback stopped.
    pub(crate) fn fail_audio(&mut self, message: String) {
        self.playing = false;
        self.audio_error = Some(message);
    }

    /// Show `message` on the status line until the next key.
    pub(crate) fn set_message(&mut self, message: impl Into<String>) {
        self.message_error = false;
        self.message = Some(message.into());
    }

    /// Show `message` in the error color until the next key.
    pub(crate) fn set_error(&mut self, message: impl Into<String>) {
        self.message_error = true;
        self.message = Some(message.into());
    }

    /// Replace the palette. An Omarchy reload calls this without resetting the song.
    pub fn set_theme(&mut self, theme: Theme, label: impl Into<String>) {
        self.theme = theme;
        self.theme_label = label.into();
    }

    /// Octave, step, and mixer settings from the config file or the command line.
    pub fn set_preferences(
        &mut self,
        octave: u8,
        step: u8,
        player: PlayerConfig,
        max_seconds: f64,
    ) {
        self.octave = octave;
        self.step = step;
        self.player = player;
        self.max_seconds = max_seconds;
    }

    /// Take the action armed by a save-and-continue prompt.
    pub(crate) fn take_followup(&mut self) -> Option<Followup> {
        self.followup.take()
    }

    /// Leave the UI even if a prompt is up.
    pub(crate) fn quit_now(&mut self) {
        self.overlay = Overlay::None;
        self.quit = true;
    }

    /// Whether the user asked to quit.
    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// The document has edits since the last successful save.
    pub fn is_dirty(&self) -> bool {
        self.editor.is_dirty()
    }

    /// Write the module to the path it was opened from.
    pub fn save(&mut self) -> Result<(), String> {
        if self.path.as_os_str().is_empty() {
            return Err("there is no file to save".to_string());
        }
        self.module
            .save(&self.path)
            .map_err(|err| err.to_string())?;
        self.editor.mark_saved();
        Ok(())
    }

    /// Apply one command. Movement past either end sticks.
    pub fn apply(&mut self, command: Command) -> Outcome {
        self.message = None;
        self.message_error = false;
        match command {
            Command::Save => self.save_command(),
            Command::SaveAndQuit => {
                self.followup = Some(Followup::Quit);
                self.save_command()
            }
            Command::GuardSave => self.save_command(),
            Command::GuardDiscard => self.discard_followup(),
            Command::ShowFile => {
                self.overlay = Overlay::File;
                Outcome::None
            }
            Command::NewModule => self.request_new(),
            Command::OpenModule => self.request_open(),
            Command::SaveAs => {
                self.prompt_path(PathKind::SaveModule);
                Outcome::None
            }
            Command::QuitAsk => self.ask_quit(),
            Command::QuitDiscard => {
                self.quit_now();
                Outcome::None
            }
            Command::CloseOverlay | Command::TextCancel => {
                self.followup = None;
                self.overlay = Overlay::None;
                Outcome::None
            }
            Command::ShowHelp => {
                self.overlay = Overlay::Help;
                Outcome::None
            }
            Command::TogglePlay => {
                self.playing = !self.playing;
                if self.playing {
                    self.audio_error = None;
                }
                Outcome::Play
            }
            Command::ToggleMute(channel) => {
                if let Some(muted) = self.muted.get_mut(channel) {
                    *muted = !*muted;
                    Outcome::Mute(channel)
                } else {
                    Outcome::None
                }
            }
            Command::ToggleEdit => {
                self.editing = !self.editing;
                if self.editing {
                    self.focus = Focus::Pattern;
                    self.field = Field::Note;
                }
                Outcome::None
            }
            Command::Octave(delta) => {
                self.octave = clamp_octave(i32::from(self.octave) + i32::from(delta));
                Outcome::None
            }
            Command::Step(delta) => {
                self.step = clamp_step(i32::from(self.step) + i32::from(delta));
                Outcome::None
            }
            Command::TextPush(ch) => {
                self.push_text(ch);
                Outcome::None
            }
            Command::TextBackspace => {
                if let Overlay::Text { buffer, .. } = &mut self.overlay {
                    buffer.pop();
                }
                Outcome::None
            }
            Command::TextConfirm => self.confirm_text(),
            other if is_motion(other) => {
                self.apply_motion(other);
                Outcome::None
            }
            other => match self.apply_sample(other) {
                Ok(outcome) => outcome,
                Err(command) => self.apply_change(command),
            },
        }
    }

    /// Keep the cursors inside the rows the panes can show.
    pub(crate) fn reconcile_scroll(&mut self, pattern_window: usize, sample_window: usize) {
        self.row_offset = window_start(self.row_offset, self.row, ROWS, pattern_window);
        self.sample_offset =
            window_start(self.sample_offset, self.sample, SAMPLE_COUNT, sample_window);
    }

    /// Played order length, at least 1 so the cursor always has a slot.
    pub(crate) fn song_len(&self) -> usize {
        usize::from(self.module.song_length).clamp(1, ORDER_LEN)
    }

    /// Pattern number stored at the current order position.
    pub(crate) fn order_pattern(&self) -> u8 {
        let last = self.song_len().saturating_sub(1);
        self.module.order[self.order_pos.min(last)]
    }

    /// Cell under the pattern cursor, if that pattern exists.
    pub(crate) fn current_cell(&self) -> Option<Cell> {
        let pattern = self.module.patterns.get(self.view_pattern)?;
        Some(pattern.rows[self.row][self.channel])
    }

    /// Whether the block highlight covers this cell on the pattern being shown.
    pub(crate) fn cell_selected(&self, row: usize, channel: usize) -> bool {
        self.selection.is_some_and(|selection| {
            selection.pattern == self.view_pattern && selection.range().contains(row, channel)
        })
    }

    fn save_command(&mut self) -> Outcome {
        if self.path.as_os_str().is_empty() {
            self.prompt_path(PathKind::SaveModule);
            Outcome::None
        } else {
            self.overlay = Overlay::None;
            Outcome::Save
        }
    }

    fn request_new(&mut self) -> Outcome {
        if self.is_dirty() {
            self.followup = Some(Followup::New);
            self.overlay = Overlay::Guard(Followup::New);
            Outcome::None
        } else {
            self.install_blank()
        }
    }

    fn request_open(&mut self) -> Outcome {
        if self.is_dirty() {
            self.followup = Some(Followup::Open);
            self.overlay = Overlay::Guard(Followup::Open);
            Outcome::None
        } else {
            self.prompt_path(PathKind::OpenModule);
            Outcome::None
        }
    }

    fn discard_followup(&mut self) -> Outcome {
        let followup = self.followup.take();
        self.overlay = Overlay::None;
        match followup {
            Some(Followup::New) => self.install_blank(),
            Some(Followup::Open) => {
                self.prompt_path(PathKind::OpenModule);
                Outcome::None
            }
            Some(Followup::Quit) => {
                self.quit_now();
                Outcome::None
            }
            None => Outcome::None,
        }
    }

    /// Replace the song with an empty module and stop playback.
    pub(crate) fn install_blank(&mut self) -> Outcome {
        self.module = Module::new(crate::module::Tag::Mk);
        self.path.clear();
        self.editor = Editor::new();
        self.clipboard = Clipboard::default();
        self.finish_replaced("New module")
    }

    /// Replace the song with `module` from `path` and stop playback.
    pub(crate) fn install_loaded(&mut self, module: Module, path: PathBuf) -> Outcome {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("module");
        let message = format!("Opened {name}");
        self.module = module;
        self.path = path;
        self.editor = Editor::new();
        self.clipboard = Clipboard::default();
        self.finish_replaced(&message)
    }

    fn finish_replaced(&mut self, message: &str) -> Outcome {
        self.reset_view();
        self.playing = false;
        self.message = Some(message.to_string());
        Outcome::DocumentReplaced
    }

    fn reset_view(&mut self) {
        self.focus = Focus::Pattern;
        self.editing = false;
        self.field = Field::Note;
        self.order_pos = 0;
        let pattern = usize::from(self.module.order[0]);
        self.view_pattern = if pattern < self.module.patterns.len() {
            pattern
        } else {
            0
        };
        self.row = 0;
        self.channel = 0;
        self.sample = 0;
        self.row_offset = 0;
        self.sample_offset = 0;
        self.selection = None;
        self.overlay = Overlay::None;
        self.audio_error = None;
        self.notice = None;
        self.speed = DEFAULT_SPEED;
        self.tempo = DEFAULT_TEMPO;
        self.muted = [false; CHANNELS];
    }

    fn ask_quit(&mut self) -> Outcome {
        if self.editor.is_dirty() {
            self.overlay = Overlay::Quit;
            Outcome::None
        } else {
            self.quit = true;
            Outcome::None
        }
    }

    fn apply_motion(&mut self, command: Command) {
        match command {
            Command::MoveRow(delta) => {
                self.row = step(self.row, delta, ROWS);
                self.nudge_selection();
            }
            Command::MoveChannel(delta) => {
                self.channel = step(self.channel, delta, CHANNELS);
                self.nudge_selection();
            }
            Command::MoveField(delta) => {
                let (channel, field) = crate::edit::shift_field(self.channel, self.field, delta);
                self.channel = channel;
                self.field = field;
                self.nudge_selection();
            }
            Command::FirstRow => {
                self.row = 0;
                self.nudge_selection();
            }
            Command::LastRow => {
                self.row = ROWS - 1;
                self.nudge_selection();
            }
            Command::NextChannel => {
                self.channel = (self.channel + 1) % CHANNELS;
                self.nudge_selection();
            }
            Command::PrevChannel => {
                self.channel = (self.channel + CHANNELS - 1) % CHANNELS;
                self.nudge_selection();
            }
            Command::MoveOrder(delta) => {
                let len = self.song_len();
                self.order_pos = step(self.order_pos, delta, len);
                self.retarget_pattern(usize::from(self.module.order[self.order_pos]));
            }
            Command::MovePattern(delta) => {
                let len = self.module.patterns.len().max(1);
                let next = step(self.view_pattern, delta, len);
                self.retarget_pattern(next);
            }
            Command::NextFocus => self.focus = cycle(self.focus, true),
            Command::PrevFocus => self.focus = cycle(self.focus, false),
            Command::MoveSample(delta) => self.sample = step(self.sample, delta, SAMPLE_COUNT),
            Command::FirstSample => self.sample = 0,
            Command::LastSample => self.sample = SAMPLE_COUNT - 1,
            Command::FirstOrder => {
                self.order_pos = 0;
                self.retarget_pattern(usize::from(self.module.order[0]));
            }
            Command::LastOrder => {
                self.order_pos = self.song_len().saturating_sub(1);
                self.retarget_pattern(usize::from(self.module.order[self.order_pos]));
            }
            _ => {}
        }
    }

    fn apply_change(&mut self, command: Command) -> Outcome {
        let before = self.editor.undo_len();
        match command {
            Command::EnterNote(semitone) => self.enter_note(semitone),
            Command::EnterDigit(digit) => {
                let place = self.place();
                if let Some(next) = self.editor.enter_digit(&mut self.module, place, digit) {
                    self.take_cursor(next);
                }
                self.edited(before)
            }
            Command::ClearUnderCursor => {
                let place = self.place();
                self.editor.clear_at(&mut self.module, place);
                self.edited(before)
            }
            Command::BackspaceLine => {
                let place = self.place();
                if let Some(next) = self.editor.backspace(&mut self.module, place) {
                    self.row = next.row;
                    self.field = next.field;
                    self.nudge_selection();
                }
                self.edited(before)
            }
            Command::InsertChannelRow => {
                self.editor.insert_channel_row(
                    &mut self.module,
                    self.view_pattern,
                    self.channel,
                    self.row,
                );
                self.edited(before)
            }
            Command::DeleteChannelRow => {
                self.editor.delete_channel_row(
                    &mut self.module,
                    self.view_pattern,
                    self.channel,
                    self.row,
                );
                self.edited(before)
            }
            Command::InsertPatternRow => {
                self.editor
                    .insert_pattern_row(&mut self.module, self.view_pattern, self.row);
                self.edited(before)
            }
            Command::DeletePatternRow => {
                self.editor
                    .delete_pattern_row(&mut self.module, self.view_pattern, self.row);
                self.edited(before)
            }
            Command::ClearChannel => {
                self.editor
                    .clear_channel(&mut self.module, self.view_pattern, self.channel);
                self.edited(before)
            }
            Command::ClearPattern => {
                self.editor
                    .clear_pattern(&mut self.module, self.view_pattern);
                self.selection = None;
                self.edited(before)
            }
            Command::ToggleBlock => {
                if self.selection.is_some() {
                    self.selection = None;
                } else {
                    self.selection = Some(Selection {
                        pattern: self.view_pattern,
                        anchor_row: self.row,
                        anchor_channel: self.channel,
                        row: self.row,
                        channel: self.channel,
                    });
                }
                Outcome::None
            }
            Command::SelectAll => {
                self.selection = Some(Selection {
                    pattern: self.view_pattern,
                    anchor_row: 0,
                    anchor_channel: 0,
                    row: ROWS - 1,
                    channel: CHANNELS - 1,
                });
                Outcome::None
            }
            Command::ClearBlock => {
                self.selection = None;
                Outcome::None
            }
            Command::Copy => {
                self.copy_block();
                Outcome::None
            }
            Command::Cut => {
                let range = self.active_range();
                self.clipboard = Editor::copy_range(&self.module, self.view_pattern, range);
                self.editor.cut(&mut self.module, self.view_pattern, range);
                self.selection = None;
                self.message = Some(format!("Cut {}x{}", range.rows(), range.channels()));
                self.edited(before)
            }
            Command::Paste => {
                if self.clipboard.is_empty() {
                    self.message = Some("Clipboard is empty".to_string());
                    return Outcome::None;
                }
                self.editor.paste(
                    &mut self.module,
                    self.view_pattern,
                    self.row,
                    self.channel,
                    &self.clipboard,
                );
                self.message = Some("Pasted".to_string());
                self.edited(before)
            }
            Command::Transpose(semitones) => {
                let range = self.active_range();
                self.editor
                    .transpose(&mut self.module, self.view_pattern, range, semitones);
                self.edited(before)
            }
            Command::Undo => {
                if self.editor.undo(&mut self.module) {
                    self.clamp_position();
                    Outcome::Edited
                } else {
                    self.message = Some("Nothing to undo".to_string());
                    Outcome::None
                }
            }
            Command::Redo => {
                if self.editor.redo(&mut self.module) {
                    self.clamp_position();
                    Outcome::Edited
                } else {
                    self.message = Some("Nothing to redo".to_string());
                    Outcome::None
                }
            }
            Command::OrderPattern(delta) => {
                let pos = self.order_pos;
                if self.editor.bump_order_pattern(&mut self.module, pos, delta) {
                    self.sync_view_to_order();
                    self.edited(before)
                } else {
                    self.message = Some("N makes a new pattern".to_string());
                    Outcome::None
                }
            }
            Command::InsertOrder => {
                if self.editor.insert_order(&mut self.module, self.order_pos) {
                    self.sync_view_to_order();
                    self.edited(before)
                } else {
                    self.message = Some("The order list is full".to_string());
                    Outcome::None
                }
            }
            Command::DeleteOrder => {
                if self.editor.delete_order(&mut self.module, self.order_pos) {
                    self.sync_view_to_order();
                    self.edited(before)
                } else {
                    self.message = Some("The song keeps one position".to_string());
                    Outcome::None
                }
            }
            Command::SongLength(delta) => {
                let next = i32::from(self.module.song_length) + delta;
                let next = u8::try_from(next.clamp(1, 128)).unwrap_or(1);
                if self.editor.set_song_length(&mut self.module, next) {
                    self.clamp_position();
                    self.edited(before)
                } else {
                    self.message = Some("Song length stays in 1..=128".to_string());
                    Outcome::None
                }
            }
            Command::NewPattern => {
                if self.editor.new_pattern(&mut self.module, self.order_pos) {
                    self.sync_view_to_order();
                    self.message = Some(format!("Pattern {:02}", self.view_pattern));
                    Outcome::Edited
                } else {
                    self.message = Some("Already at 256 patterns".to_string());
                    Outcome::None
                }
            }
            Command::BeginTitle => {
                self.overlay = Overlay::Text {
                    target: TextTarget::Title,
                    buffer: editable_text(&self.module.title),
                };
                Outcome::None
            }
            Command::BeginSampleName => {
                let index = self.sample.min(SAMPLE_COUNT - 1);
                self.overlay = Overlay::Text {
                    target: TextTarget::Sample(index),
                    buffer: editable_text(&self.module.samples[index].name),
                };
                Outcome::None
            }
            _ => Outcome::None,
        }
    }

    fn enter_note(&mut self, semitone: u8) -> Outcome {
        let Some(period) = period_at(self.octave, semitone) else {
            return Outcome::None;
        };
        let sample = u8::try_from(self.sample.saturating_add(1)).unwrap_or(1);
        let channel = self.channel;
        let place = self.place();
        let Some(next) = self
            .editor
            .enter_note(&mut self.module, place, period, sample)
        else {
            return Outcome::None;
        };
        self.take_cursor(next);
        Outcome::Preview {
            sample,
            period,
            channel,
        }
    }

    fn confirm_text(&mut self) -> Outcome {
        let Overlay::Text { target, buffer } = self.overlay.clone() else {
            return Outcome::None;
        };
        let result = match target {
            TextTarget::Title => self.editor.set_title(&mut self.module, &buffer),
            TextTarget::Sample(index) => {
                self.editor
                    .set_sample_name(&mut self.module, index, &buffer)
            }
        };
        match result {
            Ok(()) => {
                self.overlay = Overlay::None;
                Outcome::Edited
            }
            Err(err) => {
                self.message = Some(err.to_string());
                Outcome::None
            }
        }
    }

    fn push_text(&mut self, ch: char) {
        let Overlay::Text { target, buffer } = &mut self.overlay else {
            return;
        };
        let max = match target {
            TextTarget::Title => TITLE_LEN,
            TextTarget::Sample(_) => SAMPLE_NAME_LEN,
        };
        if buffer.chars().count() >= max || !is_name_char(ch) {
            return;
        }
        buffer.push(ch);
    }

    fn copy_block(&mut self) {
        if self.module.patterns.get(self.view_pattern).is_none() {
            self.message = Some("Pattern is not in the file".to_string());
            return;
        }
        let range = self.active_range();
        self.clipboard = Editor::copy_range(&self.module, self.view_pattern, range);
        self.selection = None;
        self.message = Some(format!("Copied {}x{}", range.rows(), range.channels()));
    }

    fn active_range(&self) -> CellRange {
        if let Some(selection) = self.selection {
            if selection.pattern == self.view_pattern {
                return selection.range();
            }
        }
        CellRange::single(self.row, self.channel)
    }

    fn place(&self) -> Place {
        Place {
            pattern: self.view_pattern,
            cursor: PatternCursor {
                row: self.row,
                channel: self.channel,
                field: self.field,
            },
            step: self.step,
        }
    }

    fn take_cursor(&mut self, cursor: PatternCursor) {
        self.row = cursor.row;
        self.channel = cursor.channel;
        self.field = cursor.field;
        self.nudge_selection();
    }

    fn edited(&self, before: usize) -> Outcome {
        if self.editor.undo_len() != before {
            Outcome::Edited
        } else {
            Outcome::None
        }
    }

    fn nudge_selection(&mut self) {
        if let Some(selection) = &mut self.selection {
            if selection.pattern == self.view_pattern {
                selection.row = self.row;
                selection.channel = self.channel;
            }
        }
    }

    fn retarget_pattern(&mut self, pattern: usize) {
        if pattern != self.view_pattern {
            self.selection = None;
            self.view_pattern = pattern;
        }
    }

    fn sync_view_to_order(&mut self) {
        self.clamp_position();
        let pattern = usize::from(self.module.order[self.order_pos]);
        if pattern < self.module.patterns.len() {
            self.retarget_pattern(pattern);
        }
    }

    fn clamp_position(&mut self) {
        let len = self.song_len();
        if self.order_pos >= len {
            self.order_pos = len.saturating_sub(1);
        }
        if self.module.patterns.is_empty() {
            self.view_pattern = 0;
            return;
        }
        if self.view_pattern >= self.module.patterns.len() {
            self.selection = None;
            self.view_pattern = self.module.patterns.len() - 1;
        }
    }
}

fn is_motion(command: Command) -> bool {
    matches!(
        command,
        Command::MoveRow(_)
            | Command::MoveChannel(_)
            | Command::MoveField(_)
            | Command::FirstRow
            | Command::LastRow
            | Command::NextChannel
            | Command::PrevChannel
            | Command::MoveOrder(_)
            | Command::MovePattern(_)
            | Command::NextFocus
            | Command::PrevFocus
            | Command::MoveSample(_)
            | Command::FirstSample
            | Command::LastSample
            | Command::FirstOrder
            | Command::LastOrder
    )
}

/// Map a key to a command.
pub fn command_for(app: &App, key: Key) -> Option<Command> {
    match app.overlay {
        Overlay::Help => return help_key(key),
        Overlay::Quit => return quit_key(key),
        Overlay::Text { .. } => return text_key(key),
        Overlay::Path(_) => return super::sample::path_command(key),
        Overlay::Import(_) => return super::sample::import_command(key),
        Overlay::Field(_) => return super::sample::field_command(key),
        Overlay::File => return file_menu_key(key),
        Overlay::Guard(_) => return guard_key(key),
        Overlay::None => {}
    }
    if matches!(key, Key::Esc) {
        return Some(esc_command(app));
    }
    let key = fold_case(key);
    if !(app.focus == Focus::Pattern && app.editing) && matches!(key, Key::Char('q')) {
        return Some(Command::QuitAsk);
    }
    if let Some(command) = global_key(key) {
        return Some(command);
    }
    if app.focus == Focus::Pattern {
        if let Some(command) = pattern_chord(key) {
            return Some(command);
        }
    }
    if app.focus == Focus::Pattern && app.editing {
        edit_key(app.field, key)
    } else {
        browse_key(app.focus, key)
    }
}

fn help_key(key: Key) -> Option<Command> {
    match key {
        Key::Esc | Key::Enter | Key::Char('?') | Key::Char('q') | Key::Char('Q') => {
            Some(Command::CloseOverlay)
        }
        _ => None,
    }
}

fn file_menu_key(key: Key) -> Option<Command> {
    match fold_case(key) {
        Key::Esc | Key::Char('q') | Key::Ctrl('f') => Some(Command::CloseOverlay),
        Key::Char('n') => Some(Command::NewModule),
        Key::Char('o') => Some(Command::OpenModule),
        Key::Char('s') => Some(Command::Save),
        Key::Char('a') => Some(Command::SaveAs),
        _ => None,
    }
}

fn guard_key(key: Key) -> Option<Command> {
    match fold_case(key) {
        Key::Char('y') => Some(Command::GuardSave),
        Key::Char('n') => Some(Command::GuardDiscard),
        Key::Esc => Some(Command::CloseOverlay),
        _ => None,
    }
}

fn quit_key(key: Key) -> Option<Command> {
    match fold_case(key) {
        Key::Char('y') => Some(Command::SaveAndQuit),
        Key::Char('n') => Some(Command::QuitDiscard),
        Key::Esc => Some(Command::CloseOverlay),
        _ => None,
    }
}

fn text_key(key: Key) -> Option<Command> {
    match key {
        Key::Enter => Some(Command::TextConfirm),
        Key::Esc => Some(Command::TextCancel),
        Key::Backspace => Some(Command::TextBackspace),
        Key::Char(ch) if !ch.is_control() => Some(Command::TextPush(ch)),
        _ => None,
    }
}

fn esc_command(app: &App) -> Command {
    if app.selection.is_some() {
        Command::ClearBlock
    } else if app.editing {
        Command::ToggleEdit
    } else {
        Command::QuitAsk
    }
}

fn global_key(key: Key) -> Option<Command> {
    match key {
        Key::Ctrl('s') => Some(Command::Save),
        Key::Ctrl('f') => Some(Command::ShowFile),
        Key::Ctrl('g') => Some(Command::BeginExportSong),
        Key::Ctrl('z') => Some(Command::Undo),
        Key::Ctrl('y') => Some(Command::Redo),
        Key::Ctrl('q') => Some(Command::QuitAsk),
        Key::Ctrl('t') => Some(Command::BeginTitle),
        Key::Ctrl('n') => Some(Command::NewPattern),
        Key::Ctrl('c') => Some(Command::Copy),
        Key::Ctrl('x') => Some(Command::Cut),
        Key::Ctrl('v') => Some(Command::Paste),
        Key::Ctrl('b') => Some(Command::ToggleBlock),
        Key::Ctrl('a') => Some(Command::SelectAll),
        Key::Char(' ') => Some(Command::TogglePlay),
        Key::Enter => Some(Command::ToggleEdit),
        Key::Char('?') => Some(Command::ShowHelp),
        Key::F(1) => Some(Command::Octave(-1)),
        Key::F(2) => Some(Command::Octave(1)),
        Key::F(3) => Some(Command::Step(-1)),
        Key::F(4) => Some(Command::Step(1)),
        Key::Alt('1') => Some(Command::ToggleMute(0)),
        Key::Alt('2') => Some(Command::ToggleMute(1)),
        Key::Alt('3') => Some(Command::ToggleMute(2)),
        Key::Alt('4') => Some(Command::ToggleMute(3)),
        _ => None,
    }
}

fn pattern_chord(key: Key) -> Option<Command> {
    match key {
        Key::AltUp => Some(Command::Transpose(1)),
        Key::AltDown => Some(Command::Transpose(-1)),
        Key::AltRight => Some(Command::Transpose(12)),
        Key::AltLeft => Some(Command::Transpose(-12)),
        Key::Alt('k') => Some(Command::ClearChannel),
        Key::Alt('p') => Some(Command::ClearPattern),
        _ => None,
    }
}

fn edit_key(field: Field, key: Key) -> Option<Command> {
    match key {
        Key::Left => Some(Command::MoveField(-1)),
        Key::Right => Some(Command::MoveField(1)),
        Key::Up => Some(Command::MoveRow(-1)),
        Key::Down => Some(Command::MoveRow(1)),
        Key::Tab => Some(Command::NextChannel),
        Key::BackTab => Some(Command::PrevChannel),
        Key::PageUp => Some(Command::MoveRow(-PATTERN_PAGE)),
        Key::PageDown => Some(Command::MoveRow(PATTERN_PAGE)),
        Key::Home => Some(Command::FirstRow),
        Key::End => Some(Command::LastRow),
        Key::Char('[') => Some(Command::MoveOrder(-1)),
        Key::Char(']') => Some(Command::MoveOrder(1)),
        Key::Char(',') => Some(Command::MovePattern(-1)),
        Key::Char('.') => Some(Command::MovePattern(1)),
        Key::Delete => Some(Command::ClearUnderCursor),
        Key::Backspace => Some(Command::BackspaceLine),
        Key::Insert => Some(Command::InsertChannelRow),
        Key::CtrlBackspace => Some(Command::DeleteChannelRow),
        Key::CtrlInsert => Some(Command::InsertPatternRow),
        Key::CtrlDelete => Some(Command::DeletePatternRow),
        Key::Char(ch) => note_or_digit(field, ch),
        _ => None,
    }
}

fn note_or_digit(field: Field, ch: char) -> Option<Command> {
    match field {
        Field::Note => semitone_from_key(ch).map(Command::EnterNote),
        Field::SampleHigh | Field::SampleLow => ch
            .to_digit(10)
            .map(|digit| Command::EnterDigit(u8::try_from(digit).unwrap_or(0))),
        Field::Effect | Field::ParamHigh | Field::ParamLow => ch
            .to_digit(16)
            .map(|digit| Command::EnterDigit(u8::try_from(digit).unwrap_or(0))),
    }
}

fn browse_key(focus: Focus, key: Key) -> Option<Command> {
    if let Some(command) = shared_browse(key) {
        return Some(command);
    }
    match focus {
        Focus::Pattern => pattern_browse(key),
        Focus::Samples => sample_browse(key),
        Focus::Order => order_browse(key),
    }
}

fn shared_browse(key: Key) -> Option<Command> {
    match key {
        Key::Tab => Some(Command::NextFocus),
        Key::BackTab => Some(Command::PrevFocus),
        Key::Char('[') => Some(Command::MoveOrder(-1)),
        Key::Char(']') => Some(Command::MoveOrder(1)),
        Key::Char(',') => Some(Command::MovePattern(-1)),
        Key::Char('.') => Some(Command::MovePattern(1)),
        Key::Char('1') => Some(Command::ToggleMute(0)),
        Key::Char('2') => Some(Command::ToggleMute(1)),
        Key::Char('3') => Some(Command::ToggleMute(2)),
        Key::Char('4') => Some(Command::ToggleMute(3)),
        _ => None,
    }
}

fn pattern_browse(key: Key) -> Option<Command> {
    match key {
        Key::Up | Key::Char('k') => Some(Command::MoveRow(-1)),
        Key::Down | Key::Char('j') => Some(Command::MoveRow(1)),
        Key::Left | Key::Char('h') => Some(Command::MoveChannel(-1)),
        Key::Right | Key::Char('l') => Some(Command::MoveChannel(1)),
        Key::PageUp => Some(Command::MoveRow(-PATTERN_PAGE)),
        Key::PageDown => Some(Command::MoveRow(PATTERN_PAGE)),
        Key::Home => Some(Command::FirstRow),
        Key::End => Some(Command::LastRow),
        _ => None,
    }
}

fn sample_browse(key: Key) -> Option<Command> {
    match key {
        Key::Up | Key::Char('k') => Some(Command::MoveSample(-1)),
        Key::Down | Key::Char('j') => Some(Command::MoveSample(1)),
        Key::PageUp => Some(Command::MoveSample(-SAMPLE_PAGE)),
        Key::PageDown => Some(Command::MoveSample(SAMPLE_PAGE)),
        Key::Home => Some(Command::FirstSample),
        Key::End => Some(Command::LastSample),
        Key::Char('r') => Some(Command::BeginSampleName),
        Key::Char('i') => Some(Command::BeginImport),
        Key::Char('o') => Some(Command::BeginExportSample),
        Key::Char('v') => Some(Command::BeginVolume),
        Key::Char('f') => Some(Command::BeginFinetune),
        Key::Char('l') => Some(Command::BeginLoop),
        Key::Char('/') => Some(Command::ToggleSampleLoop),
        Key::Char('t') => Some(Command::BeginTrim),
        Key::Char('n') => Some(Command::NormalizeSample),
        Key::Char('w') => Some(Command::ReverseSample),
        Key::Char('a') => Some(Command::FadeIn),
        Key::Char('z') => Some(Command::FadeOut),
        Key::Char('c') => Some(Command::ClearSampleData),
        Key::Char('y') => Some(Command::BeginCopySample),
        Key::Char('p') => Some(Command::AuditionSample),
        Key::Char('u') => Some(Command::Undo),
        Key::Char('-') | Key::Char('_') => Some(Command::PreviewNote(-1)),
        Key::Char('=') | Key::Char('+') => Some(Command::PreviewNote(1)),
        _ => None,
    }
}

fn order_browse(key: Key) -> Option<Command> {
    match key {
        Key::Left | Key::Char('h') => Some(Command::MoveOrder(-1)),
        Key::Right | Key::Char('l') => Some(Command::MoveOrder(1)),
        Key::Up | Key::Char('k') => Some(Command::OrderPattern(1)),
        Key::Down | Key::Char('j') => Some(Command::OrderPattern(-1)),
        Key::Home => Some(Command::FirstOrder),
        Key::End => Some(Command::LastOrder),
        Key::PageUp => Some(Command::MoveOrder(-8)),
        Key::PageDown => Some(Command::MoveOrder(8)),
        Key::Insert => Some(Command::InsertOrder),
        Key::Delete => Some(Command::DeleteOrder),
        Key::Char('+') | Key::Char('=') => Some(Command::SongLength(1)),
        Key::Char('-') => Some(Command::SongLength(-1)),
        Key::Char('n') => Some(Command::NewPattern),
        _ => None,
    }
}

fn fold_case(key: Key) -> Key {
    match key {
        Key::Char(ch) => Key::Char(ch.to_ascii_lowercase()),
        Key::Alt(ch) => Key::Alt(ch.to_ascii_lowercase()),
        Key::Ctrl(ch) => Key::Ctrl(ch.to_ascii_lowercase()),
        other => other,
    }
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

fn window_start(offset: usize, cursor: usize, len: usize, window: usize) -> usize {
    if window == 0 {
        return 0;
    }
    let max_offset = len.saturating_sub(window);
    let mut offset = offset.min(max_offset);
    if cursor < offset {
        offset = cursor;
    } else if cursor >= offset.saturating_add(window) {
        offset = cursor + 1 - window;
    }
    offset.min(max_offset)
}

fn cycle(focus: Focus, forward: bool) -> Focus {
    const PANES: [Focus; 3] = [Focus::Pattern, Focus::Samples, Focus::Order];
    let index = PANES.iter().position(|pane| *pane == focus).unwrap_or(0);
    let len = isize::try_from(PANES.len()).unwrap_or(1);
    let delta = if forward { 1 } else { -1 };
    let next = (isize::try_from(index).unwrap_or(0) + delta).rem_euclid(len);
    let next = usize::try_from(next).unwrap_or(0);
    PANES[next.min(PANES.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::Tag;

    fn app_with_patterns(count: usize) -> App {
        let mut module = Module::new(Tag::Mk);
        module.song_length = u8::try_from(count.clamp(1, 128)).unwrap_or(1);
        for (index, slot) in module.order.iter_mut().enumerate().take(count) {
            *slot = u8::try_from(index).unwrap_or(0);
        }
        module.resize_patterns();
        App::new(module)
    }

    #[test]
    fn movement_clamps_and_order_follows_the_pattern() {
        let mut app = app_with_patterns(3);
        app.apply(Command::MoveRow(-1));
        assert_eq!(app.row, 0);
        app.apply(Command::MoveRow(16));
        assert_eq!(app.row, 16);
        app.apply(Command::LastRow);
        assert_eq!(app.row, 63);
        app.apply(Command::MoveRow(1));
        assert_eq!(app.row, 63);
        app.apply(Command::FirstRow);
        assert_eq!(app.row, 0);

        app.apply(Command::MoveChannel(-1));
        assert_eq!(app.channel, 0);
        app.apply(Command::MoveChannel(10));
        assert_eq!(app.channel, 3);

        app.apply(Command::MoveOrder(-1));
        assert_eq!(app.order_pos, 0);
        assert_eq!(app.view_pattern, 0);
        app.apply(Command::MoveOrder(1));
        assert_eq!(app.order_pos, 1);
        assert_eq!(app.view_pattern, 1);
        app.apply(Command::MovePattern(1));
        assert_eq!(app.order_pos, 1);
        assert_eq!(app.view_pattern, 2);
        app.apply(Command::MovePattern(5));
        assert_eq!(app.view_pattern, 2);
        app.apply(Command::MovePattern(-1));
        assert_eq!(app.view_pattern, 1);
    }

    #[test]
    fn sample_focus_moves_samples_not_rows() {
        let mut app = app_with_patterns(1);
        app.apply(command_for(&app, Key::Tab).unwrap());
        assert_eq!(app.focus, Focus::Samples);
        app.apply(command_for(&app, Key::Down).unwrap());
        app.apply(command_for(&app, Key::Char('j')).unwrap());
        assert_eq!(app.sample, 2);
        assert_eq!(app.row, 0);
        app.apply(Command::LastSample);
        assert_eq!(app.sample, 30);
        app.apply(Command::MoveSample(1));
        assert_eq!(app.sample, 30);
        app.apply(command_for(&app, Key::BackTab).unwrap());
        assert_eq!(app.focus, Focus::Pattern);
        app.apply(command_for(&app, Key::Tab).unwrap());
        app.apply(command_for(&app, Key::Tab).unwrap());
        assert_eq!(app.focus, Focus::Order);
    }

    #[test]
    fn keys_quit_and_ignore_unknown_until_edit_mode() {
        let app = app_with_patterns(1);
        assert_eq!(command_for(&app, Key::Char('Q')), Some(Command::QuitAsk));
        assert_eq!(command_for(&app, Key::Esc), Some(Command::QuitAsk));
        assert_eq!(command_for(&app, Key::Ctrl('q')), Some(Command::QuitAsk));
        assert_eq!(command_for(&app, Key::Ctrl('c')), Some(Command::Copy));
        assert_eq!(command_for(&app, Key::Char('x')), None);
        assert_eq!(command_for(&app, Key::Char(' ')), Some(Command::TogglePlay));
        assert_eq!(
            command_for(&app, Key::Char('3')),
            Some(Command::ToggleMute(2))
        );
        let mut playing = app;
        playing.apply(Command::TogglePlay);
        assert!(playing.playing);
        playing.apply(Command::ToggleMute(0));
        assert!(playing.muted[0]);
        playing.apply(Command::TogglePlay);
        assert!(!playing.playing);
        assert_eq!(
            command_for(&playing, Key::Char(']')),
            Some(Command::MoveOrder(1))
        );
        assert_eq!(
            command_for(&playing, Key::Char('.')),
            Some(Command::MovePattern(1))
        );
        assert_eq!(
            command_for(&playing, Key::Char('l')),
            Some(Command::MoveChannel(1))
        );
        playing.apply(Command::NextFocus);
        assert_eq!(command_for(&playing, Key::Right), None);
        playing.apply(Command::QuitAsk);
        assert!(playing.should_quit());
    }

    #[test]
    fn edit_mode_enters_notes_digits_and_asks_before_quit() {
        let mut app = app_with_patterns(1);
        app.module.samples[0].volume = 64;
        app.apply(command_for(&app, Key::Enter).unwrap());
        assert!(app.editing);
        assert_eq!(app.focus, Focus::Pattern);
        assert_eq!(app.octave, 2);
        assert_eq!(app.step, 1);

        let note = command_for(&app, Key::Char('z')).unwrap();
        assert_eq!(note, Command::EnterNote(0));
        assert!(matches!(
            app.apply(note),
            Outcome::Preview {
                sample: 1,
                period: 428,
                channel: 0
            }
        ));
        assert_eq!(app.module.patterns[0].rows[0][0].period, 428);
        assert_eq!(app.module.patterns[0].rows[0][0].sample, 1);
        assert_eq!(app.row, 1);
        assert!(app.is_dirty());

        app.apply(command_for(&app, Key::Char('q')).unwrap());
        assert_eq!(app.module.patterns[0].rows[1][0].period, 214);
        assert_eq!(app.row, 2);
        assert!(!app.should_quit());

        app.apply(Command::Undo);
        app.apply(Command::Undo);
        assert!(!app.is_dirty());
        assert_eq!(app.module.patterns[0].rows[0][0], Cell::empty());

        app.row = 0;
        app.apply(Command::MoveField(1));
        app.apply(Command::EnterDigit(1));
        app.apply(Command::EnterDigit(2));
        assert_eq!(app.field, Field::Effect);
        assert_eq!(app.module.patterns[0].rows[0][0].sample, 12);
        app.apply(Command::EnterDigit(0x0C));
        app.apply(Command::EnterDigit(0x04));
        app.apply(Command::EnterDigit(0x00));
        assert_eq!(app.module.patterns[0].rows[0][0].effect, 0x0C);
        assert_eq!(app.module.patterns[0].rows[0][0].param, 0x40);
        assert_eq!(app.field, Field::Note);
        assert_eq!(app.row, 1);

        app.apply(Command::QuitAsk);
        assert!(!app.should_quit());
        assert!(matches!(app.overlay, Overlay::Quit));
        app.apply(command_for(&app, Key::Esc).unwrap());
        assert!(matches!(app.overlay, Overlay::None));
        app.apply(Command::QuitAsk);
        app.apply(command_for(&app, Key::Char('n')).unwrap());
        assert!(app.should_quit());
    }

    #[test]
    fn block_copy_paste_and_order_edits_mark_the_song_dirty() {
        let mut app = app_with_patterns(1);
        app.apply(Command::ToggleEdit);
        app.apply(Command::EnterNote(0));
        app.row = 0;
        app.apply(command_for(&app, Key::Ctrl('b')).unwrap());
        app.apply(Command::MoveRow(1));
        app.apply(Command::MoveChannel(1));
        assert!(app.cell_selected(0, 0));
        assert!(app.cell_selected(1, 1));
        app.apply(Command::Copy);
        assert!(app.selection.is_none());
        app.row = 4;
        app.channel = 2;
        assert!(matches!(app.apply(Command::Paste), Outcome::Edited));
        assert_eq!(app.module.patterns[0].rows[4][2].period, 428);
        assert_eq!(app.message.as_deref(), Some("Pasted"));

        app.apply(Command::Transpose(1));
        assert_eq!(app.module.patterns[0].rows[4][2].period, 404);

        app.focus = Focus::Order;
        app.editing = false;
        app.apply(command_for(&app, Key::Char('n')).unwrap());
        assert_eq!(app.module.patterns.len(), 2);
        assert_eq!(app.view_pattern, 1);
        app.apply(command_for(&app, Key::Char('+')).unwrap());
        assert_eq!(app.module.song_length, 2);
        app.apply(Command::Undo);
        assert_eq!(app.module.song_length, 1);
    }

    #[test]
    fn save_round_trips_and_clears_the_modified_flag() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("omatrack-app-save-{}.mod", std::process::id()));
        let mut app = App::open(Module::new(Tag::Mk), path.clone());
        app.apply(Command::ToggleEdit);
        app.apply(Command::EnterNote(0));
        app.apply(Command::BeginTitle);
        for ch in "Edit".chars() {
            app.apply(Command::TextPush(ch));
        }
        assert!(matches!(app.apply(Command::TextConfirm), Outcome::Edited));
        assert_eq!(app.module.display_title(), "Edit");
        app.save().unwrap();
        assert!(!app.is_dirty());
        let loaded = Module::load(&path).unwrap();
        assert_eq!(loaded.display_title(), "Edit");
        assert_eq!(loaded.patterns[0].rows[0][0].period, 428);
        assert_eq!(loaded.patterns[0].rows[0][0].sample, 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn file_menu_asks_before_replacing_a_dirty_song_and_save_as_needs_a_path() {
        let mut app = app_with_patterns(1);
        assert_eq!(command_for(&app, Key::Ctrl('f')), Some(Command::ShowFile));
        app.apply(Command::ShowFile);
        assert!(matches!(app.overlay, Overlay::File));
        assert_eq!(command_for(&app, Key::Char('n')), Some(Command::NewModule));
        assert!(matches!(
            app.apply(Command::NewModule),
            Outcome::DocumentReplaced
        ));
        assert!(!app.is_dirty());
        assert!(app.path.as_os_str().is_empty());
        assert_eq!(app.message.as_deref(), Some("New module"));

        app.apply(Command::ToggleEdit);
        app.apply(Command::EnterNote(0));
        assert!(app.is_dirty());
        app.apply(Command::ShowFile);
        app.apply(command_for(&app, Key::Char('O')).unwrap());
        assert!(matches!(app.overlay, Overlay::Guard(Followup::Open)));
        app.apply(command_for(&app, Key::Esc).unwrap());
        assert!(app.is_dirty());
        assert!(matches!(app.overlay, Overlay::None));

        app.apply(Command::NewModule);
        assert!(matches!(app.overlay, Overlay::Guard(Followup::New)));
        assert!(matches!(
            app.apply(Command::GuardDiscard),
            Outcome::DocumentReplaced
        ));
        assert!(!app.is_dirty());

        app.apply(Command::EnterNote(0));
        assert!(matches!(app.apply(Command::Save), Outcome::None));
        assert!(matches!(app.overlay, Overlay::Path(_)));
    }
}
