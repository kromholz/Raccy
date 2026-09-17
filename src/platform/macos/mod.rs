// The macOS half of the seam. Everything the system is asked for goes through
// the same twelve modules as the other two, with the window itself kept apart
// in `appkit`, the way the Wayland half keeps its own.

mod appkit;
pub mod autoruns;
pub mod autostart;
pub mod desktop;
pub mod host;
pub mod inspect;
pub mod lan;
pub mod net;
pub mod redirects;
pub mod shell;
pub mod system;
pub mod tools;
pub mod wire;

// The programs he asks things of, while they run. Their sockets are his own
// doing, not a program on this machine talking, so the sampler leaves them out.
static HELPERS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn command_output(program: &str, args: &[&str]) -> Option<String> {
    ran(program, args).map(|(said, _)| said)
}

// What a program said on both streams, for the ones that say what they know
// on the error stream even when nothing has gone wrong. codesign is one.
pub(crate) fn command_said(program: &str, args: &[&str]) -> Option<String> {
    ran(program, args).map(|(said, complained)| said + &complained)
}

fn ran(program: &str, args: &[&str]) -> Option<(String, String)> {
    let child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let pid = child.id();
    HELPERS.lock().unwrap_or_else(|e| e.into_inner()).push(pid);
    let out = child.wait_with_output();
    HELPERS.lock().unwrap_or_else(|e| e.into_inner()).retain(|&held| held != pid);
    let out = out.ok()?;
    out.status
        .success()
        .then(|| (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

pub(crate) fn is_helper(pid: u32) -> bool {
    HELPERS.lock().unwrap_or_else(|e| e.into_inner()).contains(&pid)
}

pub(crate) fn home() -> std::path::PathBuf {
    std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("/"))
}
