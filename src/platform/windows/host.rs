use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime};

use super::super::FontFiles;
use super::system::CREATE_NO_WINDOW;
use crate::trace;

const DETACHED_PROCESS: u32 = 0x0000_0008;
// Seconds from 1601, where FILETIME counts from, to 1970.
const EPOCH_GAP: i64 = 11_644_473_600;

pub fn tz_offset_at(unix: i64) -> i64 {
    unsafe {
        let ticks = (unix + EPOCH_GAP) * 10_000_000;
        let utc_ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        let (mut utc, mut local): (SYSTEMTIME, SYSTEMTIME) = (std::mem::zeroed(), std::mem::zeroed());
        let mut local_ft: FILETIME = std::mem::zeroed();
        let ok = FileTimeToSystemTime(&utc_ft, &mut utc) != 0
            && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) != 0
            && SystemTimeToFileTime(&local, &mut local_ft) != 0;
        if !ok {
            return 0;
        }
        let local_ticks = ((local_ft.dwHighDateTime as i64) << 32) | local_ft.dwLowDateTime as i64;
        (local_ticks - ticks) / 10_000_000
    }
}

pub fn computer_name() -> Option<String> {
    Some(std::env::var("COMPUTERNAME").ok()?.to_lowercase()).filter(|name| !name.is_empty())
}

pub fn locale_name() -> String {
    let mut buf = [0u16; 85];
    let len = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..(len.max(1) - 1) as usize]).to_lowercase()
}

pub fn font_files() -> FontFiles {
    let fonts = PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into())).join("Fonts");
    FontFiles {
        main: ["CascadiaMono.ttf", "consola.ttf", "segoeui.ttf"].iter().map(|f| fonts.join(f)).collect(),
        fallback: ["msgothic.ttc", "YuGothR.ttc"].iter().map(|f| fonts.join(f)).collect(),
    }
}

// The wait is a ping, since timeout wants a console.
pub fn restart_after_exit() {
    let exe = std::env::current_exe().unwrap_or_default();
    // Started as himself, not through the task: the task may be switched off
    // while the program is perfectly well installed, and a run of a task that
    // is off does nothing at all. He is already in the user's session here,
    // so the task buys nothing anyway.
    let start = format!("start \"\" \"{}\"", exe.display());
    // Verbatim: cmd does not read the backslash-escaped quotes Rust would put in.
    let spawned = Command::new("cmd")
        .raw_arg(format!("/c ping -n 4 127.0.0.1 >nul & {start}"))
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(e) = spawned {
        trace::record(|| format!("could not start again: {e}"));
    }
}

pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(std::path::PathBuf::from);
    base.unwrap_or_else(|| ".".into()).join("raccy")
}
