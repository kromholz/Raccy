use std::ffi::c_char;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::super::FontFiles;
use super::command_output;
use crate::trace;

// glibc's `struct tm` on the 64-bit targets, with the offset it puts after
// the nine standard fields.
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

// Seconds to add to UTC for local wall-clock time at `unix`, by the time
// zone's rules for that date.
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
    let mut buf = [0u8; 256];
    if unsafe { gethostname(buf.as_mut_ptr() as *mut c_char, buf.len()) } != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).to_lowercase();
    // A machine with no name of its own is called this, which names nothing.
    (!name.is_empty() && name != "localhost").then_some(name)
}

pub fn locale_name() -> String {
    let raw = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())).unwrap_or_default();
    raw.split(['.', '@']).next().unwrap_or("").replace('_', "-").to_lowercase()
}

pub fn font_files() -> FontFiles {
    // fc-match always answers with something: asked for a family by name it
    // may hand back anything at all, so its answer counts only when it is
    // that family. The generic names take whatever they are given.
    let ask = |pattern: &str, by_name: bool| {
        let (families, file) = command_output("fc-match", &["-f", "%{family}\t%{file}", pattern])?.split_once('\t').map(|(f, p)| (f.to_string(), PathBuf::from(p)))?;
        (file.is_file() && (!by_name || same_family(pattern, &families))).then_some(file)
    };
    let mut main: Vec<PathBuf> = ["Cascadia Mono", "Cascadia Code", "JetBrains Mono", "DejaVu Sans Mono"].iter().filter_map(|p| ask(p, true)).collect();
    main.extend(ask("monospace", false));
    let fallback: Vec<PathBuf> = [":lang=ja", ":lang=zh", ":lang=ko"].iter().filter_map(|p| ask(p, false)).collect();
    FontFiles { main, fallback }
}

// fontconfig answers with a comma-separated list of the family's names.
fn same_family(wanted: &str, families: &str) -> bool {
    families.split(',').any(|f| f.trim().eq_ignore_ascii_case(wanted))
}

pub fn restart_after_exit() {
    let exe = std::env::current_exe().unwrap_or_default();
    let spawned = Command::new("sh")
        .arg("-c")
        .arg(format!("sleep 3; exec '{}'", exe.display()))
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(e) = spawned {
        trace::record(|| format!("could not start again: {e}"));
    }
}

pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    base.unwrap_or_else(|| ".".into()).join("raccy")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_font_counts_only_when_it_is_the_family_asked_for() {
        assert!(same_family("DejaVu Sans Mono", "DejaVu Sans Mono,DejaVu Sans Mono Book"));
        assert!(same_family("cascadia mono", "Cascadia Mono"));
        assert!(!same_family("Cascadia Mono", "DejaVu Sans"), "fc-match answers with something either way");
    }
}
