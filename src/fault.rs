use std::path::PathBuf;

use crate::{clock, platform, trace};

const RESTARTS: usize = 3;

fn crash_path() -> PathBuf {
    crate::platform::host::app_dir().join("crash.log")
}

pub fn install_hook() {
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("ui").to_string();
        let at = info.location().map_or_else(|| "?".to_string(), |l| format!("{}:{}", l.file(), l.line()));
        let what = info.payload_as_str().unwrap_or("panic").replace(['\r', '\n'], " ");
        let text = format!("panic in {name} at {at}: {what}");
        let before = std::fs::read_to_string(crash_path()).map_or(0, |t| t.lines().filter(|l| !l.trim().is_empty()).count());
        let again = before + 1 < RESTARTS;
        trace::record(|| format!("{text}; {}", if again { "starting again" } else { "staying down" }));
        let line = format!("{} {text}\n", clock::record_stamp(clock::unix_now()));
        let path = crash_path();
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            use std::io::Write;
            let _ = f.write_all(line.as_bytes());
        }
        if again {
            platform::host::restart_after_exit();
        }
        std::process::exit(101);
    }));
}

pub fn crashes() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(crash_path()) else { return Vec::new() };
    text.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()
}

pub fn forgive() {
    let _ = std::fs::remove_file(crash_path());
}

pub fn place(crash_line: &str) -> String {
    let at: Option<String> = crash_line.split(" at ").nth(1).map(|rest| rest.split(':').take(2).collect::<Vec<_>>().join(":"));
    at.map(|at| at.rsplit(['\\', '/']).next().unwrap_or(&at).to_string()).unwrap_or_else(|| "?".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crash_line_names_its_place_short() {
        assert_eq!(place(r"16.09. 08:43:21 panic in raccy-net at src\net.rs:123: index out of bounds"), "net.rs:123");
        assert_eq!(place("16.09. 08:43:21 panic in ui at src/app.rs:7: x"), "app.rs:7");
        assert_eq!(place("garbage"), "?");
    }
}
