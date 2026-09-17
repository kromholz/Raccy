#[cfg(test)]
pub mod golden;
#[cfg(test)]
pub mod icon;
pub mod sprite;

use fontdue::{Font, FontSettings, Metrics};
use std::collections::HashMap;

use crate::tuning::tuning;
use crate::pet::Activity;
use sprite::{NEON_CYAN, NEON_RED, Pose, Px, Rgb};

// In sprite pixels: the window is W x H of them.
pub const W: usize = 64;
pub const H: usize = 60;
pub const SPRITE_X: usize = W - sprite::SIZE - 1;
pub const SPRITE_Y: usize = H - sprite::SIZE;
const MOUTH: (f32, f32) = (15.5, 17.5);
const ANTENNA: (f32, f32) = (13.5, 1.0);

const TEXT: Rgb = [0xe8, 0xf7, 0xff];
const PANEL: Rgb = [0x0d, 0x0a, 0x14];
const ALERT: Rgb = [0xff, 0xb0, 0x20];
const DIM: Rgb = [0x8a, 0x82, 0xa0];
// QR readers need dark modules on a light background.
const QR_LIGHT: Rgb = [0xf4, 0xf4, 0xf0];
const DECODE_FRAMES: u32 = 5;
// BIT_FLIGHT_FRAMES is counted back from the bite the bits fly into.
const BIT_FLIGHT_FRAMES: u64 = 12;
const BURST_FRAMES: u64 = 3;
const NOISE: &[char] = &['Ж', 'З', 'И', 'Л', 'Ф', 'Ц', 'Ч', 'Ш', 'Щ', 'Ы', 'Э', 'Ю', 'Я', 'Б', 'Г', 'Д', 'П'];

// In sprite pixels, like every other size on these canvases.
fn text_px(scale: usize) -> f32 {
    3.25 * scale as f32
}

// xorshift64. The golden sheets are compared byte for byte, so a seed has to keep
// giving the sequence it gives now.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    fn step(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn unit(&mut self) -> f32 {
        (self.step() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn unit64(&mut self) -> f64 {
        (self.step() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn noise_glyph(rng: &mut Rng) -> char {
    NOISE[(rng.unit() * NOISE.len() as f32) as usize % NOISE.len()]
}

// A line part-way through its decode: one number off the generator for every
// character that is not a space, and a second for each one that comes out scrambled.
fn decoding(line: &str, age: u32, rng: &mut Rng) -> String {
    let noise = match age < DECODE_FRAMES {
        true => (DECODE_FRAMES - age) as f32 / (DECODE_FRAMES + 1) as f32,
        false => 0.0,
    };
    line.chars().map(|ch| if ch != ' ' && rng.unit() < noise { noise_glyph(rng) } else { ch }).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Item { id: usize, label: String, checked: bool, grayed: bool },
    Submenu { label: String, items: Vec<MenuItem> },
    Separator,
}

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    // Premultiplied 0xAARRGGBB.
    pub pixels: Vec<u32>,
}

pub fn clip_rows(src: &Canvas, rows: usize, dst: &mut Canvas) {
    dst.width = src.width;
    dst.height = src.height;
    dst.pixels.clear();
    dst.pixels.extend_from_slice(&src.pixels[..rows.min(src.height) * src.width]);
    dst.pixels.resize(src.width * src.height, 0);
}

pub const PIPE_ROWS: u32 = 12;

// `x` and `base` are relative to the canvas; `shown` is in screen pixels, so the
// pipe can slide by the pixel.
pub fn pipe(canvas: &mut Canvas, x: i32, base: i32, shown: i32, scale: usize) {
    const OUTLINE: Rgb = [0x0b, 0x08, 0x12];
    const MOUTH: Rgb = [0x05, 0x03, 0x08];
    const HIGHLIGHT: Rgb = [0xb4, 0xae, 0xc8];
    const LIGHT: Rgb = [0x8a, 0x84, 0xa0];
    const METAL: Rgb = [0x5a, 0x56, 0x70];
    const SHADE: Rgb = [0x3e, 0x3a, 0x50];
    const DARK: Rgb = [0x26, 0x22, 0x32];
    const RUST: Rgb = [0x8a, 0x4a, 0x2a];
    const RUST_DARK: Rgb = [0x5a, 0x30, 0x1e];
    // Across a round surface lit from the left: a highlight, then fading into shade.
    let round = |col: i32| match col {
        ..=1 | 30.. => DARK,
        2 | 3 => METAL,
        4 => LIGHT,
        5 | 6 => HIGHLIGHT,
        7..=10 => LIGHT,
        11..=19 => METAL,
        20..=25 => SHADE,
        _ => DARK,
    };
    let s = scale as i32;
    let left = x + SPRITE_X as i32 * s;
    let top = base - shown.clamp(0, PIPE_ROWS as i32 * s);
    let mut cell = |col: i32, row: i32, c: Rgb, alpha: u8| {
        let y = top + row * s;
        canvas.rect(left + col * s, y, s, s.min(base - y), c, alpha);
    };
    for row in (0..PIPE_ROWS as i32).take_while(|row| top + row * s < base) {
        for col in 0..sprite::SIZE as i32 {
            let c = match (row, col) {
                (0, 3..=28) => OUTLINE,
                (1, 0 | 31) => OUTLINE,
                (1, 1..=4) | (1, 27..=30) => round(col),
                (1, _) => MOUTH,
                (2, 0 | 31) => OUTLINE,
                (2, _) => round(col),
                (3, 1..=30) | (_, 1 | 30) => OUTLINE,
                (6, 2..=29) if (col - 2) % 6 == 3 => HIGHLIGHT,
                (6, 2..=29) => SHADE,
                (7.., 9 | 21) => RUST,
                (8.., 10) | (9.., 22) => RUST_DARK,
                (_, 2..=29) => round(col),
                _ => continue,
            };
            cell(col, row, c, 255);
        }
        if row == 1 {
            for col in 8..24 {
                cell(col, row, NEON_CYAN, 60);
            }
        }
    }
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Canvas {
        Canvas { width, height, pixels: vec![0; width * height] }
    }

    fn clear(&mut self) {
        self.pixels.fill(0);
    }

    fn blend(&mut self, x: i32, y: i32, c: Rgb, alpha: u8) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height || alpha == 0 {
            return;
        }
        let i = y as usize * self.width + x as usize;
        let a = alpha as u32;
        let inv = 255 - a;
        let d = self.pixels[i];
        let mix = |src: u8, shift: u32| (src as u32 * a + ((d >> shift) & 0xff) * inv) / 255;
        let out_a = a + ((d >> 24) & 0xff) * inv / 255;
        self.pixels[i] = (out_a << 24) | (mix(c[0], 16) << 16) | (mix(c[1], 8) << 8) | mix(c[2], 0);
    }

    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb, alpha: u8) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.blend(xx, yy, c, alpha);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn frame_rect(&mut self, x: i32, y: i32, w: i32, h: i32, t: i32, c: Rgb, alpha: u8) {
        self.rect(x, y, w, t, c, alpha);
        self.rect(x, y + h - t, w, t, c, alpha);
        self.rect(x, y + t, t, h - 2 * t, c, alpha);
        self.rect(x + w - t, y + t, t, h - 2 * t, c, alpha);
    }

    // The fill, the red frame offset by one edge, then the cyan frame over both.
    fn chrome(&mut self, x: i32, y: i32, w: i32, h: i32, edge: i32, fill: u8) {
        self.rect(x, y, w, h, PANEL, fill);
        self.frame_rect(x + edge, y + edge, w - edge, h - edge, edge, NEON_RED, 150);
        self.frame_rect(x, y, w - edge, h - edge, edge, NEON_CYAN, 255);
    }

    fn scanlines(&mut self) {
        for y in (1..self.height).step_by(2) {
            for p in &mut self.pixels[y * self.width..(y + 1) * self.width] {
                if *p >> 24 == 0 {
                    continue;
                }
                let dim = |shift: u32| (((*p >> shift) & 0xff) * 3 / 4) << shift;
                *p = (*p & 0xff00_0000) | dim(16) | dim(8) | dim(0);
            }
        }
    }
}

type Glyph = (Metrics, Vec<u8>);

type Drawn = Vec<(u32, Option<HashMap<char, Glyph>>)>;
static DRAWN: std::sync::Mutex<Drawn> = std::sync::Mutex::new(Vec::new());

// A parsed font holds a couple of hundred megabytes and parsing it takes long enough
// to stop the animation, so this runs off the main thread and lets each font go once drawn.
fn draw_fallback(px: f32, main: &Font) {
    let Ok(mut drawn) = DRAWN.lock() else { return };
    if drawn.iter().any(|(size, _)| *size == px.to_bits()) {
        return;
    }
    drawn.push((px.to_bits(), None));
    let mut wanted: Vec<char> = crate::lang::book::CS.chars().chain(crate::lang::book::EN.chars()).filter(|&c| main.lookup_glyph_index(c) == 0).collect();
    wanted.sort_unstable();
    wanted.dedup();
    std::thread::spawn(move || {
        let mut glyphs: HashMap<char, Glyph> = HashMap::new();
        for file in crate::platform::host::font_files().fallback {
            if wanted.iter().all(|c| glyphs.contains_key(c)) {
                break;
            }
            let Some(font) = std::fs::read(&file).ok().and_then(|b| Font::from_bytes(b, FontSettings::default()).ok()) else {
                continue;
            };
            for &c in &wanted {
                if !glyphs.contains_key(&c) && font.lookup_glyph_index(c) != 0 {
                    glyphs.insert(c, font.rasterize(c, px));
                }
            }
        }
        if let Ok(mut drawn) = DRAWN.lock()
            && let Some(slot) = drawn.iter_mut().find(|(size, _)| *size == px.to_bits())
        {
            slot.1 = Some(glyphs);
        }
    });
}

// `Some(None)`: no fallback font has it. `None`: the glyphs for this size are still drawing.
fn fallback_glyph(ch: char, px: f32, main: &Font) -> Option<Option<Glyph>> {
    draw_fallback(px, main);
    let drawn = DRAWN.lock().ok()?;
    let glyphs = drawn.iter().find(|(size, _)| *size == px.to_bits())?.1.as_ref()?;
    Some(glyphs.get(&ch).cloned())
}

struct Text {
    font: Option<Font>,
    px: f32,
    glyphs: HashMap<char, (Metrics, Vec<u8>)>,
    stand_in: Option<Glyph>,
}

impl Text {
    fn load(px: f32) -> Text {
        let font = crate::platform::host::font_files()
            .main
            .iter()
            .filter_map(|f| std::fs::read(f).ok())
            .find_map(|bytes| Font::from_bytes(bytes, FontSettings::default()).ok());
        if let Some(font) = &font {
            draw_fallback(px, font);
        }
        Text { font, px, glyphs: HashMap::new(), stand_in: None }
    }

    fn glyph(&mut self, ch: char) -> Option<&(Metrics, Vec<u8>)> {
        let font = self.font.as_ref()?;
        let px = self.px;
        if !self.glyphs.contains_key(&ch) {
            let glyph = match (font.lookup_glyph_index(ch) == 0).then(|| fallback_glyph(ch, px, font)) {
                Some(None) => {
                    self.stand_in = Some(font.rasterize(ch, px));
                    return self.stand_in.as_ref();
                }
                Some(Some(Some(glyph))) => glyph,
                _ => font.rasterize(ch, px),
            };
            self.glyphs.insert(ch, glyph);
        }
        self.glyphs.get(&ch)
    }

    fn advance(&mut self, s: &str) -> f32 {
        s.chars().map(|c| self.glyph(c).map_or(0.0, |g| g.0.advance_width)).sum()
    }

    fn metrics(&self) -> (f32, f32) {
        self.font
            .as_ref()
            .and_then(|f| f.horizontal_line_metrics(self.px))
            .map_or((self.px * 1.3, self.px), |m| (m.new_line_size, m.ascent))
    }

    fn wrap(&mut self, text: &str, max: f32) -> Vec<String> {
        text.split('\n').flat_map(|paragraph| self.wrap_paragraph(paragraph, max)).collect()
    }

    fn wrap_paragraph(&mut self, text: &str, max: f32) -> Vec<String> {
        if self.advance(text) <= max {
            return vec![text.to_string()];
        }
        let mut lines = Vec::new();
        let mut line = String::new();
        let mut rest = text;
        while !rest.is_empty() {
            let after = rest.trim_start_matches(' ');
            let gap = &rest[..rest.len() - after.len()];
            let end = after.find(' ').unwrap_or(after.len());
            let word = &after[..end];
            rest = &after[end..];
            let candidate = format!("{line}{gap}{word}");
            if self.advance(&candidate) <= max {
                line = candidate;
                continue;
            }
            if self.advance(word) <= max {
                if !line.trim().is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                line.push_str(word);
                continue;
            }
            line.push_str(gap);
            for ch in word.chars() {
                line.push(ch);
                if self.advance(&line) > max && line.chars().count() > 1 {
                    let last = line.pop().unwrap_or(ch);
                    lines.push(std::mem::take(&mut line));
                    line.push(last);
                }
            }
        }
        if !line.trim().is_empty() {
            lines.push(line);
        }
        lines
    }

    fn cut(&mut self, line: &str, max: f32) -> String {
        let mut cut = line.trim_end().to_string();
        while !cut.is_empty() && self.advance(&format!("{cut}...")) > max {
            cut.pop();
        }
        format!("{}...", cut.trim_end())
    }

    fn draw(&mut self, canvas: &mut Canvas, s: &str, x: f32, baseline: f32, color: Rgb, alpha: u32) {
        let mut pen = x;
        for ch in s.chars() {
            let Some((m, cov)) = self.glyph(ch) else { return };
            let m = *m;
            let gx = (pen + m.xmin as f32).round() as i32;
            let gy = (baseline - m.height as f32 - m.ymin as f32).round() as i32;
            for row in 0..m.height {
                for col in 0..m.width {
                    let a = cov[row * m.width + col] as u32 * alpha / 255;
                    canvas.blend(gx + col as i32, gy + row as i32, color, a as u8);
                }
            }
            pen += m.advance_width;
        }
    }
}

#[cfg(target_os = "linux")]
mod menu;
mod panel;
mod scene;

pub use panel::{PANEL_WHEEL_ROWS, Panel, Qr, Row, Sheet, Tail, Tone, panel_place};
pub use scene::Scene;
#[cfg(target_os = "linux")]
pub use menu::Menu;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::sprite::{Gait, Idle, Visor};

    #[test]
    fn a_pipe_shows_its_top_rows_above_its_foot_and_nothing_below() {
        let scale = 2;
        let mut canvas = Canvas::new(W * scale, H * scale);
        let base = (H * scale) as i32 - 10;
        pipe(&mut canvas, 0, base, 4 * scale as i32, scale);
        let drawn: Vec<usize> = (0..canvas.height).filter(|&y| canvas.pixels[y * canvas.width..(y + 1) * canvas.width].iter().any(|&p| p != 0)).collect();
        assert_eq!(drawn.first(), Some(&(base as usize - 4 * scale)), "rising: only its top shows");
        assert_eq!(drawn.last(), Some(&(base as usize - 1)), "nothing below its foot");
    }

    #[test]
    fn nothing_of_raccy_shows_below_the_edge_he_is_behind() {
        let pose = Pose {
            activity: Activity::Content,
            frame: 6,
            busy: false,
            feast: false,
            squash: false,
            morsel: None,
            led: NEON_CYAN,
            alert: false,
            petted: false,
            stage: 1,
            neglect: 0.0,
            idle: Idle::Still,
            gait: Gait::Still,
            facing_left: false,
            visor: Visor::Up,
        };
        let drawn = |canvas: &Canvas, from_row: usize| canvas.pixels.chunks(canvas.width).skip(from_row).flatten().filter(|p| **p >> 24 != 0).count();
        let edge = (SPRITE_Y + 11) * 4;
        let mut scene = Scene::new(4);
        let frame = scene.frame(&pose, None);
        assert!(drawn(frame, edge) > 0, "normally his lower rows are drawn");
        let mut cut = Canvas::new(0, 0);
        clip_rows(frame, edge, &mut cut);
        assert_eq!(drawn(&cut, edge), 0, "nothing below the edge");
        assert_eq!(cut.pixels[..edge * cut.width], frame.pixels[..edge * frame.width], "everything above it");
    }

    fn drawn_fallback(ch: char, px: f32, main: &Font) -> Option<Glyph> {
        for _ in 0..600 {
            if let Some(glyph) = fallback_glyph(ch, px, main) {
                return glyph;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        None
    }

    #[test]
    fn every_character_in_the_voice_files_can_be_drawn() {
        let text = Text::load(16.0);
        let Some(font) = text.font.as_ref() else { return };
        let missing: std::collections::BTreeSet<char> = crate::lang::book::CS
            .chars()
            .chain(crate::lang::book::EN.chars())
            .filter(|c| !c.is_control())
            .filter(|&c| font.lookup_glyph_index(c) == 0 && drawn_fallback(c, 16.0, font).is_none())
            .collect();
        assert!(missing.is_empty(), "no glyph for {missing:?}");
    }

}
