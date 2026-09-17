// The screens and the windows on them, and the one conversion everything else
// on this system uses between his pixels and the points a Mac counts in.
//
// The window list needs no screen recording right: a Mac keeps back a window's
// title without it, but not who owns it, where it is or how far forward.

use std::sync::atomic::{AtomicU64, Ordering};

use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFNumberType, CFString, CGPoint, CGRect};
use objc2_core_graphics::{
    CGEvent, CGRectMakeWithDictionaryRepresentation, CGWindowID, CGWindowListCopyWindowInfo, CGWindowListOption, kCGWindowAlpha,
    kCGWindowBounds, kCGWindowLayer, kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
};

use objc2_foundation::{NSNumber, NSString};

use super::super::{Desktop, Monitor, MonitorId, Rect, Window, WindowId};

pub fn snapshot() -> Desktop {
    let monitors = monitors();
    let windows = windows();
    // The list comes front to back, so the first thing anybody could be
    // working in is the one in front.
    let foreground = windows.iter().find(|w| !w.overlay).map(|w| w.id);
    Desktop { monitors, windows, foreground, cursor: cursor() }
}

// A Mac lays its screens out in points and draws them in pixels, and the two
// differ by a factor that belongs to each screen. His pixels are the main
// screen's, one space for the whole desktop, so that a spot on it turns into
// a window's corner and back again without drifting. On a desk whose screens
// are of different densities, the far one is measured in the near one's
// pixels.
//
// AppKit is asked rather than Core Graphics, because a Mac with its lid shut
// and nothing plugged in still has a screen to draw on and windows sitting on
// it, and `CGGetActiveDisplayList` says there are none. The screen asked is
// the first one AppKit lists, the one with the menu bar, and not the one it
// calls main: that is whichever holds the keyboard, and a program that never
// takes the keyboard has none. Asked from any thread but the first, AppKit
// says nothing at all, so what it last said is kept: every thread has to
// agree about this or they would place him in two different spots.
pub(super) fn scale() -> f64 {
    static KNOWN: AtomicU64 = AtomicU64::new(0);
    if let Some(mtm) = MainThreadMarker::new()
        && let Some(screen) = NSScreen::screens(mtm).iter().next()
    {
        let scale = screen.backingScaleFactor();
        if scale > 0.0 {
            KNOWN.store(scale.to_bits(), Ordering::Relaxed);
            return scale;
        }
    }
    match KNOWN.load(Ordering::Relaxed) {
        0 => 1.0,
        bits => f64::from_bits(bits),
    }
}

// Core Graphics count downwards from the top left of the main screen; AppKit
// counts upwards from its bottom left. A window is given its corner in his
// pixels, from the top, and its own height to stand on.
pub(super) fn appkit_origin(at: (i32, i32), height_px: f64) -> CGPoint {
    let scale = scale();
    let top = at.1 as f64 / scale;
    CGPoint::new(at.0 as f64 / scale, main_height() - top - height_px / scale)
}

// And back again: the top left corner of a window, in his pixels.
pub(super) fn raccy_pos(frame: CGRect) -> (i32, i32) {
    let scale = scale();
    let top = main_height() - frame.origin.y - frame.size.height;
    ((frame.origin.x * scale).round() as i32, (top * scale).round() as i32)
}

// AppKit measures everything upwards from the bottom of the screen that holds
// the menu bar, which is the first one it lists, whichever one is in use.
fn main_height() -> f64 {
    static KNOWN: AtomicU64 = AtomicU64::new(0);
    if let Some(mtm) = MainThreadMarker::new()
        && let Some(main) = NSScreen::screens(mtm).iter().next()
    {
        let tall = main.frame().size.height;
        if tall > 0.0 {
            KNOWN.store(tall.to_bits(), Ordering::Relaxed);
            return tall;
        }
    }
    f64::from_bits(KNOWN.load(Ordering::Relaxed))
}

// A rectangle Core Graphics gave in points, in his pixels.
fn in_pixels(rect: CGRect, scale: f64) -> Rect {
    let left = (rect.origin.x * scale).round() as i32;
    let top = (rect.origin.y * scale).round() as i32;
    Rect::new(left, top, left + (rect.size.width * scale).round() as i32, top + (rect.size.height * scale).round() as i32)
}

// The screens as AppKit has them, which is the same list even when the lid is
// shut. Its rectangles are the other way up from the window list, so each one
// is turned over before it is measured in his pixels.
fn monitors() -> Vec<Monitor> {
    let Some(mtm) = MainThreadMarker::new() else { return Vec::new() };
    let scale = scale();
    NSScreen::screens(mtm)
        .iter()
        .map(|screen| Monitor {
            id: MonitorId(screen_number(&screen)),
            whole: in_pixels(flipped(screen.frame()), scale),
            // What the menu bar and the dock have left of it, which is where
            // he stands so as not to be walking about underneath them.
            work: in_pixels(flipped(screen.visibleFrame()), scale),
        })
        .collect()
}

// The window list measures downwards from the top; AppKit upwards from the
// bottom. A screen has to be turned over before the two can be compared.
fn flipped(rect: CGRect) -> CGRect {
    CGRect::new(CGPoint::new(rect.origin.x, main_height() - rect.origin.y - rect.size.height), rect.size)
}

// The name Core Graphics knows the screen by, which is what the window list
// and the display notifications use.
fn screen_number(screen: &NSScreen) -> isize {
    let described = screen.deviceDescription();
    let key = NSString::from_str("NSScreenNumber");
    described.objectForKey(&key).and_then(|held| held.downcast::<NSNumber>().ok()).map_or(0, |n| n.integerValue())
}

// Where the pointer is, which a Mac says without being asked for any right.
fn cursor() -> Option<(i32, i32)> {
    let at = CGEvent::location(CGEvent::new(None).as_deref());
    let scale = scale();
    Some(((at.x * scale).round() as i32, (at.y * scale).round() as i32))
}

// Everything on the screen, front to back, as the window server has it.
fn windows() -> Vec<Window> {
    let Some(list) = CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, 0) else { return Vec::new() };
    let scale = scale();
    (0..list.count()).filter_map(|at| entry_at(&list, at)).filter_map(|entry| window_of(entry, scale)).collect()
}

fn entry_at(list: &CFArray, at: isize) -> Option<&CFDictionary> {
    unsafe { (list.value_at_index(at) as *const CFDictionary).as_ref() }
}

fn window_of(entry: &CFDictionary, scale: f64) -> Option<Window> {
    let frame = in_pixels(bounds(entry)?, scale);
    // One nobody can see is one nobody is looking at either.
    if alpha(entry)? <= 0.0 || frame.is_empty() {
        return None;
    }
    // The layer is how far above the ordinary windows a window floats. Zero
    // is where the work happens; above it is chrome, a menu, the dock, a
    // heads-up display, and he among them.
    let layer = number(entry, unsafe { kCGWindowLayer })?;
    Some(Window {
        id: WindowId(number(entry, unsafe { kCGWindowNumber })? as isize),
        pid: number(entry, unsafe { kCGWindowOwnerPID })? as u32,
        frame,
        class: text(entry, unsafe { kCGWindowOwnerName }).unwrap_or_default(),
        // A Mac has no maximise of its own that leaves a bar showing: a window
        // is either the size somebody dragged it to, or it has taken the whole
        // screen and the menu bar with it, and that is full screen.
        maximised: false,
        overlay: layer != 0,
        topmost: layer > 0,
    })
}

fn bounds(entry: &CFDictionary) -> Option<CGRect> {
    let dict: &CFDictionary = unsafe { (entry.value(key(kCGWindowBounds)) as *const CFDictionary).as_ref() }?;
    let mut rect = CGRect::new(CGPoint::new(0.0, 0.0), objc2_core_foundation::CGSize::new(0.0, 0.0));
    unsafe { CGRectMakeWithDictionaryRepresentation(Some(dict), &mut rect) }.then_some(rect)
}

fn key(name: &'static CFString) -> *const std::ffi::c_void {
    (name as *const CFString).cast()
}

fn number(entry: &CFDictionary, name: &'static CFString) -> Option<i64> {
    let number: &CFNumber = unsafe { (entry.value(key(name)) as *const CFNumber).as_ref() }?;
    let mut out: i64 = 0;
    // kCFNumberSInt64Type.
    unsafe { number.value(CFNumberType::SInt64Type, std::ptr::from_mut(&mut out).cast()) }.then_some(out)
}

fn alpha(entry: &CFDictionary) -> Option<f64> {
    let number: &CFNumber = unsafe { (entry.value(key(kCGWindowAlpha)) as *const CFNumber).as_ref() }?;
    let mut out: f64 = 0.0;
    unsafe { number.value(CFNumberType::Float64Type, std::ptr::from_mut(&mut out).cast()) }.then_some(out)
}

fn text(entry: &CFDictionary, name: &'static CFString) -> Option<String> {
    let string: &CFString = unsafe { (entry.value(key(name)) as *const CFString).as_ref() }?;
    Some(string.to_string())
}

// One window, asked after by name rather than by reading the whole list, which
// is what following a window somebody is dragging takes.
pub fn window_now(window: WindowId) -> Option<Rect> {
    let list = CGWindowListCopyWindowInfo(CGWindowListOption::OptionIncludingWindow, window.0 as CGWindowID)?;
    Some(in_pixels(bounds(entry_at(&list, 0)?)?, scale()))
}

// Nothing of his own is a seat, and neither is the desktop itself, the dock,
// the menu bar, or whatever the window server draws for its own reasons.
pub const FURNITURE: &[&str] =
    &["raccy", "Raccy", "Window Server", "Dock", "Spotlight", "Control Center", "Notification Center", "SystemUIServer"];

#[cfg(test)]
mod tests {
    use objc2_core_foundation::CGSize;

    use super::*;

    // Whatever the screen, a corner put into the Mac's way of counting and
    // taken back out again is the corner it started as.
    #[test]
    fn a_corner_survives_the_trip_into_the_macs_counting_and_back() {
        let scale = scale();
        let height = 240.0;
        for at in [(0, 0), (100, 50), (-1920, 300), (2560, -200)] {
            let origin = appkit_origin(at, height);
            let frame = CGRect::new(origin, CGSize::new(64.0, height / scale));
            assert_eq!(raccy_pos(frame), at, "scale {scale}");
        }
    }

    #[test]
    fn a_rectangle_in_points_becomes_one_in_his_pixels() {
        let rect = CGRect::new(CGPoint::new(10.0, 20.0), CGSize::new(100.0, 50.0));
        assert_eq!(in_pixels(rect, 1.0), Rect::new(10, 20, 110, 70));
        assert_eq!(in_pixels(rect, 2.0), Rect::new(20, 40, 220, 140), "on a retina screen");
    }

    #[test]
    #[ignore]
    fn what_this_machine_shows_live() {
        println!("scale {}", scale());
        for m in monitors() {
            println!("{:?} {:?}", m.id, m.whole);
        }
        let desk = snapshot();
        println!("cursor {:?}, in front {:?}, windows {}", desk.cursor, desk.foreground, desk.windows.len());
        for w in desk.windows.iter().take(12) {
            println!("  {:?} pid={} {:?} overlay={} topmost={} class={}", w.id, w.pid, w.frame, w.overlay, w.topmost, w.class);
        }
    }
}
