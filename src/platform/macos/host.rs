use std::ffi::c_char;
use std::path::PathBuf;

use super::super::FontFiles;
use super::{command_output, home};

// The same `struct tm` as on Linux: macOS carry the offset after the nine
// standard fields too.
#[repr(C)]
struct Tm {
    sec: i32,
    min: i32,
    hour: i32,
    mday: i32,
    mon: i32,
    year: i32,
    wday: i32,
    yday: i32,
    isdst: i32,
    gmtoff: i64,
    zone: *const c_char,
}

unsafe extern "C" {
    fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
    fn tzset();
    fn gethostname(name: *mut c_char, len: usize) -> i32;
}

pub fn tz_offset_at(unix: i64) -> i64 {
    let mut tm = Tm { sec: 0, min: 0, hour: 0, mday: 0, mon: 0, year: 0, wday: 0, yday: 0, isdst: 0, gmtoff: 0, zone: std::ptr::null() };
    unsafe {
        tzset();
        if localtime_r(&unix, &mut tm).is_null() {
            return 0;
        }
    }
    tm.gmtoff
}

pub fn computer_name() -> Option<String> {
    // The name people give a Mac has spaces and accents in it; the host name
    // is the one other machines see, and is what the other systems give here.
    let mut buf = [0u8; 256];
    if unsafe { gethostname(buf.as_mut_ptr() as *mut c_char, buf.len()) } != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).to_lowercase();
    let name = name.trim_end_matches(".local").to_string();
    (!name.is_empty() && name != "localhost").then_some(name)
}

pub fn locale_name() -> String {
    // `cs_CZ@calendar=gregorian` from the defaults, or the environment when
    // nobody has set one.
    let raw = command_output("defaults", &["read", "-g", "AppleLocale"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("LANG").ok())
        .unwrap_or_default();
    raw.split(['.', '@']).next().unwrap_or("").replace('_', "-").to_lowercase()
}

pub fn font_files() -> FontFiles {
    let system = PathBuf::from("/System/Library/Fonts");
    let supplemental = system.join("Supplemental");
    let user = home().join("Library/Fonts");
    let main = ["SFNSMono.ttf", "Menlo.ttc", "Monaco.ttf", "Courier.ttc"]
        .iter()
        .flat_map(|f| [system.join(f), supplemental.join(f), user.join(f)])
        .filter(|p| p.is_file())
        .collect();
    let fallback = ["Apple Symbols.ttf", "AppleSDGothicNeo.ttc", "PingFang.ttc", "Hiragino Sans GB.ttc"]
        .iter()
        .flat_map(|f| [system.join(f), supplemental.join(f)])
        .filter(|p| p.is_file())
        .collect();
    FontFiles { main, fallback }
}

pub fn restart_after_exit() {
    let exe = std::env::current_exe().unwrap_or_default();
    let spawned = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("sleep 3; exec \"{}\"", exe.display()))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = spawned {
        crate::trace::record(|| format!("could not start again: {e}"));
    }
}

// Where a Mac keeps what a program remembers about itself.
pub fn app_dir() -> PathBuf {
    home().join("Library/Application Support/Raccy")
}
