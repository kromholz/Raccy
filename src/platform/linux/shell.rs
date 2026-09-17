use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::super::{Event, WindowId};
use super::wayland::{Wayland, Which};
use super::{desktop, system};
use crate::app::dispatch;
use crate::render::{Canvas, MenuItem};
use crate::trace;

// Only the loop's thread touches it.
static WAYLAND: Mutex<Option<Wayland>> = Mutex::new(None);
static POS: Mutex<(i32, i32)> = Mutex::new((0, 0));
static POSTED: Mutex<Vec<Event>> = Mutex::new(Vec::new());
static QUIT: AtomicBool = AtomicBool::new(false);
static FRAME_MS: AtomicU32 = AtomicU32::new(100);
static GLIDING: AtomicBool = AtomicBool::new(false);

fn wayland() -> MutexGuard<'static, Option<Wayland>> {
    WAYLAND.lock().unwrap_or_else(|e| e.into_inner())
}

static SEAT: Mutex<Option<WindowId>> = Mutex::new(None);

pub fn prepare_process() {
    let mut held = wayland();
    if held.is_some() {
        return;
    }
    *held = Wayland::connect();
    drop(held);
    system::catch_the_end();
    desktop::watch_events(|told| match told {
        desktop::Told::Layout => {
            if let Some(seat) = *SEAT.lock().unwrap_or_else(|e| e.into_inner()) {
                post(Event::SeatMoved(seat));
            }
        }
        desktop::Told::Screens => post(Event::DisplayChanged),
    });
}

pub fn system_dpi() -> u32 {
    let scale = desktop::screens().first().map(|s| s.scale).filter(|s| *s > 0.0).unwrap_or(1.0);
    (96.0 * scale).round() as u32
}

pub fn claim_instance(handover: bool) -> bool {
    for attempt in 0.. {
        match system::running_pid() {
            None => {
                system::write_pid_file();
                return true;
            }
            Some(_) if handover && attempt < 50 => std::thread::sleep(Duration::from_millis(200)),
            Some(_) => return false,
        }
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shell;

impl Shell {
    pub fn create(x: i32, y: i32, w: i32, h: i32) -> Option<Shell> {
        prepare_process();
        *POS.lock().unwrap_or_else(|e| e.into_inner()) = (x, y);
        if let Some(wayland) = wayland().as_mut() {
            wayland.state.open(Which::Pet, (x, y), (w, h));
        }
        Some(Shell)
    }

    // His own window, which the compositor does not list among clients.
    pub fn id(&self) -> WindowId {
        WindowId(-1)
    }

    pub fn pos(&self) -> (i32, i32) {
        wayland()
            .as_ref()
            .and_then(|w| w.state.at(Which::Pet))
            .unwrap_or_else(|| *POS.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn place(&self, x: i32, y: i32) {
        *POS.lock().unwrap_or_else(|e| e.into_inner()) = (x, y);
        let moved = match wayland().as_mut() {
            Some(wayland) => {
                wayland.state.place(Which::Pet, (x, y));
                true
            }
            None => true,
        };
        if moved {
            dispatch(Event::Moved);
        }
    }

    // The canvas says how big he is; the spot is all that is left to set.
    pub fn resize_to(&self, x: i32, y: i32, _w: i32, _h: i32) {
        self.place(x, y);
    }

    pub fn show(&self, shown: bool) {
        if let Some(wayland) = wayland().as_mut() {
            wayland.state.show(Which::Pet, shown);
        }
    }

    // The overlay layer is over every window there is: nothing to climb
    // back onto.
    pub fn raise(&self) {}

    pub fn present(&self, canvas: &Canvas) {
        if let Some(wayland) = wayland().as_mut() {
            wayland.state.present(Which::Pet, canvas);
        }
    }

    pub fn dpi(&self) -> u32 {
        wayland().as_mut().and_then(|w| w.state.dpi()).unwrap_or_else(system_dpi)
    }

    // Never call it with the app borrowed, the same as on Windows: the loop's
    // own work carries on here while the menu is up.
    pub fn menu(&self, items: &[MenuItem], scale: usize) -> Option<usize> {
        let opened = wayland().as_mut().is_some_and(|w| w.state.open_menu(items, scale));
        if !opened {
            return None;
        }
        let frame = Duration::from_millis(u64::from(FRAME_MS.load(Ordering::Relaxed).max(20)));
        let mut next = Instant::now() + frame;
        let last_call = Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(wayland) = wayland().as_mut() {
                wayland.state.draw_menu();
            }
            spin(frame, &mut next);
            let done = wayland().as_ref().and_then(|w| w.state.menu_done());
            if done.is_some() || QUIT.load(Ordering::Relaxed) || Instant::now() > last_call {
                break;
            }
        }
        let chosen = wayland().as_mut().and_then(|w| w.state.close_menu());
        trace::record(|| format!("menu closed, chose {chosen:?}"));
        chosen
    }

    pub fn copy(&self, text: &str, private: bool) -> bool {
        use std::io::Write;
        // A Wi-Fi password is given away once: `-o` takes it off the clipboard
        // after one paste. wl-copy can offer one type at a time, so the hint
        // the password managers read cannot go with it without breaking an
        // ordinary paste.
        let mut command = std::process::Command::new("wl-copy");
        if private {
            command.arg("-o");
        }
        let Ok(mut child) = command.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn() else {
            return false;
        };
        let written = child.stdin.take().is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
        child.wait().is_ok() && written
    }

    pub fn watch(&self, window: Option<WindowId>) {
        *SEAT.lock().unwrap_or_else(|e| e.into_inner()) = window;
    }

    pub fn post_autostart_done(&self, on: bool, done: bool) {
        post(Event::AutostartDone { on, done });
    }

    #[cfg(feature = "trace")]
    pub fn post_test_command(&self, id: usize) {
        post(Event::TestCommand(id));
    }

    #[cfg(feature = "trace")]
    pub fn post_menu(&self) {
        post(Event::Menu);
    }

    pub fn run(&self, frame_ms: u32) {
        FRAME_MS.store(frame_ms, Ordering::Relaxed);
        let frame = Duration::from_millis(u64::from(frame_ms));
        let mut next = Instant::now() + frame;
        while !QUIT.load(Ordering::Relaxed) {
            spin(frame, &mut next);
        }
    }

    pub fn quit(&self) {
        QUIT.store(true, Ordering::Relaxed);
    }
}

fn spin(frame: Duration, next: &mut Instant) {
    if system::ending() && !QUIT.swap(true, Ordering::SeqCst) {
        dispatch(Event::EndSession);
    }
    let mut wait = next.saturating_duration_since(Instant::now()).min(frame);
    follow_seat(&mut wait);
    for event in wait_for_compositor(wait) {
        dispatch(event);
    }
    let posted: Vec<Event> = POSTED.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
    for event in posted {
        dispatch(event);
    }
    tend_strip();
    if GLIDING.load(Ordering::Relaxed) && ready_for_frame() {
        dispatch(Event::Glide);
    }
    if Instant::now() >= *next {
        *next = Instant::now() + frame;
        dispatch(Event::Frame);
    }
}

const STILL_MOVING: Duration = Duration::from_millis(700);
const WHILE_MOVING: Duration = Duration::from_millis(16);

static SEAT_AT: Mutex<Option<(crate::platform::Rect, Instant)>> = Mutex::new(None);

// Hyprland says a window has been moved once it lands, not while it is going,
// so its corner is asked for every turn of the loop, and while it keeps
// changing the loop turns at the screen's pace instead of the frame's.
fn follow_seat(wait: &mut Duration) {
    let seat = *SEAT.lock().unwrap_or_else(|e| e.into_inner());
    let mut known = SEAT_AT.lock().unwrap_or_else(|e| e.into_inner());
    let Some(seat) = seat else {
        *known = None;
        return;
    };
    if let Some(frame) = desktop::window_now(seat)
        && known.is_none_or(|(was, _)| was != frame)
    {
        let first = known.is_none();
        *known = Some((frame, Instant::now()));
        if !first {
            post(Event::SeatMoved(seat));
        }
    }
    if known.is_some_and(|(_, moved)| moved.elapsed() < STILL_MOVING) {
        *wait = (*wait).min(WHILE_MOVING);
    }
}

// The connection is let go of before the events reach the app: answering one
// draws him, and that wants it again.
// On a tiled monitor the tiles reach the bottom, and whatever stands there
// stands on somebody's work. He asks the layout for a strip along that edge
// as tall as he is, the way a bar does, and gives it back the moment the
// ground is free again. Asked once a second: the layout does not change
// faster than that, and the asking is a trip to the compositor.
fn tend_strip() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|at| at.elapsed() < Duration::from_secs(1)) {
        return;
    }
    *last = Some(Instant::now());
    drop(last);
    let body = super::wayland::BODY_LOGICAL.load(Ordering::Relaxed);
    let Some((output, _)) = wayland().as_ref().and_then(|w| w.state.pet_output()) else { return };
    let wanted = (body > 0 && desktop::tiles_reach_bottom(&output)).then(|| (output.clone(), body));
    let held = desktop::STRIP.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let same = match (&held, &wanted) {
        (Some((a, ha)), Some((b, hb))) => a == b && (ha - hb).abs() < 8,
        (None, None) => true,
        _ => false,
    };
    if same {
        return;
    }
    let screen = desktop::screens().into_iter().find(|s| s.name == output);
    let mut w = wayland();
    let Some(w) = w.as_mut() else { return };
    match (&wanted, screen) {
        (Some((_, height)), Some(screen)) => {
            w.state.open_strip(&screen, *height);
            crate::trace::record(|| format!("wayland: strip of {height} along the bottom of {output}, the tiles reach it"));
        }
        _ => {
            w.state.close(super::wayland::Which::Strip);
            crate::trace::record(|| format!("wayland: strip on {output} given back"));
        }
    }
    *desktop::STRIP.lock().unwrap_or_else(|e| e.into_inner()) = wanted;
}

fn wait_for_compositor(wait: Duration) -> Vec<Event> {
    let mut held = wayland();
    match held.as_mut() {
        Some(wayland) => {
            let events = wayland.pump(wait);
            drop(held);
            events
        }
        None => {
            drop(held);
            std::thread::sleep(if GLIDING.load(Ordering::Relaxed) { Duration::from_millis(16) } else { wait });
            Vec::new()
        }
    }
}

fn ready_for_frame() -> bool {
    wayland().as_ref().is_none_or(Wayland::ready_for_frame)
}

fn post(event: Event) {
    POSTED.lock().unwrap_or_else(|e| e.into_inner()).push(event);
}

pub struct PanelWindow;

impl PanelWindow {
    pub fn create(width: i32, height: i32) -> Option<PanelWindow> {
        if let Some(wayland) = wayland().as_mut() {
            let at = wayland.state.at(Which::Pet).unwrap_or((0, 0));
            wayland.state.open(Which::Panel, at, (width, height));
        }
        Some(PanelWindow)
    }

    pub fn place(&self, x: i32, y: i32) {
        if let Some(wayland) = wayland().as_mut() {
            wayland.state.place(Which::Panel, (x, y));
        }
    }

    pub fn present(&self, canvas: &Canvas) {
        if let Some(wayland) = wayland().as_mut() {
            wayland.state.present(Which::Panel, canvas);
        }
    }

    pub fn raise(&self) {}
}

impl Drop for PanelWindow {
    fn drop(&mut self) {
        if let Some(wayland) = wayland().as_mut() {
            wayland.state.close(Which::Panel);
        }
    }
}

pub struct Glider;

impl Glider {
    pub fn moving(&self, on: bool) {
        GLIDING.store(on, Ordering::Relaxed);
    }

    pub fn start(_shell: Shell) -> Arc<Glider> {
        Arc::new(Glider)
    }
}
