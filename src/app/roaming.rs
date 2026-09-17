use std::time::Instant;

use super::glide::Glide;
use super::{App, place_window};
use crate::pet::Activity;
use crate::platform::{self, Rect, WindowId};
use crate::render::sprite::{Gait, Idle};
use crate::tuning::tuning;
use crate::{net, render, roam, trace};

// GLANCE_PX is in sprite pixels, either way from his eyes.
const GLANCE_PX: i32 = 80;
const GLANCE_AFTER_FRAMES: u32 = 3;
const GLANCE_LINGER_FRAMES: u32 = 5;

// The sprite is drawn mirrored when he faces left, so left and right swap.
fn glance(cursor: (i32, i32), eyes: (i32, i32), scale: i32, facing_left: bool) -> Option<Idle> {
    let (dx, dy) = (cursor.0 - eyes.0, cursor.1 - eyes.1);
    if dx.abs() > GLANCE_PX * scale || dy.abs() > GLANCE_PX * scale || dx.abs() < 8 * scale {
        return None;
    }
    Some(if (dx < 0) != facing_left { Idle::LookLeft } else { Idle::LookRight })
}

impl App {
    pub(super) fn own(&self) -> WindowId {
        self.shell.id()
    }

    pub(super) fn refresh_desk(&mut self) {
        self.desk = platform::desktop::snapshot();
    }

    // Roaming counts positions with the sprite at the bottom of the window; flipped,
    // the window itself sits lower by the bubble's rows.
    pub(super) fn logical_pos(&self) -> (i32, i32) {
        let (x, y) = self.shell.pos();
        (x, y - self.layout_offset())
    }

    pub(super) fn layout_offset(&self) -> i32 {
        if self.scene.flipped { render::SPRITE_Y as i32 * self.scale as i32 } else { 0 }
    }

    pub(super) fn place(&self, (x, y): (i32, i32)) {
        self.shell.place(x, y + self.layout_offset());
    }

    pub(super) fn stay_on_top(&mut self) {
        if self.hidden || self.fullscreen.is_some() {
            return;
        }
        let own = self.own();
        let Some(frame) = self.desk.window(own).map(|w| w.frame) else { return };
        if !roam::buried(&self.desk, own, &frame) {
            return;
        }
        trace::record(|| format!("back on top at={frame:?}"));
        self.shell.raise();
        self.raise_panel();
    }

    fn fit_layout(&mut self, logical: (i32, i32)) {
        let flipped = roam::home_work_area(&self.desk, logical, self.scale as i32).is_some_and(|work| logical.1 < work.top);
        if flipped != self.scene.flipped {
            trace::record(|| format!("layout flipped={flipped}"));
            self.scene.flipped = flipped;
            if self.glide.is_none() {
                self.place(logical);
            }
        }
    }

    pub(super) fn roam_frame(&mut self, activity: Activity) -> roam::Move {
        // Mid-glide, roaming counts him at the glide's target, not where the window is.
        let here = self.glide.map_or_else(|| self.logical_pos(), |g| g.to);
        let calm = matches!(activity, Activity::Content | Activity::Eating | Activity::Bored);
        self.fullscreen = roam::fullscreen_app(&self.desk, self.pet.pos.unwrap_or(here), self.scale as i32);
        let frame = roam::Frame {
            desk: &self.desk,
            own: self.shell.id(),
            here,
            home: self.pet.pos.unwrap_or(here),
            scale: self.scale as i32,
            may_start: calm && !self.hushed && self.line.is_none() && self.panel.is_none(),
            may_stay: activity != Activity::Sleeping && !self.hushed,
            hide: self.fullscreen.is_some(),
        };
        let roaming = self.roam.step(&frame);
        self.watch_seat();
        if roaming.underground {
            if self.line.as_ref().is_some_and(|line| !line.alert) {
                self.line = None;
            }
            self.talk.hold();
        }
        if roaming.hidden != self.hidden {
            self.hidden = roaming.hidden;
            self.shell.show(!self.hidden);
        }
        match roaming.to {
            Some(to) if roaming.teleport => {
                self.glide = None;
                self.place(to);
            }
            Some(to) if to != here => {
                self.glide = Some(Glide { from: self.logical_pos(), to, since: Instant::now(), secs: self.frame_secs });
                self.glider.moving(true);
            }
            _ => {}
        }
        self.fit_layout(roaming.to.unwrap_or(here));
        if let Some(home) = roaming.home {
            self.pet.pos = Some(home);
            self.pet.save();
        }
        if let Some(event) = roaming.event.filter(|_| !self.line.as_ref().is_some_and(|l| l.alert) && self.pet.muted_until.is_none()) {
            trace::record(|| format!("roam {event:?}"));
            let text = match event {
                roam::Event::Patrol => self.lang.patrol(self.frame).to_string(),
                roam::Event::Sat(pid) => self.lang.sits_on(&net::process_of(pid), self.frame),
                roam::Event::Fell => self.lang.fell(self.frame).to_string(),
                roam::Event::Covered => self.lang.covered(self.frame).to_string(),
                roam::Event::Back => self.lang.back(self.frame).to_string(),
                roam::Event::Travel => self.lang.travel(self.frame).to_string(),
                roam::Event::Arrived => self.lang.arrived(self.frame).to_string(),
                roam::Event::Hide => {
                    let app = self.fullscreen.map(net::process_of).unwrap_or_default();
                    self.lang.hiding(&app, self.frame)
                }
                roam::Event::Escape => {
                    let app = self.fullscreen.map(net::process_of).unwrap_or_default();
                    self.lang.escaping(&app, self.frame)
                }
            };
            self.say(text, false);
            if let (roam::Event::Hide | roam::Event::Escape, Some(line)) = (event, self.line.as_mut()) {
                line.frames_left = tuning().roam.announce_frames;
            }
            self.talk.hold();
        }
        roaming
    }

    pub(super) fn glancing(&mut self, roaming: &roam::Move, activity: Activity) -> Option<Idle> {
        let near = if roaming.gait != Gait::Still || activity == Activity::Sleeping || self.hidden {
            None
        } else {
            let (x, y) = self.shell.pos();
            let s = self.scale as i32;
            let eyes = (x + (render::SPRITE_X as i32 + 16) * s, y + (self.scene.sprite_y() as i32 + 12) * s);
            self.desk.cursor.and_then(|cursor| glance(cursor, eyes, s, roaming.facing_left))
        };
        match near {
            Some(look) => {
                self.glance_near += 1;
                if self.glance_near >= GLANCE_AFTER_FRAMES {
                    self.glance_hold = Some((look, GLANCE_LINGER_FRAMES));
                }
            }
            None => {
                self.glance_near = 0;
                self.glance_hold = self.glance_hold.and_then(|(look, left)| (left > 0).then_some((look, left - 1)));
            }
        }
        self.glance_hold.map(|(look, _)| look)
    }

    pub(super) fn set_ground(&mut self, roaming: &roam::Move) {
        self.ground = roaming.ground;
        self.pipe_from = match (self.pipe, roaming.pipe) {
            (Some(old), Some(new)) if (old.x, old.base) == (new.x, new.base) => old.rows,
            _ => 0,
        };
        self.pipe_since = Instant::now();
        if roaming.pipe.is_some_and(|pipe| pipe.rows != self.pipe_from) {
            self.glider.moving(true);
        }
        self.pipe = roaming.pipe;
    }

    pub(super) fn present_shown(&mut self) {
        if self.ground.is_none() && self.pipe.is_none() {
            self.shell.present(&self.shown);
            return;
        }
        let (x, y) = self.shell.pos();
        let rows = self.ground.map_or(usize::MAX, |ground| (ground - y).max(0) as usize);
        render::clip_rows(&self.shown, rows, &mut self.clipped);
        if let Some(pipe) = self.pipe {
            let t = (self.pipe_since.elapsed().as_secs_f32() / self.frame_secs).min(1.0);
            let rows = self.pipe_from as f32 + (pipe.rows as f32 - self.pipe_from as f32) * t;
            render::pipe(&mut self.clipped, pipe.x - x, pipe.base - y, (rows * self.scale as f32).round() as i32, self.scale);
        }
        self.shell.present(&self.clipped);
    }

    pub(super) fn stand_home(&mut self) {
        let Some(home) = self.pet.pos.filter(|_| !self.grabbed) else { return };
        let standing = roam::standing(&self.desk, home, self.scale as i32);
        if standing == home {
            return;
        }
        trace::record(|| format!("home {home:?} stands at {standing:?} now"));
        self.pet.pos = Some(standing);
        self.pet.save();
        if self.roam.at_home() {
            self.glide = None;
            self.place(standing);
        }
    }

    fn watch_seat(&mut self) {
        self.shell.watch(self.roam.seat_window());
    }

    pub(super) fn follow_seat(&mut self) {
        // A window being dragged reports its corner at the screen's own pace, faster than
        // the frame snapshot; only when the system will not say is the whole desktop reread.
        let moved = self
            .roam
            .seat_window()
            .and_then(|seat| Some((seat, platform::desktop::window_now(seat)?)))
            .and_then(|(seat, frame)| self.desk.windows.iter_mut().find(|w| w.id == seat).map(|w| w.frame = frame));
        if moved.is_none() {
            self.refresh_desk();
        }
        let Some(spot) = self.roam.seat_spot(&self.desk, self.scale as i32) else { return };
        self.glide = None;
        self.fit_layout(spot);
        self.place(spot);
        self.follow_panel();
    }

    pub(super) fn dropped(&mut self) {
        let here = self.logical_pos();
        let scale = self.scale as i32;
        // No frames ran during the drag, so the desktop snapshot is stale.
        self.refresh_desk();
        let home = roam::standing(&self.desk, here, scale);
        let perch = roam::perch_under(&self.desk, self.own(), here, scale);
        trace::record(|| format!("dropped at {here:?}, home {home:?}, on a window {}", perch.is_some()));
        self.roam.dropped(here, home, scale, perch);
        self.pet.pos = Some(home);
        self.pet.save();
    }

    pub(super) fn check_home(&mut self) {
        let Some((x, y)) = self.pet.pos else { return };
        let (w, h) = self.scene.size();
        self.refresh_desk();
        if self.desk.on_a_monitor(&Rect::new(x, y, x + w as i32, y + h as i32)) {
            self.stand_home();
            return;
        }
        let home = place_window(&self.desk, None, w as i32, h as i32);
        trace::record(|| format!("home {x},{y} is off every monitor, moving to {home:?}"));
        self.pet.pos = Some(home);
        self.pet.save();
        self.roam.grabbed();
        self.glide = None;
        self.fit_layout(home);
        self.place(home);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn he_looks_towards_the_pointer_nearby() {
        let eyes = (1000, 500);
        assert_eq!(glance((800, 520), eyes, 4, false), Some(Idle::LookLeft));
        assert_eq!(glance((1200, 480), eyes, 4, false), Some(Idle::LookRight));
        assert_eq!(glance((1200, 480), eyes, 4, true), Some(Idle::LookLeft), "drawn mirrored");
        assert_eq!(glance((1010, 500), eyes, 4, false), None, "right over his face");
        assert_eq!(glance((2000, 500), eyes, 4, false), None, "too far");
        assert_eq!(glance((900, 1000), eyes, 4, false), None, "far below");
    }
}
