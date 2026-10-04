# Omatrack

Omatrack is a ProTracker / Amiga-style music tracker for [Omarchy](https://omarchy.org) Linux (Arch + Hyprland), written in Rust as a terminal UI. It also runs in any terminal that can host a normal Rust binary.

Milestone 1 loads a 4-channel `.mod` file and shows it. You can move through the order list, the pattern, and the sample list. Nothing is played and nothing is edited yet.

## Build and run

Rust 1.83 or newer is required. The committed `Cargo.lock` pins a few transitive crates so that compiler still builds.

```bash
cargo build --release
cargo run -- path/to/song.mod
```

There is no demo song in the repo (module files are gitignored, and copyrighted modules do not belong here). Generate a small original one and open it:

```bash
cargo run --example write_showcase -- /tmp/omatrack-showcase.mod
cargo run -- /tmp/omatrack-showcase.mod
```

`--help` prints the keys. The view wants about 76 columns by 20 rows; 80×24 is comfortable.

```text
q, Esc, Ctrl-C   quit
Tab              switch between the pattern and the sample list
Up/Down, j/k     move the cursor
Left/Right, h/l  change channel (pattern view)
PgUp/PgDn        page
Home/End         first or last row, or sample
[ ]              previous / next order position
, .              previous / next pattern
```

`[ ]` follows the order list and changes the pattern you see. `,` `.` walks patterns directly, including ones the current order position does not point at.

Notes use ProTracker octave numbers (`C-1` is period 856, not `C-4`). Sample numbers in the pattern are decimal `01`–`31`. The loop column is `start+length` in bytes, and `-` means the sample does not loop. Quit with `q`; the file is not modified.

## Checks

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

GitHub Actions runs those three on Rust 1.83.

## What `.mod` files are supported

31-sample, 4-channel ProTracker modules:

| Tag | Where it shows up |
| --- | --- |
| `M.K.` | ProTracker |
| `M!K!` | ProTracker with more than 64 patterns |
| `FLT4` | Startrekker 4-channel |
| `4CHN` | generic 4-channel |

The reader and writer share one layout: 20-byte title, 31 × 30-byte sample headers (name, length in words, finetune, volume, loop start, loop length), song length, restart byte, 128-byte order list, the tag, then 64×4 patterns and signed 8-bit sample data.

Pattern count is `max(order) + 1` over all 128 order bytes, including slots past the song length. That is the ProTracker rule, and it is what makes `parse` then `write` return the same bytes. Names, the restart byte, finetune bits above the low nibble, volumes above 64, and any suffix after the samples are kept. Cells that do not fit the bit fields (period wider than 12 bits, effect wider than 4 bits) are rejected on write rather than silently truncated.

A short file, a bad tag, or a song length outside `1..=128` returns an error. 15-sample modules and 2/6/8-channel tags are not this format and are rejected. Tests synthesize their own modules; do not commit copyrighted `.mod` files.

## Layout

`omatrack` is a library and a binary. The binary is a thin command-line wrapper so the format code can be tested without a terminal.

```text
src/lib.rs        library root
src/main.rs       arguments, exit codes, terminal startup
src/module.rs     Module, Pattern, Cell, Sample
src/modfile.rs    .mod parser and writer
src/notes.rs      finetune-0 period table and effect names
src/error.rs
src/tui/          cursor, keys, drawing, colors
```

`Module` is the document. The TUI borrows it and keeps only view state (which row, which channel, which order position). That leaves room for the rest of v1:

| Milestone | What it adds | Where it should live |
| --- | --- | --- |
| M1 | `.mod` load/save model, read-only tracker view | this tree |
| M2 | 4-channel mixer, Amiga periods, arpeggio, slides, vibrato, volume, speed/tempo, pattern jump/break, PipeWire/ALSA via cpal | a `player` module that borrows `Module` and owns voice state; audio output behind a small backend trait |
| M3 | note entry, copy/paste, undo, ProTracker-style keys | commands that mutate `Module`, with the undo stack next to `App` rather than inside the file format |
| M4 | load samples from WAV and save `.mod` from the UI | produce signed 8-bit `Sample.data` (even length) and call the existing writer |
| M5 | Omarchy theme, Arch `PKGBUILD`, polish | replace `Theme::protracker()`; packaging stays outside the library |

Playback is not in this milestone, and the viewer does not write the file it opened.
