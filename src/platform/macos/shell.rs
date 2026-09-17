use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::super::{Event, WindowId};
use super::appkit::{self, Which};
use super::{desktop, system};
use crate::app::dispatch;
use crate::render::{Canvas, MenuItem};

static QUIT: AtomicBool = AtomicBool::new(false);
static FRAME_MS: AtomicU32 = AtomicU32::new(100);
static POSTED: Mutex<Vec<Event>> = Mutex::new(Vec::new());
static GLIDING: AtomicBool = AtomicBool::new(false);
static SEAT: Mutex<Option<WindowId>> = Mutex::new(None);
// Where he was asked to be, for the moment before there is a window to ask.
static POS: Mutex<(i32, i32)> = Mutex::new((0, 0));

pub fn prepare_process() {
    appkit::start();
    system::hear_the_end();
}

pub fn system_dpi() -> u32 {
    (96.0 * desktop::scale()).round() as u32
}

// One Raccy at a time, by a file beside his memory holding his pid.
pub fn claim_instance(handover: bool) -> bool {
    let path = super::host::app_dir().join("raccy.pid");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    for _ in 0..if handover { 50 } else { 1 } {
        let held = std::fs::read_to_string(&path).ok().and_then(|text| text.trim().parse::<i32>().ok()).is_some_and(alive);
        if !held {
            return std::fs::write(&path, std::process::id().to_string()).is_ok();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn alive(pid: i32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    pid != std::process::id() as i32 && unsafe { kill(pid, 0) } == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shell;

impl Shell {
    pub fn create(x: i32, y: i32, w: i32, h: i32) -> Option<Shell> {
        prepare_process();
        *POS.lock().unwrap_or_else(|e| e.into_inner()) = (x, y);
        appkit::open(Which::Pet, (x, y), (w, h));
        Some(Shell)
    }

    // He is in the window list like anybody else here, so he is asked what
    // name he goes by there rather than making one up.
    pub fn id(&self) -> WindowId {
        WindowId(appkit::number(Which::Pet).unwrap_or(-1))
    }

    pub fn pos(&self) -> (i32, i32) {
        appkit::at(Which::Pet).unwrap_or_else(|| *POS.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn place(&self, x: i32, y: i32) {
        *POS.lock().unwrap_or_else(|e| e.into_inner()) = (x, y);
        appkit::place(Which::Pet, (x, y));
        dispatch(Event::Moved);
    }

    // The canvas says how big he is; the spot is all that is left to set.
    pub fn resize_to(&self, x: i32, y: i32, _w: i32, _h: i32) {
        self.place(x, y);
    }

    pub fn show(&self, shown: bool) {
        appkit::show(Which::Pet, shown);
    }

    pub fn raise(&self) {
        appkit::raise(Which::Pet);
    }

    pub fn present(&self, canvas: &Canvas) {
        appkit::present(Which::Pet, canvas);
    }

    pub fn dpi(&self) -> u32 {
        system_dpi()
    }

    // The system puts the menu up and keeps it; nothing of his own turns
    // while it is there, the same as on Windows.
    pub fn menu(&self, items: &[MenuItem], _scale: usize) -> Option<usize> {
        appkit::menu(items)
    }

    pub fn copy(&self, text: &str, private: bool) -> bool {
        appkit::copy(text, private)
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
        appkit::close(Which::Pet);
    }
}

// A turn of the loop: the end of the session, whatever the system had to say,
// whatever was posted, a glide when one is under way, and a frame when its
// time has come.
fn spin(frame: Duration, next: &mut Instant) {
    if system::ending() && !QUIT.swap(true, Ordering::SeqCst) {
        dispatch(Event::EndSession);
    }
    // A tool asked for the network's name and the system kept it back: the
    // window that puts that right is the main thread's to raise.
    if super::tools::WANTS_LOCATION.swap(false, Ordering::SeqCst)
        && let Some(mtm) = objc2::MainThreadMarker::new()
    {
        super::tools::ask_for_location(mtm);
    }
    let wait = next.saturating_duration_since(Instant::now()).min(frame);
    appkit::pump(if GLIDING.load(Ordering::Relaxed) { wait.min(GLIDE_STEP) } else { wait });
    let posted: Vec<Event> = POSTED.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
    for event in posted {
        dispatch(event);
    }
    if GLIDING.load(Ordering::Relaxed) {
        dispatch(Event::Glide);
    }
    if Instant::now() >= *next {
        *next = Instant::now() + frame;
        dispatch(Event::Frame);
    }
}

// While he is sliding somewhere, the loop turns at the screen's pace rather
// than the frame's.
const GLIDE_STEP: Duration = Duration::from_millis(16);

fn post(event: Event) {
    POSTED.lock().unwrap_or_else(|e| e.into_inner()).push(event);
}

pub struct PanelWindow;

impl PanelWindow {
    pub fn create(width: i32, height: i32) -> Option<PanelWindow> {
        let at = appkit::at(Which::Pet).unwrap_or((0, 0));
        appkit::open(Which::Panel, at, (width, height));
        Some(PanelWindow)
    }

    pub fn place(&self, x: i32, y: i32) {
        appkit::place(Which::Panel, (x, y));
    }

    pub fn present(&self, canvas: &Canvas) {
        appkit::present(Which::Panel, canvas);
    }

    pub fn raise(&self) {
        appkit::raise(Which::Panel);
    }
}

impl Drop for PanelWindow {
    fn drop(&mut self) {
        appkit::close(Which::Panel);
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
