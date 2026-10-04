//! Loud transient, then silence. Writes a frame strip of the spectrum caps.
//!
//!     cargo run --example peak_fall_frames -- /tmp/peak-fall

use std::env;
use std::fs;
use std::path::PathBuf;

use omatrack::omarchy::palette_from_colors_toml;
use omatrack::player::{Playback, PlayerConfig};
use omatrack::tui::{draw, App, Theme};
use omatrack::viz::{spectrum_column, ColumnInk, VizSnapshot, VizState, WINDOW};
use omatrack::Module;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const COLS: usize = 24;
const ROWS: usize = 4;
const FRAME_DT: f32 = 0.10;
const FRAMES: usize = 30;

const MATTE: &str = r##"
background = "#121212"
foreground = "#bebebe"
accent = "#e68e0d"
color1 = "#D35F5F"
color2 = "#FFC107"
color3 = "#b91c1c"
color4 = "#e68e0d"
color5 = "#D35F5F"
color6 = "#bebebe"
color7 = "#bebebe"
color8 = "#8a8a8d"
color14 = "#eaeaea"
color15 = "#ffffff"
"##;

fn main() {
    let dir = PathBuf::from(env::args().nth(1).unwrap_or_else(|| "peak-fall".into()));
    fs::create_dir_all(&dir).expect("output dir");

    let frames = transient_then_silence();
    write_ppm(&dir.join("peak-fall.ppm"), &frames);
    fs::write(dir.join("peak-fall.txt"), text_of(&frames)).expect("text");

    let module_lines = module_spectrum_lines("tests/data/11thhour_TDK_CCBY.mod");
    fs::write(dir.join("11th-hour.txt"), &module_lines).expect("module text");
    println!("wrote {}", dir.display());
}

/// One loud window, then silence. Caps are sampled from [`VizState`] at the
/// displayed column width, which is the path the panel paints.
fn transient_then_silence() -> Vec<Vec<Vec<(char, Ink)>>> {
    let mut state = VizState::new();
    state.set_column_count(COLS);
    let mut frames = Vec::with_capacity(FRAMES);
    let mut gen = 1u64;
    for frame in 0..FRAMES {
        let snap = if frame < 3 {
            burst_snapshot(gen)
        } else {
            silence_snapshot(gen)
        };
        gen += 1;
        state.tick(Some(&snap), FRAME_DT);
        let mut columns = Vec::with_capacity(COLS);
        for index in 0..COLS {
            let level = state.column_level(index);
            let peak = state.column_peak(index).max(level);
            columns.push(paint(level, peak));
        }
        frames.push(columns);
    }
    frames
}

fn burst_snapshot(gen: u64) -> VizSnapshot {
    let mut stereo = [0i16; WINDOW * 2];
    for index in 0..WINDOW {
        let t = index as f32 / 44_100.0;
        let low = (std::f32::consts::TAU * 180.0 * t).sin();
        let mid = (std::f32::consts::TAU * 900.0 * t).sin();
        let high = (std::f32::consts::TAU * 4_000.0 * t).sin();
        let mixed = low * 0.85 + mid * 0.35 + high * 0.15;
        let value = (mixed.clamp(-1.0, 1.0) * 28_000.0) as i16;
        stereo[index * 2] = value;
        stereo[index * 2 + 1] = value / 2;
    }
    VizSnapshot {
        stereo,
        peaks: [7000, 2000, 5000, 1000],
        rate: 44_100,
        gen,
    }
}

fn silence_snapshot(gen: u64) -> VizSnapshot {
    VizSnapshot {
        stereo: [0; WINDOW * 2],
        peaks: [0; 4],
        rate: 44_100,
        gen,
    }
}

fn paint(level: f32, peak: f32) -> Vec<(char, Ink)> {
    spectrum_column(ROWS, level, peak)
        .into_iter()
        .map(|cell| {
            let ink = match cell.ink {
                ColumnInk::Empty => Ink::Empty,
                ColumnInk::Body => Ink::Body,
                ColumnInk::Peak => Ink::Peak,
            };
            (cell.glyph, ink)
        })
        .collect()
}

fn text_of(frames: &[Vec<Vec<(char, Ink)>>]) -> String {
    let mut out = String::new();
    for (index, columns) in frames.iter().enumerate() {
        let t = index as f32 * FRAME_DT;
        out.push_str(&format!("t={t:.1}s\n"));
        for row in 0..ROWS {
            for column in columns {
                out.push(column[row].0);
            }
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// Play the module and draw the real panel, so the glyphs are the ones the
/// terminal shows. Returns the spectrum rows from a few seconds in.
fn module_spectrum_lines(path: &str) -> String {
    let module = Module::load(path).expect("module");
    let mut playback = Playback::new(PlayerConfig::default());
    playback.start(&module, 0, 0);
    let mut app = App::open(Module::load(path).expect("module"), PathBuf::from(path));
    let palette = palette_from_colors_toml(MATTE).expect("palette");
    app.set_theme(Theme::from_palette(&palette), "matte-black");

    let rate = 44_100u32;
    let dt = 0.02f32;
    let frames = (rate as f32 * dt) as usize;
    let mut pcm = vec![0i16; frames * 2];
    let mut window = Vec::<i16>::new();
    let mut gen = 0u64;
    let mut last = String::new();
    for step in 0..200 {
        let wrote = playback.render(&module, &mut pcm);
        if wrote == 0 {
            break;
        }
        window.extend_from_slice(&pcm[..wrote * 2]);
        let mut snap = None;
        while window.len() >= WINDOW * 2 {
            gen += 1;
            let mut stereo = [0i16; WINDOW * 2];
            stereo.copy_from_slice(&window[..WINDOW * 2]);
            window.drain(..WINDOW * 2);
            snap = Some(VizSnapshot {
                stereo,
                peaks: playback.channel_peaks(),
                rate,
                gen,
            });
        }
        app.tick_viz(snap.as_ref(), dt);
        if step % 25 == 24 {
            last = render_spectrum(&mut app);
        }
    }
    last
}

fn render_spectrum(app: &mut App) -> String {
    let backend = TestBackend::new(100, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let peak = Theme::from_palette(&palette_from_colors_toml(MATTE).unwrap()).spectrum_peak;
    let mut out = String::new();
    let mut started = false;
    for y in 0..40 {
        let mut line = String::new();
        let mut marks = String::new();
        for x in 0..86 {
            let cell = &buf[(x, y)];
            line.push_str(cell.symbol());
            let glyph = cell.symbol().chars().next().unwrap_or(' ');
            if cell.fg == peak && glyph != ' ' {
                marks.push(glyph);
            } else {
                marks.push(' ');
            }
        }
        if line.contains("Spectrum") {
            started = true;
        }
        if started {
            out.push_str(line.trim_end());
            out.push('\n');
            if marks.chars().any(|ch| ch != ' ') {
                out.push_str(marks.trim_end());
                out.push_str("   <- peak glyphs\n");
            }
            if line.contains("VIEW") || line.contains("Pat ") {
                break;
            }
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ink {
    Empty,
    Body,
    Peak,
}

fn write_ppm(path: &std::path::Path, frames: &[Vec<Vec<(char, Ink)>>]) {
    let cell_w = 14u32;
    let cell_h = 22u32;
    let gap = 8u32;
    let label_h = 14u32;
    let width = COLS as u32 * cell_w;
    let frame_h = label_h + ROWS as u32 * cell_h + gap;
    let height = FRAMES as u32 * frame_h;
    let mut rgb = vec![0x12u8; (width * height * 3) as usize];
    for (frame_index, columns) in frames.iter().enumerate() {
        let origin_y = frame_index as u32 * frame_h;
        stamp_label(&mut rgb, width, origin_y, frame_index);
        for (col_index, column) in columns.iter().enumerate() {
            for (row_index, (glyph, ink)) in column.iter().enumerate() {
                paint_cell(
                    &mut rgb,
                    CellPaint {
                        stride: width,
                        x: col_index as u32 * cell_w,
                        y: origin_y + label_h + row_index as u32 * cell_h,
                        width: cell_w,
                        height: cell_h,
                        glyph: *glyph,
                        ink: *ink,
                        row: row_index,
                    },
                );
            }
        }
    }
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    ppm.extend_from_slice(&rgb);
    fs::write(path, ppm).expect("ppm");
}

fn stamp_label(rgb: &mut [u8], stride: u32, y: u32, frame: usize) {
    let tenths = frame;
    let text = format!("{tenths:02}");
    for (index, byte) in text.bytes().enumerate() {
        blit_digit(rgb, stride, 4 + index as u32 * 8, y + 2, byte);
    }
}

fn blit_digit(rgb: &mut [u8], stride: u32, x: u32, y: u32, digit: u8) {
    const FONT: [[u8; 5]; 10] = [
        [0b111, 0b101, 0b101, 0b101, 0b111],
        [0b010, 0b110, 0b010, 0b010, 0b111],
        [0b111, 0b001, 0b111, 0b100, 0b111],
        [0b111, 0b001, 0b111, 0b001, 0b111],
        [0b101, 0b101, 0b111, 0b001, 0b001],
        [0b111, 0b100, 0b111, 0b001, 0b111],
        [0b111, 0b100, 0b111, 0b101, 0b111],
        [0b111, 0b001, 0b001, 0b001, 0b001],
        [0b111, 0b101, 0b111, 0b101, 0b111],
        [0b111, 0b101, 0b111, 0b001, 0b111],
    ];
    let rows = FONT[usize::from(digit - b'0')];
    for (row, bits) in rows.iter().enumerate() {
        for col in 0..3 {
            if bits & (1 << (2 - col)) != 0 {
                for dy in 0..2 {
                    for dx in 0..2 {
                        put(
                            rgb,
                            stride,
                            x + col * 2 + dx,
                            y + row as u32 * 2 + dy,
                            [0xbe, 0xbe, 0xbe],
                        );
                    }
                }
            }
        }
    }
}

struct CellPaint {
    stride: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    glyph: char,
    ink: Ink,
    row: usize,
}

fn paint_cell(rgb: &mut [u8], cell: CellPaint) {
    let bg = [0x12, 0x12, 0x12];
    let fill = match cell.ink {
        Ink::Empty => bg,
        Ink::Body => body_rgb(cell.row),
        Ink::Peak => [0xb9, 0x1c, 0x1c],
    };
    let eighths = match cell.glyph {
        ' ' => 0,
        '▁' => 1,
        '▂' => 2,
        '▃' => 3,
        '▄' => 4,
        '▅' => 5,
        '▆' => 6,
        '▇' => 7,
        '█' => 8,
        '▔' => 1,
        _ => 0,
    };
    let top = cell.glyph == '▔';
    for py in 0..cell.height {
        let from_bottom = cell.height - 1 - py;
        let lit = if top {
            py * 8 < cell.height
        } else {
            from_bottom * 8 < eighths * cell.height
        };
        let color = if lit { fill } else { bg };
        for px in 0..cell.width {
            put(rgb, cell.stride, cell.x + px, cell.y + py, color);
        }
    }
}

fn body_rgb(row_index: usize) -> [u8; 3] {
    let t = (ROWS - 1 - row_index) as f32 / (ROWS - 1) as f32;
    let mix = 0.45 + 0.55 * t;
    [
        lerp(0x12, 0xe6, mix),
        lerp(0x12, 0x8e, mix),
        lerp(0x12, 0x0d, mix),
    ]
}

fn lerp(from: u8, to: u8, t: f32) -> u8 {
    let value = f32::from(from) + (f32::from(to) - f32::from(from)) * t;
    value.round().clamp(0.0, 255.0) as u8
}

fn put(rgb: &mut [u8], stride: u32, x: u32, y: u32, color: [u8; 3]) {
    let index = ((y * stride + x) * 3) as usize;
    if index + 2 < rgb.len() {
        rgb[index..index + 3].copy_from_slice(&color);
    }
}
