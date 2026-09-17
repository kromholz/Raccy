use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::super::WindowId;
use super::runtime_dir;

// How long the compositor waits before it says nobody is there. Counting
// from the moment it says so gives the seconds since the last input.
pub(super) const IDLE_AFTER_SECS: u64 = 30;

static IDLE_SINCE: Mutex<Option<Instant>> = Mutex::new(None);

pub(super) fn set_idle(idle: bool) {
    let mut since = IDLE_SINCE.lock().unwrap_or_else(|e| e.into_inner());
    *since = idle.then(Instant::now);
}

pub fn idle_secs() -> u64 {
    let since = IDLE_SINCE.lock().unwrap_or_else(|e| e.into_inner());
    since.map_or(0, |since| IDLE_AFTER_SECS + since.elapsed().as_secs())
}

// A Linux desktop keeps do-not-disturb in whichever program shows the
// notifications, so the ones people run are asked in turn and the one that
// answers is the one asked from then on.
pub fn hushed() -> bool {
    static ASKED: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut asked = ASKED.lock().unwrap_or_else(|e| e.into_inner());
    if asked.is_none_or(|(at, _)| at.elapsed() > ASK_EVERY) {
        *asked = Some((Instant::now(), do_not_disturb()));
    }
    asked.is_some_and(|(_, quiet)| quiet)
}

const ASK_EVERY: Duration = Duration::from_secs(5);

const DAEMONS: [(&str, &[&str], &str); 3] = [
    ("dunstctl", &["is-paused"], "true"),
    ("makoctl", &["mode"], "do-not-disturb"),
    ("gsettings", &["get", "org.gnome.desktop.notifications", "show-banners"], "false"),
];

fn do_not_disturb() -> bool {
    static ANSWERS: Mutex<Option<usize>> = Mutex::new(None);
    let mut answers = ANSWERS.lock().unwrap_or_else(|e| e.into_inner());
    let ask = |(program, args, quiet): (&str, &[&str], &str)| super::command_output(program, args).map(|said| said.lines().any(|line| line.trim() == quiet));
    if let Some(known) = *answers {
        if let Some(quiet) = ask(DAEMONS[known]) {
            return quiet;
        }
        *answers = None;
    }
    for (which, daemon) in DAEMONS.into_iter().enumerate() {
        if let Some(quiet) = ask(daemon) {
            *answers = Some(which);
            return quiet;
        }
    }
    false
}

// Nothing to elevate to here: the sampler reads what it may as this user,
// and more would mean asking for a password every start.
pub fn may_elevate() -> bool {
    false
}

static ENDING: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn take_leave(_signal: i32) {
    ENDING.store(true, Ordering::SeqCst);
}

// All the handler does is set a flag, which is all a signal handler safely
// can.
pub(super) fn catch_the_end() {
    let handler: unsafe extern "C" fn(i32) = take_leave;
    for which in [SIGHUP, SIGINT, SIGTERM] {
        unsafe { signal(which, handler as usize) };
    }
}

pub(super) fn ending() -> bool {
    ENDING.load(Ordering::SeqCst)
}

const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

fn pid_file() -> std::path::PathBuf {
    runtime_dir().join("raccy.pid")
}

pub(super) fn running_pid() -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(pid_file()).ok()?.trim().parse().ok()?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    (pid != std::process::id() && cmdline.starts_with(b"/") && String::from_utf8_lossy(&cmdline).contains("raccy")).then_some(pid)
}

pub(super) fn write_pid_file() {
    let _ = std::fs::write(pid_file(), std::process::id().to_string());
}

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn signal(signal: i32, handler: usize) -> usize;
}

pub fn close_running() {
    let Some(pid) = running_pid() else { return };
    unsafe { kill(pid as i32, SIGTERM) };
    for _ in 0..50 {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn wait_for_predecessor() {
    let args: Vec<String> = std::env::args().collect();
    let Some(pid) = args.windows(2).find(|w| w[0] == "--after").and_then(|w| w[1].parse::<u32>().ok()) else {
        return;
    };
    for _ in 0..600 {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// Nothing to elevate to here.
pub fn restart_elevated(_owner: WindowId) -> bool {
    false
}

// Nothing asks here: a package manager takes the program and leaves what is
// under the user's own home alone.
pub fn ask(_question: &str) -> bool {
    false
}
