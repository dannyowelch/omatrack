//! Render a few consecutive spectrum frames the way the panel used to, and
//! the way it does now.
//!
//!     cargo run --example spectrum_frames -- /tmp/spectrum-frames
//!
//! The "before" painter is the previous column code: the top cell of the bar
//! was recolored as the peak, and a peak that landed in a higher cell was a
//! bottom-anchored partial block. The "after" frames go through
//! [`omatrack::viz::smooth_spectrum`] and [`omatrack::viz::spectrum_column`].

use std::env;
use std::fs;
use std::path::PathBuf;

use omatrack::viz::{smooth_spectrum, spectrum_column, Ballistics, ColumnInk, Meter};

const COLS: usize = 16;
const ROWS: usize = 4;
const FRAMES: usize = 12;
const DT: f32 = 0.10;

fn main() {
    let dir = PathBuf::from(
        env::args()
            .nth(1)
            .unwrap_or_else(|| "spectrum-frames".into()),
    );
    fs::create_dir_all(&dir).expect("output dir");

    let mut old_meters = [OldMeter::default(); COLS];
    let mut new_meters = [Meter::default(); COLS];
    let old_ballistics = old_spectrum();
    let mut before_text = String::new();
    let mut after_text = String::new();
    let mut before_frames = Vec::new();
    let mut after_frames = Vec::new();

    for frame in 0..FRAMES {
        let inputs = frame_inputs(frame);
        for (meter, input) in old_meters.iter_mut().zip(&inputs) {
            meter.update(*input, DT, old_ballistics);
        }
        smooth_spectrum(&mut new_meters, &inputs, DT);

        let old_cols: Vec<Vec<(char, Ink)>> = old_meters
            .iter()
            .map(|meter| old_column(ROWS, meter.level, meter.peak))
            .collect();
        let new_cols: Vec<Vec<(char, Ink)>> = new_meters
            .iter()
            .map(|meter| {
                spectrum_column(ROWS, meter.level, meter.peak)
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
            })
            .collect();

        push_text(&mut before_text, frame, &old_cols);
        push_text(&mut after_text, frame, &new_cols);
        before_frames.push(old_cols);
        after_frames.push(new_cols);
    }

    fs::write(dir.join("before.txt"), &before_text).expect("before.txt");
    fs::write(dir.join("after.txt"), &after_text).expect("after.txt");
    write_ppm(&dir.join("before.ppm"), &before_frames);
    write_ppm(&dir.join("after.ppm"), &after_frames);
    println!("wrote {}", dir.display());
}

fn frame_inputs(frame: usize) -> Vec<f32> {
    (0..COLS)
        .map(|index| {
            let x = index as f32;
            let hump = (-(x - 4.5).powi(2) / 10.0).exp() * 0.78;
            let air = (-(x - 12.0).powi(2) / 18.0).exp() * 0.28;
            let wobble = ((frame * 5 + index * 3) % 9) as f32 / 9.0;
            let noise = (wobble - 0.5) * 0.16;
            let hit = if frame == 2 && (7..11).contains(&index) {
                0.92
            } else {
                0.0
            };
            (hump + air + noise + hit).clamp(0.0, 1.0)
        })
        .collect()
}

fn push_text(out: &mut String, frame: usize, columns: &[Vec<(char, Ink)>]) {
    out.push_str(&format!("frame {frame}\n"));
    for row in 0..ROWS {
        for column in columns {
            let (glyph, _) = column[row];
            out.push(glyph);
        }
        out.push('\n');
    }
    out.push('\n');
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ink {
    Empty,
    Body,
    Peak,
}

const VBLOCK: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The spectrum constants from before this change.
fn old_spectrum() -> Ballistics {
    Ballistics {
        attack: 0.025,
        decay: 0.16,
        hold: 0.22,
        peak_decay: 0.40,
        linear_peak: false,
    }
}

#[derive(Clone, Copy)]
struct OldMeter {
    level: f32,
    peak: f32,
    hold_left: f32,
}

impl Default for OldMeter {
    fn default() -> Self {
        Self {
            level: 0.0,
            peak: 0.0,
            hold_left: 0.0,
        }
    }
}

impl OldMeter {
    /// Previous ballistics: exponential peak fall, and the peak was allowed
    /// to chase the input for the whole timestep once the hold hit zero.
    fn update(&mut self, input: f32, dt: f32, ballistics: Ballistics) {
        let input = input.clamp(0.0, 1.0);
        let tau = if input > self.level {
            ballistics.attack
        } else {
            ballistics.decay
        };
        self.level = approach(self.level, input, dt, tau);
        if input >= self.peak {
            self.peak = input;
            self.hold_left = ballistics.hold;
        } else {
            self.hold_left = (self.hold_left - dt).max(0.0);
            if self.hold_left == 0.0 {
                self.peak = approach(self.peak, input, dt, ballistics.peak_decay);
            }
        }
    }
}

fn approach(current: f32, target: f32, dt: f32, tau: f32) -> f32 {
    if tau <= 1.0e-4 {
        return target;
    }
    let coef = 1.0 - (-dt / tau).exp();
    current + (target - current) * coef
}

/// Previous column painter. The top body cell is the peak color, and a peak
/// above the bar is a partial block grown from the bottom of its cell.
fn old_column(rows: usize, level: f32, peak: f32) -> Vec<(char, Ink)> {
    let total = rows * 8;
    let steps = quantize(level, total);
    let peak_steps = quantize(peak.max(level), total).max(steps);
    let mut cells = vec![(' ', Ink::Empty); rows];
    for (row, cell) in cells.iter_mut().enumerate() {
        let from_bottom = rows - 1 - row;
        let start = from_bottom * 8;
        let filled = steps.saturating_sub(start).min(8);
        let floating = peak_steps > steps && peak_steps > start && peak_steps <= start + 8;
        *cell = if filled == 0 && floating {
            let mark = (peak_steps - start).clamp(1, 8);
            (VBLOCK[mark], Ink::Peak)
        } else if filled == 0 {
            (' ', Ink::Empty)
        } else if from_bottom == steps.saturating_sub(1) / 8 {
            (VBLOCK[filled], Ink::Peak)
        } else {
            (VBLOCK[filled], Ink::Body)
        };
    }
    cells
}

fn quantize(level: f32, total: usize) -> usize {
    let steps = (level.clamp(0.0, 1.0) * total as f32).round() as usize;
    steps.min(total)
}

fn write_ppm(path: &std::path::Path, frames: &[Vec<Vec<(char, Ink)>>]) {
    let cell_w = 16u32;
    let cell_h = 24u32;
    let gap = 10u32;
    let width = COLS as u32 * cell_w;
    let height = FRAMES as u32 * (ROWS as u32 * cell_h + gap);
    let mut rgb = vec![0u8; (width * height * 3) as usize];
    for (frame_index, columns) in frames.iter().enumerate() {
        let origin_y = frame_index as u32 * (ROWS as u32 * cell_h + gap);
        for (col_index, column) in columns.iter().enumerate() {
            for (row_index, (glyph, ink)) in column.iter().enumerate() {
                paint_cell(
                    &mut rgb,
                    CellPaint {
                        stride: width,
                        x: col_index as u32 * cell_w,
                        y: origin_y + row_index as u32 * cell_h,
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
    let eighths = block_eighths(cell.glyph);
    let top_eighth = cell.glyph == '▔';
    for y in 0..cell.height {
        let from_bottom = cell.height - 1 - y;
        let from_top = y;
        let lit = if top_eighth {
            from_top * 8 < cell.height
        } else {
            from_bottom * 8 < eighths * cell.height
        };
        let color = if lit { fill } else { bg };
        for x in 0..cell.width {
            let index = (((cell.y + y) * cell.stride + (cell.x + x)) * 3) as usize;
            rgb[index..index + 3].copy_from_slice(&color);
        }
    }
}

fn body_rgb(row_index: usize) -> [u8; 3] {
    // Top row is Matte Black's spectrum orange. Lower rows ease toward the
    // background, matching the panel gradient.
    let t = if ROWS <= 1 {
        1.0
    } else {
        (ROWS - 1 - row_index) as f32 / (ROWS - 1) as f32
    };
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

fn block_eighths(glyph: char) -> u32 {
    match glyph {
        ' ' => 0,
        '▁' => 1,
        '▂' => 2,
        '▃' => 3,
        '▄' => 4,
        '▅' => 5,
        '▆' => 6,
        '▇' => 7,
        '█' => 8,
        '▀' => 4,
        '▔' => 1,
        _ => 0,
    }
}
