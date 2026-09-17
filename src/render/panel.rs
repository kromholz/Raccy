use super::*;

pub struct Sheet {
    pub name: String,
    pub stage: String,
    pub about: Vec<String>,
    pub meters: Vec<(String, f32)>,
    pub rows: Vec<String>,
    pub warnings: Vec<String>,
}

const PANEL_W: usize = 150;
const PANEL_H: usize = 116;
const PANEL_TAIL: usize = 5;
pub const PANEL_WHEEL_ROWS: i32 = 3;

// x and y are in canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tail {
    Down { x: i32 },
    Right { y: i32 },
    Left { y: i32 },
}

// `raccy` and `work` are (left, top, right, bottom); `sprite_y` is in sprite rows.
pub fn panel_place(raccy: (i32, i32, i32, i32), sprite_y: i32, size: (i32, i32), work: (i32, i32, i32, i32), scale: i32) -> ((i32, i32), Tail) {
    let (left, top, _, _) = raccy;
    let (w, h) = size;
    let (work_left, work_top, work_right, work_bottom) = work;
    let tail = PANEL_TAIL as i32 * scale;
    let clamp_x = |x: i32| x.clamp(work_left, (work_right - w).max(work_left));
    let clamp_y = |y: i32| y.clamp(work_top, (work_bottom - h).max(work_top));
    let sprite_left = left + SPRITE_X as i32 * scale;
    let sprite_right = sprite_left + sprite::SIZE as i32 * scale;
    let sprite_top = top + sprite_y * scale;
    let head = (sprite_left + (sprite::SIZE / 2) as i32 * scale, sprite_top + 10 * scale);
    // His own speech bubble does not show while this one is open, so the space it
    // keeps above him is free.
    let above = sprite_top - h + scale;
    if above >= work_top {
        let at = (clamp_x(sprite_right - w + tail), above);
        return (at, Tail::Down { x: head.0 - at.0 });
    }
    let y = clamp_y(sprite_top + sprite::SIZE as i32 * scale - h);
    if sprite_left - w + tail >= work_left {
        let at = (sprite_left - w + tail, y);
        return (at, Tail::Right { y: head.1 - at.1 });
    }
    let at = (clamp_x(sprite_right - tail), y);
    (at, Tail::Left { y: head.1 - at.1 })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Warn,
    Quiet,
}

pub struct Row {
    pub text: String,
    pub tone: Tone,
    pub indent: usize,
}

// 0 when the row is not a "label   value" pair.
fn column(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let start = chars.iter().position(|c| *c != ' ').unwrap_or(0);
    let gap = (start..chars.len().saturating_sub(1)).find(|&i| chars[i] == ' ' && chars[i + 1] == ' ');
    gap.and_then(|g| (g..chars.len()).find(|&i| chars[i] != ' ')).filter(|&i| i <= 24).unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Qr {
    pub size: usize,
    pub modules: Vec<bool>,
}

pub struct Panel {
    scale: usize,
    canvas: Canvas,
    text: Text,
    title: String,
    rows: Vec<Row>,
    lines: Vec<(String, Tone)>,
    busy: bool,
    scroll: usize,
    age: u32,
    rng: Rng,
    pub scanlines: bool,
    voice: String,
    voice_age: u32,
    pub qr: Option<Qr>,
    card: Option<(Pose, Sheet)>,
    generation: u64,
    drawn: Option<u64>,
}

impl Panel {
    pub fn new(scale: usize) -> Panel {
        let (width, height) = (PANEL_W * scale, PANEL_H * scale);
        Panel {
            scale,
            canvas: Canvas::new(width, height),
            text: Text::load(text_px(scale)),
            title: String::new(),
            rows: Vec::new(),
            lines: Vec::new(),
            busy: true,
            scroll: 0,
            age: 0,
            rng: Rng::new(0x2545_f491_4f6c_dd1d),
            scanlines: true,
            voice: String::new(),
            voice_age: 0,
            qr: None,
            card: None,
            generation: 0,
            drawn: None,
        }
    }

    pub fn set_card(&mut self, pose: Pose, sheet: Sheet) {
        self.title = sheet.name.clone();
        (self.busy, self.qr) = (false, None);
        self.card = Some((pose, sheet));
    }

    pub fn size(&self) -> (usize, usize) {
        (self.canvas.width, self.canvas.height)
    }

    pub fn scale(&self) -> usize {
        self.scale
    }

    pub fn set(&mut self, title: &str, body: Option<&str>) {
        let rows = body.unwrap_or_default().lines().map(|text| Row { text: text.to_string(), tone: Tone::Plain, indent: column(text) }).collect();
        self.set_rows(title, rows, true);
        self.busy = body.is_none();
    }

    pub fn set_rows(&mut self, title: &str, rows: Vec<Row>, fresh: bool) {
        self.title = title.to_string();
        self.busy = false;
        (self.qr, self.card) = (None, None);
        self.generation += 1;
        let max = self.text_width();
        let space = self.text.advance(" ").max(1.0);
        let mut lines = Vec::new();
        for row in &rows {
            let indent = " ".repeat(row.indent);
            for (i, line) in self.text.wrap(&row.text, max - space * row.indent as f32).into_iter().enumerate() {
                lines.push((if i == 0 { line } else { format!("{indent}{line}") }, row.tone));
            }
        }
        // New rows come in at the top, so every index shifts.
        let reading = self.lines.get(self.scroll).cloned().filter(|_| self.scroll > 0);
        self.rows = rows;
        self.lines = lines;
        match fresh {
            true => (self.scroll, self.age) = (0, 0),
            false => {
                if let Some(at) = reading.and_then(|line| self.lines.iter().position(|l| *l == line)) {
                    self.scroll = at;
                }
                self.scroll(0)
            }
        }
    }

    pub fn scroll(&mut self, rows: i32) {
        let last = self.lines.len().saturating_sub(self.rows());
        self.scroll = (self.scroll as i32 + rows).clamp(0, last as i32) as usize;
    }

    pub fn plain(&self) -> String {
        if let Some((_, sheet)) = &self.card {
            let meters = sheet.meters.iter().map(|(label, percent)| format!("{label} {percent:.0}%"));
            let body: Vec<String> =
                [sheet.stage.clone()].into_iter().chain(sheet.about.clone()).chain(meters).chain(sheet.rows.clone()).chain(sheet.warnings.clone()).collect();
            return format!("{}\n\n{}", self.title, body.join("\n"));
        }
        let body: Vec<&str> = self.rows.iter().map(|r| r.text.as_str()).collect();
        format!("{}\n\n{}", self.title, body.join("\n"))
    }

    fn card_lines(&mut self, rows: &[String], x: f32, max: f32, baseline: &mut f32, limit: f32, colour: Rgb) {
        let (lh, ascent) = self.text.metrics();
        for row in rows {
            for line in self.text.wrap(row, max) {
                if *baseline + lh - ascent > limit {
                    return;
                }
                self.text.draw(&mut self.canvas, &line, x, *baseline, colour, 255);
                *baseline += lh;
            }
        }
    }

    fn card_height(&mut self, rows: &[String], max: f32) -> f32 {
        let (lh, _) = self.text.metrics();
        rows.iter().map(|row| self.text.wrap(row, max).len()).sum::<usize>() as f32 * lh
    }

    fn draw_card(&mut self, x: f32, top: f32) {
        let Some((pose, sheet)) = self.card.take() else { return };
        let s = self.scale as i32;
        let (lh, ascent) = self.text.metrics();
        let (edge, pad) = ((s / 2).max(1), 3 * s);
        let side = sprite::SIZE as i32 * s;
        let frame = (x as i32 + edge, top as i32 + edge, side + 2 * pad, side + 2 * pad);
        let (x0, y0) = (frame.0 + pad, frame.1 + pad);
        let c = &mut self.canvas;
        c.rect(frame.0, frame.1, frame.2, frame.3, PANEL, 255);
        c.frame_rect(frame.0 + edge, frame.1 + edge, frame.2, frame.3, edge, NEON_RED, 150);
        c.frame_rect(frame.0, frame.1, frame.2, frame.3, edge, NEON_CYAN, 255);
        let px = sprite::draw(&pose);
        for (y, row) in px.iter().enumerate() {
            for (px_x, cell) in row.iter().enumerate() {
                let Some(Px { c: colour, glow: true }) = *cell else { continue };
                c.rect(x0 + (px_x as i32 - 1) * s, y0 + (y as i32 - 1) * s, 3 * s, 3 * s, colour, 26);
            }
        }
        for (y, row) in px.iter().enumerate() {
            for (px_x, cell) in row.iter().enumerate() {
                if let Some(Px { c: colour, .. }) = *cell {
                    c.rect(x0 + px_x as i32 * s, y0 + y as i32 * s, s, s, colour, 255);
                }
            }
        }

        let dx = (frame.0 + frame.2 + 6 * s) as f32;
        let max = x + self.text_width() - dx;
        let limit = (self.canvas.height - PANEL_TAIL * self.scale) as f32 - self.padding() - lh * 3.0;
        let mut baseline = top + ascent;
        self.text.draw(&mut self.canvas, &sheet.stage, dx, baseline, NEON_CYAN, 255);
        baseline += lh * 1.5;
        self.card_lines(&sheet.about, dx, max, &mut baseline, limit, TEXT);
        baseline += lh * 0.5;

        let gap = self.text.advance("   ");
        let label_w = sheet.meters.iter().map(|(label, _)| self.text.advance(label)).fold(0.0, f32::max) + gap;
        let room = (max - label_w - gap - self.text.advance("100%")) as i32;
        for (label, percent) in &sheet.meters {
            self.text.draw(&mut self.canvas, label, dx, baseline, TEXT, 255);
            let (bx, bw, bh) = ((dx + label_w) as i32, (40 * s).min(room), (5 * s / 2).max(2));
            let by = (baseline - ascent * 0.75) as i32;
            let filled = (bw as f32 * percent.clamp(0.0, 100.0) / 100.0).round() as i32;
            let colour = if *percent < 30.0 { NEON_RED } else { NEON_CYAN };
            self.canvas.rect(bx, by, bw, bh, colour, 36);
            self.canvas.rect(bx, by, filled, bh, colour, 255);
            self.canvas.frame_rect(bx, by, bw, bh, (s / 2).max(1), colour, 170);
            self.text.draw(&mut self.canvas, &format!("{percent:.0}%"), (bx + bw) as f32 + gap, baseline, TEXT, 255);
            baseline += lh;
        }
        baseline += lh * 0.5;
        // The rows give way to the warnings, not the other way round.
        let warnings = self.card_height(&sheet.warnings, max);
        self.card_lines(&sheet.rows, dx, max, &mut baseline, limit - warnings - lh * 0.5, TEXT);
        baseline += lh * 0.5;
        self.card_lines(&sheet.warnings, dx, max, &mut baseline, limit, ALERT);
        self.card = Some((pose, sheet));
    }

    fn padding(&self) -> f32 {
        4.0 * self.scale as f32
    }

    fn text_width(&self) -> f32 {
        (self.canvas.width - 2 * PANEL_TAIL * self.scale) as f32 - 2.0 * self.padding() - 3.0 * self.scale as f32
    }

    fn rows(&self) -> usize {
        let (lh, _) = self.text.metrics();
        let box_h = (self.canvas.height - PANEL_TAIL * self.scale) as f32;
        ((box_h - 2.0 * self.padding() - 4.6 * lh) / lh).floor().max(1.0) as usize
    }

    pub fn frame(&mut self, tail: Tail, voice: Option<(&str, bool)>, hint: &str, busy_text: &str) -> Option<&Canvas> {
        self.age = self.age.saturating_add(1);
        let (said, alert) = voice.unwrap_or(("", false));
        if said != self.voice {
            (self.voice, self.voice_age) = (said.to_string(), 0);
        }
        self.voice_age = self.voice_age.saturating_add(1);
        let key = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (tail, alert, &self.voice, hint, busy_text, self.generation, self.scroll, self.qr.is_some(), self.scanlines).hash(&mut hasher);
            hasher.finish()
        };
        let moving = self.busy || self.card.is_some() || self.age <= DECODE_FRAMES || self.voice_age <= DECODE_FRAMES;
        if !moving && self.drawn == Some(key) {
            return None;
        }
        self.drawn = Some(key);
        let s = self.scale as i32;
        let (w, h) = (self.canvas.width as i32, self.canvas.height as i32);
        let t = PANEL_TAIL as i32 * s;
        let edge = (s / 2).max(1);
        // The box, inside the margin the tail may use.
        let (bx, by, bw, bh) = (t, 0, w - 2 * t, h - t);
        let pad = self.padding();
        let (lh, ascent) = self.text.metrics();
        let rows = self.rows();

        self.canvas.clear();
        let c = &mut self.canvas;
        c.chrome(bx, by, bw, bh, edge, 236);
        for step in 0..PANEL_TAIL as i32 {
            let half = (PANEL_TAIL as i32 - step) * s;
            match tail {
                Tail::Down { x } => {
                    let x = x.clamp(bx + half + 4 * s, bx + bw - half - 4 * s);
                    let y = by + bh - 2 * edge + step * s;
                    c.rect(x - half - edge, y, 2 * half + 2 * edge, s, NEON_CYAN, 255);
                    c.rect(x - half, y, 2 * half, s, PANEL, 236);
                }
                Tail::Right { y } | Tail::Left { y } => {
                    let y = y.clamp(by + half + 4 * s, by + bh - half - 4 * s);
                    let x = match tail {
                        Tail::Right { .. } => bx + bw - 2 * edge + step * s,
                        _ => bx - (step + 1) * s + 2 * edge,
                    };
                    c.rect(x, y - half - edge, s, 2 * half + 2 * edge, NEON_CYAN, 255);
                    c.rect(x, y - half, s, 2 * half, PANEL, 236);
                }
            }
        }

        let x = bx as f32 + pad;
        let baseline = by as f32 + pad + ascent;
        let title = self.title.clone();
        self.text.draw(&mut self.canvas, &title, x - 1.0, baseline, NEON_RED, 120);
        self.text.draw(&mut self.canvas, &title, x, baseline, NEON_CYAN, 255);
        let rule = (by as f32 + pad + lh * 1.2) as i32;
        self.canvas.rect(bx + 4 * s, rule, bw - 8 * s, (s / 4).max(1), NEON_CYAN, 90);

        let top = by as f32 + pad + lh * 1.6;
        if self.busy {
            let noise: String = (0..3 + (self.age / 3) % 4).map(|_| noise_glyph(&mut self.rng)).collect();
            self.text.draw(&mut self.canvas, &format!("{busy_text} {noise}"), x, top + ascent, TEXT, 255);
        } else if self.card.is_some() {
            self.draw_card(x, top);
        } else if let Some(qr) = &self.qr {
            // QR readers need a quiet margin of four modules: hence the +8 and the +4.
            let cells = qr.size as i32 + 8;
            let room = (self.text_width() as i32).min((rows as f32 * lh) as i32);
            let m = (room / cells).max(1);
            let side = cells * m;
            let (qx, qy) = (bx + (bw - side) / 2, top as i32);
            self.canvas.rect(qx, qy, side, side, QR_LIGHT, 255);
            for (i, _) in qr.modules.iter().enumerate().filter(|(_, dark)| **dark) {
                let (col, row) = ((i % qr.size) as i32, (i / qr.size) as i32);
                self.canvas.rect(qx + (col + 4) * m, qy + (row + 4) * m, m, m, PANEL, 255);
            }
        } else {
            let visible: Vec<(String, Tone)> = self.lines.iter().skip(self.scroll).take(rows).cloned().collect();
            for (i, (line, tone)) in visible.iter().enumerate() {
                let shown = decoding(line, self.age, &mut self.rng);
                let colour = match tone {
                    Tone::Plain => TEXT,
                    Tone::Warn => ALERT,
                    Tone::Quiet => DIM,
                };
                self.text.draw(&mut self.canvas, &shown, x, top + ascent + i as f32 * lh, colour, 255);
            }
            if self.lines.len() > rows {
                let track_top = top as i32;
                let track_h = (rows as f32 * lh) as i32;
                let thumb_h = (track_h * rows as i32 / self.lines.len() as i32).max(3 * s);
                let thumb_y = track_top + (track_h - thumb_h) * self.scroll as i32 / (self.lines.len() - rows) as i32;
                let bar_x = bx + bw - edge - 3 * s;
                self.canvas.rect(bar_x, track_top, s, track_h, NEON_CYAN, 40);
                self.canvas.rect(bar_x, thumb_y, s, thumb_h, NEON_CYAN, 220);
            }
        }
        let max = self.text_width();
        let mut lines = self.text.wrap(&self.voice, max);
        if lines.len() > 2 {
            lines.truncate(2);
            let second = lines[1].clone();
            lines[1] = self.text.cut(&second, max);
        }
        let colour = if alert { ALERT } else { NEON_CYAN };
        let voice_top = (by + bh) as f32 - pad - lh * 3.0;
        for (i, line) in lines.iter().enumerate() {
            let shown = decoding(line, self.voice_age, &mut self.rng);
            let baseline = voice_top + ascent + i as f32 * lh;
            self.text.draw(&mut self.canvas, &shown, x - 1.0, baseline, NEON_RED, 110);
            self.text.draw(&mut self.canvas, &shown, x, baseline, colour, 255);
        }

        let footer = (by + bh) as f32 - pad - (lh - ascent);
        self.text.draw(&mut self.canvas, hint, x, footer, DIM, 255);
        // Scanlines over a QR code would keep phones from reading it.
        if self.scanlines && self.qr.is_none() {
            self.canvas.scanlines();
        }
        Some(&self.canvas)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::sprite::{Gait, Idle, Visor};

    #[test]
    fn the_panel_wraps_scrolls_and_copies() {
        let mut panel = Panel::new(4);
        panel.set("Co mám otevřeno", None);
        assert!(panel.frame(Tail::Down { x: 500 }, None, "hint", "čmuchám").expect("drawn").pixels.iter().any(|p| p >> 24 != 0));
        let body: String = (0..60).map(|i| format!("  TCP {i}\n")).collect();
        panel.set("Co mám otevřeno", Some(&body));
        let rows = panel.rows();
        assert!((15..60).contains(&rows), "{rows}");
        panel.scroll(1000);
        assert_eq!(panel.scroll, 60 - rows);
        panel.scroll(-1000);
        assert_eq!(panel.scroll, 0);
        assert!(panel.plain().starts_with("Co mám otevřeno\n\n  TCP 0\n"));
        for tail in [Tail::Down { x: 0 }, Tail::Right { y: 9999 }, Tail::Left { y: -5 }] {
            let canvas = panel.frame(tail, Some(("zkopírováno. schránka je tvoje", false)), "hint", "").expect("a new tail draws");
            assert_eq!(canvas.pixels.len(), canvas.width * canvas.height);
        }
    }

    #[test]
    fn a_still_bubble_is_not_drawn_again() {
        let mut panel = Panel::new(2);
        panel.set("title", Some("body\nmore"));
        let tail = Tail::Down { x: 50 };
        for _ in 0..=DECODE_FRAMES + 1 {
            panel.frame(tail, None, "hint", "");
        }
        assert!(panel.frame(tail, None, "hint", "").is_none(), "nothing changed");
        assert!(panel.frame(tail, Some(("a new line", false)), "hint", "").is_some(), "a new line");
        for _ in 0..=DECODE_FRAMES + 1 {
            panel.frame(tail, Some(("a new line", false)), "hint", "");
        }
        assert!(panel.frame(tail, Some(("a new line", false)), "hint", "").is_none(), "the line has settled");
        assert!(panel.frame(Tail::Down { x: 60 }, Some(("a new line", false)), "hint", "").is_some(), "he moved");
        panel.set("title", Some("other"));
        assert!(panel.frame(Tail::Down { x: 60 }, Some(("a new line", false)), "hint", "").is_some(), "new content");
    }

    #[test]
    fn journal_rows_keep_their_tone_and_line_up_under_themselves() {
        let mut panel = Panel::new(4);
        let long = format!("15.09. 12:14 ◆ {}", "slovo ".repeat(40));
        let rows = vec![
            Row { text: long, tone: Tone::Warn, indent: 15 },
            Row { text: "15.09. 12:15 ○ krátký".into(), tone: Tone::Quiet, indent: 15 },
        ];
        panel.set_rows("Deník", rows, true);
        assert!(panel.lines.len() >= 3, "{:?}", panel.lines);
        assert!(panel.lines[1].0.starts_with(&" ".repeat(15)) && panel.lines[1].1 == Tone::Warn, "{:?}", panel.lines[1]);
        assert_eq!(panel.lines.last().map(|l| l.1), Some(Tone::Quiet));
        assert!(panel.plain().ends_with("15.09. 12:15 ○ krátký"));

        let rows = |texts: &[String]| texts.iter().map(|t| Row { text: t.clone(), tone: Tone::Plain, indent: 0 }).collect::<Vec<Row>>();
        let many: Vec<String> = (0..80).map(|i| format!("row {i}")).collect();
        panel.set_rows("Deník", rows(&many), true);
        panel.scroll(10);
        let more: Vec<String> = std::iter::once("new".to_string()).chain(many).collect();
        panel.set_rows("Deník", rows(&more), false);
        assert_eq!(panel.scroll, 11, "an update keeps the reader on the same rows");
        let full: Vec<String> = std::iter::once("newer".to_string()).chain(more[..more.len() - 1].iter().cloned()).collect();
        panel.set_rows("Deník", rows(&full), false);
        assert_eq!(panel.lines[panel.scroll].0, "row 10", "still on the row being read");
    }

    #[test]
    fn a_hash_wraps_under_its_label_and_never_past_the_edge() {
        let mut panel = Panel::new(4);
        let hash = "0123456789abcdef".repeat(4);
        panel.set("soubor", Some(&format!("sha-256         {hash}")));
        let max = panel.text_width();
        let widest = panel.lines.clone().iter().map(|(l, _)| panel.text.advance(l)).fold(0.0, f32::max);
        assert!(widest <= max, "{widest} > {max}: {:?}", panel.lines);
        assert!(panel.lines[0].0.starts_with("sha-256") && panel.lines[0].0.len() > 20, "{:?}", panel.lines);
        assert!(panel.lines[1].0.starts_with(&" ".repeat(16)), "{:?}", panel.lines);
        assert_eq!(panel.plain().matches(&hash).count(), 1, "copied whole");
    }

    #[test]
    fn a_qr_code_in_the_panel_reads_back() {
        let text = "WIFI:T:WPA;S:home;P:secret;;";
        let code = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium).unwrap();
        let size = code.size() as usize;
        let modules = (0..size * size).map(|i| code.get_module((i % size) as i32, (i / size) as i32)).collect();
        let mut panel = Panel::new(2);
        panel.set("wifi", Some("SSID  home"));
        panel.qr = Some(Qr { size, modules });
        let canvas = panel.frame(Tail::Down { x: 100 }, None, "hint", "busy").expect("drawn");
        let w = canvas.width;
        let grey = |x: usize, y: usize| {
            let p = canvas.pixels[y * w + x];
            (((p >> 16) & 0xff) + ((p >> 8) & 0xff) + (p & 0xff)) as f32 / 3.0
        };
        let mut image = rqrr::PreparedImage::prepare_from_greyscale(w, canvas.height, |x, y| grey(x, y) as u8);
        let grids = image.detect_grids();
        assert_eq!(grids.len(), 1);
        assert_eq!(grids[0].decode().unwrap().1, text);
    }

    #[test]
    fn the_panel_sits_above_raccy_or_beside_him() {
        let work = (0, 0, 2560, 1392);
        let (at, tail) = panel_place((2300, 1150, 2556, 1390), SPRITE_Y as i32, (600, 464), work, 4);
        assert_eq!(at, (1960, 1150 + SPRITE_Y as i32 * 4 - 464 + 4));
        assert_eq!(tail, Tail::Down { x: 2300 + (SPRITE_X as i32 + 16) * 4 - 1960 });
        let (at, tail) = panel_place((1000, 100, 1256, 340), SPRITE_Y as i32, (600, 464), work, 4);
        assert!(matches!(tail, Tail::Right { .. }) && at.0 == 1000 + SPRITE_X as i32 * 4 - 600 + 20, "{at:?} {tail:?}");
        let (at, tail) = panel_place((100, 100, 356, 340), SPRITE_Y as i32, (600, 464), work, 4);
        assert!(matches!(tail, Tail::Left { .. }) && at.0 == 100 + (SPRITE_X + sprite::SIZE) as i32 * 4 - 20, "{at:?} {tail:?}");
    }

    #[test]
    fn the_character_screen_fits_in_the_bubble() {
        // Every pixel scale a monitor's DPI gives, from 100 % up.
        for scale in [2, 3, 4, 5, 6, 8] {
            card_fits(scale);
        }
    }

    fn card_fits(scale: usize) {
        let mut panel = Panel::new(scale);
        let pose = Pose {
            activity: Activity::Content,
            frame: 6,
            busy: true,
            feast: true,
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
        // The most a real sheet has: five rows and the muted note.
        let rows = (0..6).map(|i| format!("row {i}, today ↓35.2 GB ↑2.2 GB")).collect();
        let sheet = Sheet {
            name: "Raccy".into(),
            stage: "fence".into(),
            about: vec!["stage 2 of 4".into(), "next stage in 9 well-fed days".into(), "3 days old".into()],
            meters: vec![("full".into(), 72.0), ("mood".into(), 20.0)],
            rows,
            warnings: vec!["open to the network: remote desktop, vnc on port 5800, file sharing".into()],
        };
        panel.set_card(pose, sheet);
        assert!(panel.plain().contains("open to the network: remote desktop"), "copies as text");
        // The two lines of his voice and the hint begin here: the details must end above.
        let (lh, _) = panel.text.metrics();
        let voice_top = ((PANEL_H - PANEL_TAIL) * scale) as f32 - panel.padding() - lh * 3.0;
        let canvas = panel.frame(Tail::Down { x: 100 }, None, "hint", "busy").expect("drawn");
        // The warning's amber by its hue, not by how bright it came out: a
        // narrower font covers fewer pixels of a stroke whole.
        let amber = |p: &u32| {
            let (r, g, b) = ((p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff);
            r > 0x40 && 10 * g > 5 * r && 10 * g < 8 * r && 10 * b < 3 * r
        };
        let at = |y: usize| &canvas.pixels[y * canvas.width..(y + 1) * canvas.width];
        assert!((0..voice_top as usize).any(|y| at(y).iter().any(amber)), "the warnings are drawn at scale {scale}");
        assert!(!(voice_top as usize..canvas.height).any(|y| at(y).iter().any(amber)), "and fit above his voice at scale {scale}");
    }
}
