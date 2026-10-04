# Omatrack

Omatrack is a ProTracker / Amiga-style music tracker for [Omarchy](https://omarchy.org) Linux (Arch + Hyprland), written in Rust as a terminal UI. It also runs in any terminal that can host a normal Rust binary.

It loads a 4-channel `.mod`, shows it, and plays it. Space starts playback from the cursor. The pattern highlight follows the song. Nothing is edited yet.

## Build and run

Rust 1.83 or newer is required. The committed `Cargo.lock` pins a few transitive crates so that compiler still builds. On Linux the audio backend is [cpal](https://docs.rs/cpal) through ALSA, which is also how PipeWire and PulseAudio expose an output. Building needs the ALSA headers (`libasound2-dev` on Debian and Ubuntu, `alsa-lib` on Arch).

```bash
cargo build --release
cargo run -- path/to/song.mod
```

Copyrighted modules do not belong in the repo. The tests load freely licensed fixtures from `tests/data/` (CC0, public domain, CC BY 4.0, and BSD-3-Clause; see `tests/data/README.md` and `tests/data/ATTRIBUTION.txt`). Other `*.mod` paths stay gitignored. Generate a small original song and open it:

```bash
cargo run --example write_showcase -- /tmp/omatrack-showcase.mod
cargo run -- /tmp/omatrack-showcase.mod
```

Render that same song to a WAV file without opening a sound device. Playback stops when the song loops back on itself, or at `--max-seconds` (default 10 minutes), whichever comes first:

```bash
cargo run -- --render /tmp/omatrack-showcase.wav /tmp/omatrack-showcase.mod
```

`--rate`, `--interpolate linear|nearest`, and `--separation 0-100` apply to that render. `100` is hard Amiga panning (channels 1 and 4 left, 2 and 3 right). `0` is mono. The default interpolation is linear.

`--help` prints the keys. The view wants about 76 columns by 20 rows; 80×24 is comfortable.

```text
space            play / stop
1 2 3 4          mute that channel
q, Esc, Ctrl-C   quit
Tab              switch between the pattern and the sample list
Up/Down, j/k     move the cursor
Left/Right, h/l  change channel (pattern view)
PgUp/PgDn        page
Home/End         first or last row, or sample
[ ]              previous / next order position
, .              previous / next pattern
```

`[ ]` follows the order list and changes the pattern you see. `,` `.` walks patterns directly, including ones the current order position does not point at. While the song is playing, the view follows the playhead instead: the current row is green, and the transport bar shows order, row, speed, and tempo.

Notes use ProTracker octave numbers (`C-1` is period 856, not `C-4`). Sample numbers in the pattern are decimal `01`–`31`. The loop column is `start+length` in bytes, and `-` means the sample does not loop. Quit with `q`; the file is not modified.

If no output device can be opened, the tracker stays up and the transport bar shows the error. `--render` never touches the device.

## Playback

The mixer lives in `player` and does not open an audio device. `Playback::render` fills an interleaved stereo `i16` buffer from a `Module` and the current voice state, so tests and `--render` hear exactly what the callback plays.

Timing is the PAL CIA clock. Speed is ticks per row (default 6). Tempo is the `Fxx` BPM value (default 125, which is 50 ticks per second). Pitch is `3_546_895 / period` samples per second. A cell stores the finetune-0 period; the channel's finetune (from the sample, or from `E5x` for the next note) selects the ProTracker period table.

Slides, vibrato, arpeggio, tremolo, volume slides, retrigger, and note cut run after the first tick of the row. Notes, sample offset, jumps, breaks, fine slides, and speed/tempo run on the first tick. A zero parameter repeats the last value for slides, portamento, vibrato, tremolo, volume slides, retrigger, offset, and the fine slides.

| Command | What it does |
| --- | --- |
| `0xy` | Arpeggio |
| `1xx` `2xx` | Slide up / down, clamped to periods 113–856 |
| `3xx` | Tone portamento, without retriggering the sample |
| `4xy` | Vibrato |
| `5xy` | Tone portamento and volume slide |
| `6xy` | Vibrato and volume slide |
| `7xy` | Tremolo |
| `9xx` | Sample offset, `xx * 256` bytes |
| `Axy` | Volume slide |
| `Bxx` | Position jump |
| `Cxx` | Set volume, clamped to 64 |
| `Dxx` | Pattern break. The parameter is two decimal digits |
| `E1x` `E2x` | Fine slide up / down |
| `E3x` | Glissando on or off |
| `E4x` `E7x` | Vibrato / tremolo waveform (sine, ramp, square; bit 2 keeps the phase) |
| `E5x` | Set finetune. The note on this row keeps the previous finetune |
| `E6x` | Pattern loop. `E60` sets the start; `E6n` plays the section `n + 1` times |
| `E9x` | Retrigger |
| `EAx` `EBx` | Fine volume up / down |
| `ECx` | Note cut on tick `x` |
| `EDx` | Note delay until tick `x` |
| `EEx` | Pattern delay, `x` extra passes over the row |
| `Fxx` | `00` stops. `01`–`1F` sets speed. `20`–`FF` sets tempo |

These are ignored. The row still advances:

| Command | Why |
| --- | --- |
| `8xx` | Not a ProTracker command. Panning stays the Amiga hard pan |
| `E0x` | Amiga LED filter. There is no analogue filter here |
| `E8x` | Unused in ProTracker |
| `EFx` | Invert loop. It rewrites the sample while it plays |

Channels 1 and 4 are left, 2 and 3 are right. Mute keeps the effect running and only drops that channel's samples.

## Checks

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

GitHub Actions runs those three on Rust 1.83, after installing the ALSA headers cpal needs to build.

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

A short file, a bad tag, or a song length outside `1..=128` returns an error. 15-sample modules and 2/6/8-channel tags are not this format and are rejected. Tests synthesize modules and also round-trip the fixtures in `tests/data/`. Do not commit copyrighted `.mod` files.

## Layout

`omatrack` is a library and a binary. The binary is a thin command-line wrapper so the format code and the mixer can be tested without a terminal or a sound card.

```text
src/lib.rs        library root
src/main.rs       arguments, exit codes, terminal startup
src/module.rs     Module, Pattern, Cell, Sample
src/modfile.rs    .mod parser and writer
src/notes.rs      finetune-0 period table and effect names
src/player/       tick clock, effects, four-channel mixer
src/audio.rs      cpal output
src/wav.rs        16-bit stereo WAV writer
src/demo.rs       the original showcase module
src/error.rs
src/tui/          cursor, keys, drawing, colors
```

`Module` is the document. The TUI borrows it and keeps view state (which row, which channel, which order position). The replayer borrows it too and keeps the voices. That leaves room for the rest of v1:

| Milestone | What it adds | Where it should live |
| --- | --- | --- |
| M1 | `.mod` load/save model, read-only tracker view | this tree |
| M2 | 4-channel mixer, Amiga periods, effects, PipeWire/ALSA via cpal | `player`, `audio`, `wav` |
| M3 | note entry, copy/paste, undo, ProTracker-style keys | commands that mutate `Module`, with the undo stack next to `App` rather than inside the file format |
| M4 | load samples from WAV and save `.mod` from the UI | produce signed 8-bit `Sample.data` (even length) and call the existing writer |
| M5 | Omarchy theme, Arch `PKGBUILD`, polish | replace `Theme::protracker()`; packaging stays outside the library |

The viewer does not write the file it opened.
