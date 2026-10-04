//! Braille canvas for the full-screen scope.
//!
//! Each cell is one Unicode braille pattern (U+2800), two dots wide and four
//! tall. Drawing is pure: stereo samples and four channel levels in, bits out.
//! The UI thread paints the cells; nothing here touches the audio device.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, TAU};

/// Lissajous trace.
pub const INK_SCOPE: u8 = 1;
/// Channel shapes. Channel 1 is [`INK_CHANNEL`].
pub const INK_CHANNEL: u8 = 2;
/// Dim guides, drawn under the trace.
pub const INK_GUIDE: u8 = 6;

/// A grid of braille bits plus which element painted each cell last.
pub struct ScopeCanvas {
    /// Cells across.
    pub width: usize,
    /// Cells down.
    pub height: usize,
    /// Dot bits, row-major. Empty when the size was rejected.
    pub bits: Vec<u8>,
    /// Ink id per cell. `0` is untouched.
    pub ink: Vec<u8>,
}

impl ScopeCanvas {
    /// Braille character for cell `(x, y)`, or a space when that cell is blank
    /// or out of range.
    pub fn glyph(&self, x: usize, y: usize) -> char {
        let Some(bits) = self.bits.get(y * self.width + x).copied() else {
            return ' ';
        };
        if bits == 0 {
            ' '
        } else {
            char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ')
        }
    }

    /// Ink at `(x, y)`.
    pub fn ink_at(&self, x: usize, y: usize) -> u8 {
        self.ink.get(y * self.width + x).copied().unwrap_or(0)
    }

    fn plot(&mut self, x: i32, y: i32, ink: u8) {
        if x < 0 || y < 0 || self.width == 0 || self.height == 0 {
            return;
        }
        let x = x as usize;
        let y = y as usize;
        if x >= self.width * 2 || y >= self.height * 4 {
            return;
        }
        let cell_x = x / 2;
        let cell_y = y / 4;
        let index = cell_y * self.width + cell_x;
        // Left column is dots 1, 2, 3, 7. Right column is 4, 5, 6, 8.
        const DOTS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
        self.bits[index] |= DOTS[x % 2][y % 4];
        let previous = self.ink[index];
        if ink != INK_GUIDE || previous == 0 {
            self.ink[index] = ink;
        }
    }

    fn line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, ink: u8) {
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let step_x = if x0 < x1 { 1 } else { -1 };
        let step_y = if y0 < y1 { 1 } else { -1 };
        let mut err = dx - dy;
        loop {
            self.plot(x0, y0, ink);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let doubled = err * 2;
            if doubled > -dy {
                err -= dy;
                x0 += step_x;
            }
            if doubled < dx {
                err += dx;
                y0 += step_y;
            }
        }
    }
}

/// Draw a vectorscope and four channel shapes.
///
/// `stereo` is interleaved and roughly `-1..=1`. `levels` are `0..=1` per
/// channel. `phase` rotates the shapes, in radians. A zero size, or a canvas
/// bigger than 240×120, comes back empty so a bad layout cannot allocate
/// without bound.
pub fn draw_scope(
    width: usize,
    height: usize,
    stereo: &[f32],
    levels: [f32; 4],
    phase: f32,
) -> ScopeCanvas {
    if width == 0 || height == 0 || width > 240 || height > 120 {
        return ScopeCanvas {
            width: 0,
            height: 0,
            bits: Vec::new(),
            ink: Vec::new(),
        };
    }
    let mut canvas = ScopeCanvas {
        width,
        height,
        bits: vec![0; width * height],
        ink: vec![0; width * height],
    };
    let pixels_w = (width * 2) as f32;
    let pixels_h = (height * 4) as f32;
    let cx = pixels_w * 0.5;
    let cy = pixels_h * 0.5;
    let min_dim = pixels_w.min(pixels_h);

    let guide = min_dim * 0.46;
    canvas.line(
        (cx - guide) as i32,
        cy as i32,
        (cx + guide) as i32,
        cy as i32,
        INK_GUIDE,
    );
    canvas.line(
        cx as i32,
        (cy - guide * 0.72) as i32,
        cx as i32,
        (cy + guide * 0.72) as i32,
        INK_GUIDE,
    );

    let average = levels.iter().copied().map(unit).sum::<f32>() / 4.0;
    let pulse = min_dim * (0.10 + 0.12 * average);
    ring(&mut canvas, cx, cy, pulse, INK_GUIDE);

    let orbit = min_dim * 0.30;
    let spin = if phase.is_finite() { phase } else { 0.0 };
    for (index, level) in levels.iter().copied().enumerate() {
        let level = unit(level);
        let angle = spin + index as f32 * FRAC_PI_2;
        let ox = cx + angle.cos() * orbit;
        let oy = cy + angle.sin() * orbit * 0.72;
        let radius = min_dim * (0.045 + 0.12 * level);
        diamond(
            &mut canvas,
            ox,
            oy,
            radius,
            spin * 1.6 + index as f32 * 0.4,
            INK_CHANNEL + index as u8,
        );
    }

    trace(&mut canvas, stereo, cx, cy, min_dim * 0.22);
    canvas
}

fn unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn ring(canvas: &mut ScopeCanvas, cx: f32, cy: f32, radius: f32, ink: u8) {
    let steps = (radius * 1.4).clamp(16.0, 96.0) as i32;
    for step in 0..steps {
        let angle = TAU * step as f32 / steps as f32;
        canvas.plot(
            (cx + angle.cos() * radius) as i32,
            (cy + angle.sin() * radius) as i32,
            ink,
        );
    }
}

fn diamond(canvas: &mut ScopeCanvas, ox: f32, oy: f32, radius: f32, rotation: f32, ink: u8) {
    let mut points = [(0.0f32, 0.0f32); 4];
    for (corner, point) in points.iter_mut().enumerate() {
        let angle = rotation + FRAC_PI_4 + corner as f32 * FRAC_PI_2;
        *point = (ox + angle.cos() * radius, oy + angle.sin() * radius);
    }
    for corner in 0..4 {
        let (x0, y0) = points[corner];
        let (x1, y1) = points[(corner + 1) % 4];
        canvas.line(x0 as i32, y0 as i32, x1 as i32, y1 as i32, ink);
    }
}

fn trace(canvas: &mut ScopeCanvas, stereo: &[f32], cx: f32, cy: f32, scale: f32) {
    let frames = stereo.len() / 2;
    if frames == 0 || scale <= 0.0 {
        return;
    }
    let step = (frames / 240).max(1);
    let mut previous: Option<(f32, f32)> = None;
    let mut index = 0;
    while index < frames {
        let left = finite(stereo[index * 2]);
        let right = finite(stereo[index * 2 + 1]);
        let x = cx + left.clamp(-1.2, 1.2) * scale;
        let y = cy - right.clamp(-1.2, 1.2) * scale;
        if let Some((px, py)) = previous {
            let dx = x - px;
            let dy = y - py;
            if dx * dx + dy * dy < scale * scale {
                canvas.line(px as i32, py as i32, x as i32, y as i32, INK_SCOPE);
            } else {
                canvas.plot(x as i32, y as i32, INK_SCOPE);
            }
        } else {
            canvas.plot(x as i32, y as i32, INK_SCOPE);
        }
        previous = Some((x, y));
        index += step;
    }
}

fn finite(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(canvas: &ScopeCanvas) -> usize {
        canvas.bits.iter().filter(|bits| **bits != 0).count()
    }

    #[test]
    fn silence_at_the_origin_marks_the_center_cell() {
        let canvas = draw_scope(8, 8, &[0.0, 0.0], [0.0; 4], 0.0);
        assert_eq!(canvas.width, 8);
        assert_ne!(canvas.ink_at(4, 4), 0, "center cell should be drawn");
        assert_ne!(canvas.glyph(4, 4), ' ');
        let code = canvas.glyph(4, 4) as u32;
        assert!((0x2800..=0x28FF).contains(&code), "{code:#x}");
    }

    #[test]
    fn louder_channels_paint_more_of_the_canvas() {
        let quiet = draw_scope(24, 16, &[], [0.0; 4], 0.4);
        let loud = draw_scope(24, 16, &[], [1.0; 4], 0.4);
        assert!(
            lit(&loud) > lit(&quiet),
            "{} vs {}",
            lit(&loud),
            lit(&quiet)
        );
    }

    #[test]
    fn phase_moves_the_shapes() {
        let levels = [0.8, 0.4, 0.6, 0.2];
        let a = draw_scope(20, 14, &[], levels, 0.0);
        let b = draw_scope(20, 14, &[], levels, 1.3);
        assert_ne!(a.bits, b.bits);
    }

    #[test]
    fn a_rejected_size_is_empty() {
        let canvas = draw_scope(0, 10, &[0.0, 0.0], [1.0; 4], 0.0);
        assert!(canvas.bits.is_empty());
        assert_eq!(canvas.glyph(0, 0), ' ');
        let canvas = draw_scope(400, 10, &[], [0.0; 4], 0.0);
        assert!(canvas.bits.is_empty());
    }
}
