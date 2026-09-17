#[cfg(feature = "trace")]
use std::time::Duration;
use std::time::Instant;

use super::App;

#[derive(Clone, Copy)]
pub(super) struct Glide {
    pub(super) from: (i32, i32),
    pub(super) to: (i32, i32),
    pub(super) since: Instant,
    pub(super) secs: f32,
}

impl App {
    pub(super) fn glide(&mut self) {
        #[cfg(feature = "trace")]
        {
            static RATE: std::sync::Mutex<(u32, Option<(Instant, Instant)>)> = std::sync::Mutex::new((0, None));
            let mut rate = RATE.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            match rate.1 {
                Some((start, last)) if now - last < Duration::from_millis(100) => {
                    rate.0 += 1;
                    rate.1 = Some((start, now));
                    if now - start >= Duration::from_secs(1) {
                        let per_sec = rate.0 as f32 / (now - start).as_secs_f32();
                        crate::trace::log(|| format!("glide {per_sec:.0}/s"));
                        *rate = (0, Some((now, now)));
                    }
                }
                _ => *rate = (0, Some((now, now))),
            }
        }
        let sliding = self.pipe.is_some_and(|pipe| pipe.rows != self.pipe_from) && self.pipe_since.elapsed().as_secs_f32() < self.frame_secs;
        let Some(g) = self.glide else {
            if sliding {
                self.present_shown();
            } else {
                self.glider.moving(false);
            }
            return;
        };
        let t = (g.since.elapsed().as_secs_f32() / g.secs).min(1.0);
        let lerp = |a: i32, b: i32| a + ((b - a) as f32 * t).round() as i32;
        self.place((lerp(g.from.0, g.to.0), lerp(g.from.1, g.to.1)));
        self.follow_panel();
        if self.ground.is_some() || self.pipe.is_some() {
            self.present_shown();
        }
        if t >= 1.0 {
            self.glide = None;
            self.glider.moving(sliding);
        }
    }
}
