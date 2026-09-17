use super::*;

struct Bit {
    x: f32,
    y: f32,
    to: (f32, f32),
    speed: f32,
    big: bool,
    color: Rgb,
}

// Each colour weighted by the bytes it carries.
pub type Tastes = Vec<(Rgb, u64)>;

#[derive(Clone, Copy)]
struct Glitch {
    band: (usize, usize),
    shift: i32,
    kind: GlitchKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum GlitchKind {
    Tear,
    Dropout,
    Negative,
}

pub struct Scene {
    // Screen pixels per sprite pixel.
    scale: usize,
    canvas: Canvas,
    text: Text,
    bits: Vec<Bit>,
    to_spawn: [(u32, bool); 2],
    tastes: [Tastes; 2],
    burst_frames: u32,
    heavy_secs: u32,
    jolt_frames: u32,
    glitch: Option<(Glitch, u32)>,
    next_glitch: u32,
    shown: String,
    line_age: u32,
    rng: Rng,
    pub scanlines: bool,
    pub flipped: bool,
}

impl Scene {
    pub fn new(scale: usize) -> Scene {
        let (width, height) = (W * scale, H * scale);
        Scene {
            scale,
            canvas: Canvas::new(width, height),
            text: Text::load(text_px(scale)),
            bits: Vec::new(),
            to_spawn: [(0, false); 2],
            tastes: [Vec::new(), Vec::new()],
            burst_frames: 0,
            heavy_secs: 0,
            jolt_frames: 0,
            glitch: None,
            next_glitch: 0,
            shown: String::new(),
            line_age: 0,
            rng: Rng::new(0x9e37_79b9_7f4a_7c15),
            scanlines: true,
            flipped: false,
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.canvas.width, self.canvas.height)
    }

    pub fn sprite_y(&self) -> usize {
        if self.flipped { 0 } else { SPRITE_Y }
    }

    pub fn traffic(&mut self, rx: u64, tx: u64, tastes_in: Tastes, tastes_out: Tastes) {
        let feed = tuning().talk.feast_bps;
        let count = |b: u64| if b <= feed { 0 } else { (1.0 + (b as f32 / feed as f32).log2() * 1.5).min(24.0) as u32 };
        self.to_spawn = [(count(rx), rx > tuning().scene.big_bits_bps), (count(tx), tx > tuning().scene.big_bits_bps)];
        self.tastes = [tastes_in, tastes_out];
        if rx.max(tx) > tuning().scene.glitch_bps {
            self.burst_frames = 10;
            self.heavy_secs += 1;
        } else {
            self.heavy_secs = 0;
        }
    }

    pub fn jolt(&mut self) {
        self.jolt_frames = 30;
    }

    fn taste(&mut self, dir: usize, default: Rgb) -> Rgb {
        let total: u64 = self.tastes[dir].iter().map(|t| t.1).sum();
        if total == 0 {
            return default;
        }
        let mut pick = (self.rng.unit() * total as f32) as u64;
        for &(c, n) in &self.tastes[dir] {
            if pick < n {
                return c;
            }
            pick -= n;
        }
        default
    }

    fn glitch_pace(&self, activity: Activity) -> Option<(u32, u32)> {
        match activity {
            Activity::Sleeping => None,
            _ if self.jolt_frames > 0 => Some((3, 8)),
            _ if self.burst_frames > 0 && self.heavy_secs <= 3 => Some((8, 20)),
            _ if self.burst_frames > 0 => Some((40, 90)),
            Activity::Starving => Some((30, 60)),
            _ => Some((80, 200)),
        }
    }

    fn roll_glitch(&mut self, activity: Activity) -> Option<Glitch> {
        self.burst_frames = self.burst_frames.saturating_sub(1);
        self.jolt_frames = self.jolt_frames.saturating_sub(1);
        if let Some((glitch, left)) = self.glitch.take()
            && left > 0
        {
            self.glitch = Some((glitch, left - 1));
            return Some(glitch);
        }
        let (lo, hi) = self.glitch_pace(activity)?;
        self.next_glitch = self.next_glitch.min(hi);
        if self.next_glitch > 0 {
            self.next_glitch -= 1;
            return None;
        }
        self.next_glitch = lo + (self.rng.unit() * (hi - lo) as f32) as u32;
        let frames = if self.rng.unit() < 0.5 { 1 } else { 2 };
        let top = (self.rng.unit() * sprite::SIZE as f32) as usize;
        let height = 1 + (self.rng.unit() * 4.0) as usize;
        let kind = match self.rng.unit() {
            r if r < 0.6 => GlitchKind::Tear,
            r if r < 0.8 => GlitchKind::Dropout,
            _ => GlitchKind::Negative,
        };
        let shift = match kind {
            GlitchKind::Tear => (if self.rng.unit() < 0.5 { -1 } else { 1 }) * (1 + (self.rng.unit() * 2.0) as i32),
            _ => 0,
        };
        let glitch = Glitch { band: (top, (top + height).min(sprite::SIZE)), shift, kind };
        self.glitch = Some((glitch, frames - 1));
        Some(glitch)
    }

    // One frame is a tenth of a second.
    pub fn frame(&mut self, pose: &Pose, line: Option<(&str, bool)>) -> &Canvas {
        self.spawn(pose.frame, pose.activity);
        for bit in &mut self.bits {
            let (dx, dy) = (bit.to.0 - bit.x, bit.to.1 - bit.y);
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= bit.speed {
                bit.speed = 0.0;
            } else {
                bit.x += dx / dist * bit.speed;
                bit.y += dy / dist * bit.speed;
            }
        }
        self.bits.retain(|b| b.speed > 0.0);

        let glitch = self.roll_glitch(pose.activity);
        self.paint(pose, line, glitch)
    }

    fn paint(&mut self, pose: &Pose, line: Option<(&str, bool)>, glitch: Option<Glitch>) -> &Canvas {
        self.canvas.clear();
        let s = self.scale as i32;
        let sy = self.sprite_y();
        let px = sprite::draw(pose);
        let at = |x: usize, y: usize| -> (i32, i32) {
            let shift = match &glitch {
                Some(g) if (g.band.0..g.band.1).contains(&y) => g.shift,
                _ => 0,
            };
            let x = if pose.facing_left { sprite::SIZE - 1 - x } else { x };
            (((SPRITE_X + x) as i32 + shift) * s, (sy + y) as i32 * s)
        };

        if matches!(pose.gait, sprite::Gait::Still | sprite::Gait::Walking) {
            let row = if pose.activity == Activity::Sleeping && pose.gait == sprite::Gait::Still { 29 } else { sprite::SIZE - 1 };
            let y = (sy + row) as i32 * s;
            let x = (SPRITE_X as i32 + 6) * s;
            self.canvas.rect(x, y, 20 * s, s, [0, 0, 0], 40);
            self.canvas.rect(x + 3 * s, y, 14 * s, s, [0, 0, 0], 40);
        }

        for (y, row) in px.iter().enumerate() {
            for (x, p) in row.iter().enumerate() {
                let Some(Px { c, glow }) = *p else { continue };
                let (cx, cy) = at(x, y);
                if glow {
                    self.canvas.rect(cx - s, cy - s, 3 * s, 3 * s, c, 34);
                }
                if glitch.as_ref().is_some_and(|g| g.kind == GlitchKind::Tear) {
                    self.canvas.rect(cx - s / 2 - 1, cy, s, s, NEON_RED, 110);
                    self.canvas.rect(cx + s / 2 + 1, cy, s, s, NEON_CYAN, 110);
                }
            }
        }
        let band = glitch.as_ref().map(|g| (g.kind, g.band.0..g.band.1));
        for (y, row) in px.iter().enumerate() {
            for (x, p) in row.iter().enumerate() {
                if let Some(Px { c, .. }) = *p {
                    let (cx, cy) = at(x, y);
                    let c = match &band {
                        Some((GlitchKind::Negative, rows)) if rows.contains(&y) => c.map(|v| 255 - v),
                        Some((GlitchKind::Dropout, rows)) if rows.contains(&y) => {
                            let v = (self.rng.unit() * 255.0) as u8;
                            if v < 100 {
                                continue;
                            }
                            [v; 3]
                        }
                        _ => c,
                    };
                    self.canvas.rect(cx, cy, s, s, c, 255);
                }
            }
        }
        for i in 0..self.bits.len() {
            let b = &self.bits[i];
            let size = if b.big { 2 * s } else { s };
            let (x, y, c) = ((b.x * s as f32) as i32, (b.y * s as f32) as i32, b.color);
            self.canvas.rect(x - s / 2, y - s / 2, size + s, size + s, c, 40);
            self.canvas.rect(x, y, size, size, c, 235);
        }
        match line {
            Some((line, alert)) => {
                if line != self.shown {
                    self.shown = line.to_string();
                    self.line_age = 0;
                } else {
                    self.line_age += 1;
                }
                let jolt = glitch.as_ref().map_or(0, |g| g.shift * s / 2);
                self.bubble(line, jolt, alert);
            }
            None => self.shown.clear(),
        }
        if self.scanlines {
            self.canvas.scanlines();
        }
        &self.canvas
    }

    fn spawn(&mut self, frame: u64, activity: Activity) {
        if activity == Activity::Sleeping {
            return;
        }
        let phase = frame % sprite::BITE_FRAMES;
        let launch = sprite::BITE_FRAMES - BIT_FLIGHT_FRAMES;
        if !(launch..launch + BURST_FRAMES).contains(&phase) {
            return;
        }
        for dir in 0..2 {
            let (count, big) = self.to_spawn[dir];
            // count is a second's bits; the 10 is the frames in a second.
            let per_bite = (count * sprite::BITE_FRAMES as u32).div_ceil(10);
            let now = per_bite.div_ceil(BURST_FRAMES as u32);
            for _ in 0..now {
                let bit = if dir == 0 {
                    let sy = self.sprite_y() as f32;
                    let (x, y) = (self.rng.unit() * 6.0, sy + MOUTH.1 + (self.rng.unit() - 0.5) * 24.0);
                    let to = (SPRITE_X as f32 + MOUTH.0, sy + MOUTH.1);
                    let frames = (sprite::BITE_FRAMES - phase + 1) as f32 + self.rng.unit() * 1.5;
                    Bit {
                        x,
                        y,
                        to,
                        speed: (to.0 - x).hypot(to.1 - y) / frames,
                        big,
                        color: self.taste(0, NEON_CYAN),
                    }
                } else {
                    let (ax, ay) = (SPRITE_X as f32 + ANTENNA.0, self.sprite_y() as f32 + ANTENNA.1);
                    Bit {
                        x: ax,
                        y: ay,
                        to: (ax + (self.rng.unit() - 0.7) * 40.0, ay - 12.0 - self.rng.unit() * 10.0),
                        speed: 1.5 + self.rng.unit() * 1.5,
                        big,
                        color: self.taste(1, NEON_RED),
                    }
                };
                self.bits.push(bit);
            }
        }
    }

    fn bubble(&mut self, line: &str, jolt: i32, alert: bool) {
        let s = self.scale as f32;
        let si = self.scale as i32;
        let edge = (si / 2).max(1);
        let pad = 3.0 * s;
        let width = self.canvas.width as f32 - 2.0 * s;
        let mut lines = self.text.wrap(line, width - 2.0 * pad);
        let (lh, ascent) = self.text.metrics();
        let (bottom, top) = if self.flipped {
            (self.canvas.height as f32, (sprite::SIZE + 1) as f32 * s)
        } else {
            ((SPRITE_Y as f32 + 1.0) * s, 0.0)
        };
        let max_lines = ((bottom - top - 2.0 * pad) / lh).floor().max(1.0) as usize;
        if lines.len() > max_lines {
            lines.truncate(max_lines);
            if let Some(last) = lines.pop() {
                let cut = self.text.cut(&last, width - 2.0 * pad);
                lines.push(cut);
            }
        }
        let text_w = lines.iter().map(|l| self.text.advance(l)).fold(0.0, f32::max);
        let bw = (text_w + 2.0 * pad).ceil() as i32;
        let bh = (lines.len() as f32 * lh + 2.0 * pad).ceil() as i32;
        let tail_x = (SPRITE_X as i32 + 7) * si;
        let bx = (self.canvas.width as i32 - bw - si + jolt).min(tail_x - 3 * si).max(0);
        let by = if self.flipped { top as i32 } else { bottom as i32 - bh };

        if self.line_age < DECODE_FRAMES {
            for l in &mut lines {
                let decoded = decoding(l, self.line_age, &mut self.rng);
                *l = decoded;
            }
        }

        let c = &mut self.canvas;
        c.rect(bx, by, bw, bh, PANEL, 232);
        c.frame_rect(bx + edge, by + edge, bw, bh, edge, NEON_RED, 150);
        let border = if alert { ALERT } else { NEON_CYAN };
        c.frame_rect(bx, by, bw, bh, edge, border, 255);
        for step in 0..2 {
            let w = (2 - step) * si;
            let y = if self.flipped { by - (step + 1) * si } else { by + bh + step * si };
            c.rect(tail_x - edge, y - edge, w + 2 * edge, si + edge, border, 255);
            c.rect(tail_x, y - edge, w, si, PANEL, 232);
        }

        for (i, l) in lines.iter().enumerate() {
            let baseline = by as f32 + pad + ascent + i as f32 * lh;
            let x = bx as f32 + pad;
            self.text.draw(&mut self.canvas, l, x - 1.0, baseline, NEON_RED, 120);
            self.text.draw(&mut self.canvas, l, x, baseline, TEXT, 255);
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::sprite::{Gait, Idle, Visor};


    #[test]
    fn flipped_he_is_at_the_top_and_the_bubble_below() {
        let scale = 2;
        let pose = Pose { activity: Activity::Content, frame: 6, busy: true, feast: false, squash: false, morsel: None, led: NEON_CYAN, alert: false, petted: false, stage: 1, neglect: 0.0, idle: Idle::Still, gait: Gait::Sitting, facing_left: false, visor: Visor::Down };
        let drawn_rows = |canvas: &Canvas| -> Vec<usize> {
            (0..canvas.height).filter(|&y| canvas.pixels[y * canvas.width..(y + 1) * canvas.width].iter().any(|&p| p != 0)).collect()
        };
        let mut scene = Scene::new(scale);
        scene.scanlines = false;
        scene.flipped = true;
        let rows = drawn_rows(scene.frame(&pose, Some(("ahoj", false))));
        assert!(rows.contains(&(2 * scale)), "his hat at the top");
        assert!(rows.iter().any(|&y| y >= (sprite::SIZE + 3) * scale), "the bubble below his boots");
        let mut plain = Scene::new(scale);
        plain.scanlines = false;
        let rows = drawn_rows(plain.frame(&pose, Some(("ahoj", false))));
        assert!(!rows.contains(&(2 * scale)) && rows.contains(&((SPRITE_Y + 2) * scale)), "otherwise he is at the bottom");
        let work = (0, 0, 2560, 1400);
        let (_, tail) = panel_place((1000, 0, 1256, 240), 0, (600, 464), work, 4);
        assert!(matches!(tail, Tail::Right { .. }), "{tail:?}");
    }

    #[test]
    fn glitches_come_in_three_kinds_and_only_a_tear_slips() {
        let mut scene = Scene::new(1);
        let mut seen = Vec::new();
        for _ in 0..2000 {
            scene.jolt_frames = 5;
            if let Some(glitch) = scene.roll_glitch(Activity::Content) {
                assert_eq!(glitch.shift != 0, glitch.kind == GlitchKind::Tear);
                if !seen.contains(&glitch.kind) {
                    seen.push(glitch.kind);
                }
            }
        }
        assert_eq!(seen.len(), 3, "{seen:?}");
    }

    fn glitch_starts(scene: &mut Scene, frames: u64, heavy: bool, activity: Activity) -> Vec<u64> {
        let mut starts = Vec::new();
        let mut on = false;
        for frame in 0..frames {
            if frame % 10 == 0 {
                let rate = if heavy { tuning().scene.glitch_bps + 1 } else { 0 };
                scene.traffic(rate, 0, Vec::new(), Vec::new());
            }
            let now = scene.roll_glitch(activity).is_some();
            if now && !on {
                starts.push(frame);
            }
            on = now;
        }
        starts
    }

    #[test]
    fn glitches_are_paced_not_bunched() {
        let mut scene = Scene::new(1);
        let calm = glitch_starts(&mut scene, 12000, false, Activity::Content);
        let gaps: Vec<u64> = calm.windows(2).map(|w| w[1] - w[0]).collect();
        assert!((50..=150).contains(&calm.len()), "{}", calm.len());
        assert!(gaps.iter().all(|g| (80..=202).contains(g)), "{gaps:?}");

        let mut scene = Scene::new(1);
        let heavy = glitch_starts(&mut scene, 600, true, Activity::Content);
        let (start, later): (Vec<u64>, Vec<u64>) = heavy.iter().partition(|&&f| f < 40);
        assert!(start.len() >= 2, "a run at the start: {heavy:?}");
        assert!(start.windows(2).all(|w| w[1] - w[0] <= 22), "{start:?}");
        let gaps: Vec<u64> = later.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.iter().all(|g| (40..=92).contains(g)), "eased off: {gaps:?}");

        assert!(glitch_starts(&mut Scene::new(1), 3000, true, Activity::Sleeping).is_empty(), "none asleep");

        let mut scene = Scene::new(1);
        scene.next_glitch = 200;
        let first = glitch_starts(&mut scene, 100, true, Activity::Content).first().copied();
        assert!(first.is_some_and(|f| f <= 20), "a burst is answered soon: {first:?}");
    }

    #[test]
    fn bits_come_with_bites_on_a_real_feed() {
        let feed = tuning().talk.feast_bps;
        let flying = |rx: u64, activity: Activity| {
            let mut scene = Scene::new(2);
            scene.traffic(rx, 0, Vec::new(), Vec::new());
            (0..sprite::BITE_FRAMES)
                .map(|frame| {
                    let before = scene.bits.len();
                    scene.spawn(frame, activity);
                    for bit in &scene.bits[before..scene.bits.len()] {
                        if bit.to.1 > bit.y - 20.0 && bit.x < 10.0 {
                            let steps = ((bit.to.0 - bit.x).hypot(bit.to.1 - bit.y) / bit.speed).ceil() as u64;
                            assert!((frame + steps - 1) % sprite::BITE_FRAMES < 3, "a bit off at {frame} lands after {steps}");
                        }
                    }
                    (frame, scene.bits.len() - before)
                })
                .filter(|&(_, n)| n > 0)
                .map(|(frame, _)| frame)
                .collect::<Vec<u64>>()
        };
        assert!(flying(feed / 2, Activity::Eating).is_empty(), "a trickle throws no bits");
        let frames = flying(feed * 4, Activity::Eating);
        assert!(!frames.is_empty(), "a real feed does");
        assert!(frames.iter().all(|f| (sprite::BITE_FRAMES - BIT_FLIGHT_FRAMES..sprite::BITE_FRAMES - BIT_FLIGHT_FRAMES + BURST_FRAMES).contains(f)), "{frames:?}");
        assert!(flying(feed * 4, Activity::Sleeping).is_empty(), "not while he sleeps");
    }

    #[test]
    #[ignore]
    fn preview() {
        let tag = std::env::var("RACCY_PREVIEW_TAG").unwrap_or_else(|_| "now".into());
        let path = std::env::temp_dir().join(format!("raccy-preview-{tag}.bmp"));
        std::fs::write(&path, preview_sheet(8)).unwrap();
        println!("wrote {}", path.display());
    }

    #[test]
    fn the_scene_draws_as_its_golden_sheet() {
        crate::render::golden::check("scene", &preview_sheet(2));
    }

    fn preview_sheet(scale: usize) -> Vec<u8> {
        let calm = Pose {
            activity: Activity::Content,
            frame: 6,
            busy: true,
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
            visor: Visor::Down,
        };
        let poses = [
            calm,
            Pose { visor: Visor::Up, ..calm },
            Pose { stage: 3, ..calm },
            Pose { activity: Activity::Stuffed, ..calm },
            Pose { activity: Activity::Sleeping, visor: Visor::Up, ..calm },
            Pose { gait: Gait::Walking, frame: 0, ..calm },
            Pose { gait: Gait::Walking, frame: 2, ..calm },
            Pose { gait: Gait::Jumping, ..calm },
            Pose { gait: Gait::Sitting, idle: Idle::Snack, ..calm },
        ];
        let glitched = [GlitchKind::Tear, GlitchKind::Dropout, GlitchKind::Negative]
            .map(|kind| (calm, Some(Glitch { band: (8, 12), shift: if kind == GlitchKind::Tear { 2 } else { 0 }, kind })));
        let mut frames: Vec<Vec<u32>> = poses
            .into_iter()
            .map(|pose| (pose, None))
            .chain(glitched)
            .map(|(pose, glitch)| {
                let mut scene = Scene::new(scale);
                scene.scanlines = true;
                scene.paint(&pose, None, glitch).pixels.clone()
            })
            .collect();
        let mut down = Scene::new(scale);
        let mut piped = Canvas::new(0, 0);
        let base = ((SPRITE_Y + 22) * scale) as i32;
        clip_rows(down.paint(&calm, None, None), base as usize, &mut piped);
        pipe(&mut piped, 0, base, (PIPE_ROWS as usize * scale) as i32, scale);
        frames.push(piped.pixels);
        let (cw, ch) = (W * scale, H * scale);
        let (left, top) = ((SPRITE_X - 2) * scale, (SPRITE_Y - 2) * scale);
        let (w, h) = ((cw - left) * frames.len(), ch - top);
        let background = [0x1e_u32, 0x1e, 0x24];
        let cell = cw - left;
        crate::render::golden::bmp(w, h, |x, y| {
            let p = frames[x / cell][(top + y) * cw + left + x % cell];
            let a = p >> 24;
            // Premultiplied over the background.
            let over = |shift: u32, bg: u32| (((p >> shift) & 0xff) + bg * (255 - a) / 255).min(255) as u8;
            [over(16, background[0]), over(8, background[1]), over(0, background[2])]
        })
    }

}
