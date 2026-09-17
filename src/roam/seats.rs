use super::*;
use crate::platform::desktop::FURNITURE;

pub(super) const SIT_MIN_WIDTH: i32 = 480;
// Keep clear of the caption buttons on the right.
pub(super) const SIT_RIGHT_CLEARANCE: i32 = 220;
pub(super) const SIT_LEFT_CLEARANCE: i32 = 80;

pub fn fullscreen_app(desk: &Desktop, home: (i32, i32), scale: i32) -> Option<u32> {
    monitor_at(desk, home, scale).and_then(|m| fullscreen_on(desk, m))
}

pub fn home_work_area(desk: &Desktop, pos: (i32, i32), scale: i32) -> Option<Rect> {
    monitor_at(desk, pos, scale).map(|m| m.work)
}

pub(super) fn home_rect(f: &Frame) -> Option<Rect> {
    monitor_under(f, f.home).map(|m| m.whole)
}

pub(super) fn own_work_area(f: &Frame) -> Option<Rect> {
    monitor_under(f, f.here).map(|m| m.work)
}

pub(super) struct Seen {
    pub(super) pid: u32,
    pub(super) covers: bool,
    pub(super) topmost: bool,
}

pub(super) fn fullscreen_on(desk: &Desktop, monitor: &Monitor) -> Option<u32> {
    let whole = monitor.whole;
    let seen = desk.windows.iter().filter_map(|w| {
        // A screenshot tool's dimmed screen and click-through overlays are not what
        // anyone is watching.
        if w.overlay || FURNITURE.contains(&w.class.as_str()) || desk.monitor_of(&w.frame).map(|m| m.id) != Some(monitor.id) {
            return None;
        }
        // Empty, or parked off the monitor, like helper windows at -32000.
        if w.frame.is_empty() || !w.frame.overlaps(&whole) {
            return None;
        }
        // A maximised window is not fullscreen, even filling a monitor with no taskbar.
        Some(Seen { pid: w.pid, covers: !w.maximised && covers(w.frame, whole), topmost: w.topmost })
    });
    fullscreen_among(seen)
}

// Front to back. A topmost window that does not cover the monitor is an overlay, like
// Raccy or a picture in picture; any other window in front means someone is working there.
pub(super) fn fullscreen_among(windows: impl IntoIterator<Item = Seen>) -> Option<u32> {
    for window in windows {
        if window.covers {
            return Some(window.pid);
        }
        if !window.topmost {
            return None;
        }
    }
    None
}

pub(super) fn covers(frame: Rect, monitor: Rect) -> bool {
    frame.left <= monitor.left && frame.top <= monitor.top && frame.right >= monitor.right && frame.bottom >= monitor.bottom
}

// Screen pixels from the window's left to the middle of the sprite.
pub(super) fn middle(scale: i32) -> i32 {
    (SPRITE_X + SIZE / 2) as i32 * scale
}

// Screen pixels from the window's top to the edge he sits on, legs below it.
pub(super) fn seat_line(scale: i32) -> i32 {
    (SPRITE_Y + SIZE - LEGS_BELOW) as i32 * scale
}

// `offset` is screen pixels from the frame's left to the sprite's middle.
pub(super) fn sit_position(frame: Rect, offset: i32, scale: i32) -> (i32, i32) {
    (frame.left + offset - middle(scale), frame.top - seat_line(scale))
}

pub(super) fn sit_offset(width: i32) -> Option<i32> {
    (width >= SIT_MIN_WIDTH).then(|| (width * 3 / 10).clamp(SIT_LEFT_CLEARANCE, width - SIT_RIGHT_CLEARANCE))
}

pub(super) fn seat(f: &Frame) -> Option<(WindowId, i32)> {
    let window = f.desk.foreground.filter(|&w| w != f.own)?;
    let w = f.desk.window(window).filter(|w| !FURNITURE.contains(&w.class.as_str()))?;
    let frame = seat_frame(f.desk, w, f.scale)?;
    let work = f.desk.monitor_of(&w.frame)?.work;
    let width = frame.width();
    // Keep the speech bubble, left of the sprite, on the monitor.
    let offset = sit_offset(width)?.max(work.left - frame.left + middle(f.scale));
    (offset <= width - SIT_RIGHT_CLEARANCE).then_some((window, offset))
}

// His own menu takes the foreground for a moment, which does not count.
pub(super) fn still_seat(seat: Seat, f: &Frame) -> Option<Rect> {
    let chosen = seat.pinned || f.desk.foreground == Some(seat.window) || f.desk.foreground == Some(f.own);
    let stays = seat.pinned || f.may_stay;
    if !(stays && chosen && same_monitor(f, seat.window)) {
        return None;
    }
    f.desk.window(seat.window).and_then(|w| seat_frame(f.desk, w, f.scale))
}

// In sprite pixels, above or below the window's top edge.
pub(super) const PERCH_REACH: i32 = 24;
pub(super) const COVERED_FRAMES: u32 = 20;

pub(super) fn covered(f: &Frame, seat: WindowId, spot: (i32, i32)) -> bool {
    let size = SIZE as i32 * f.scale;
    let (left, top) = (spot.0 + SPRITE_X as i32 * f.scale, spot.1 + SPRITE_Y as i32 * f.scale);
    over(f.desk, seat, f.own, &Rect::new(left, top, left + size, top + size))
}

pub fn buried(desk: &Desktop, own: WindowId, him: &Rect) -> bool {
    over(desk, own, own, him)
}

pub(super) fn over(desk: &Desktop, behind: WindowId, own: WindowId, him: &Rect) -> bool {
    desk.windows
        .iter()
        .take_while(|w| w.id != behind)
        .any(|w| w.id != own && !w.overlay && !FURNITURE.contains(&w.class.as_str()) && w.frame.overlaps(him))
}

pub struct Perch {
    pub(super) seat: Seat,
    pub(super) frame: Rect,
}

pub fn perch_under(desk: &Desktop, own: WindowId, here: (i32, i32), scale: i32) -> Option<Perch> {
    let feet = (here.0 + middle(scale), here.1 + seat_line(scale));
    let mut in_front: Vec<Rect> = Vec::new();
    for w in &desk.windows {
        if w.id == own || w.overlay {
            continue;
        }
        let frame = w.frame;
        let near = (frame.left..frame.right).contains(&feet.0) && (feet.1 - frame.top).abs() <= PERCH_REACH * scale;
        let covered = in_front.iter().any(|r| r.contains((feet.0, frame.top + 1)));
        if near && !covered && !FURNITURE.contains(&w.class.as_str()) {
            let frame = seat_frame(desk, w, scale)?;
            let work = desk.monitor_of(&w.frame)?.work;
            let width = frame.width();
            let offset = (feet.0 - frame.left).clamp(SIT_LEFT_CLEARANCE, width - SIT_RIGHT_CLEARANCE).max(work.left - frame.left + middle(scale));
            let seat = Seat { window: w.id, offset, frames: u32::MAX, pinned: true };
            return (offset <= width - SIT_RIGHT_CLEARANCE).then_some(Perch { seat, frame });
        }
        in_front.push(frame);
    }
    None
}

// Screen pixels of him above the edge he sits on.
pub(super) fn body_above(scale: i32) -> i32 {
    (SIZE - LEGS_BELOW) as i32 * scale
}

pub(super) fn seat_frame(desk: &Desktop, w: &Window, scale: i32) -> Option<Rect> {
    if w.maximised {
        return None;
    }
    let work = desk.monitor_of(&w.frame)?.work;
    let room = w.frame.top - body_above(scale) >= work.top;
    (room && w.frame.width() >= SIT_MIN_WIDTH).then_some(w.frame)
}

pub(super) fn same_monitor(f: &Frame, window: WindowId) -> bool {
    f.desk.monitor_of_window(window).map(|m| m.id) == monitor_under(f, f.here).map(|m| m.id)
}

pub(super) fn seat_report(f: &Frame) -> String {
    let Some(foreground) = f.desk.foreground else { return "roam no seat: nothing in front".into() };
    let Some(w) = f.desk.window(foreground) else { return format!("roam no seat: the window in front does not show ({foreground:?})") };
    let rect = |r: Rect| (r.left, r.top, r.right, r.bottom);
    format!(
        "roam no seat: class={} maximised={} frame={:?} work={:?} seat_line={} ours={} same_monitor={}",
        w.class,
        w.maximised,
        rect(w.frame),
        f.desk.monitor_of(&w.frame).map(|m| rect(m.work)),
        seat_line(f.scale),
        foreground == f.own,
        same_monitor(f, foreground),
    )
}

pub(super) fn cursor_over(f: &Frame) -> bool {
    let Some(p) = f.desk.cursor else { return false };
    let left = f.here.0 + SPRITE_X as i32 * f.scale;
    let top = f.here.1 + SPRITE_Y as i32 * f.scale;
    let size = SIZE as i32 * f.scale;
    Rect::new(left, top, left + size, top + size).contains(p)
}
