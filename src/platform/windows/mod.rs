pub mod autoruns;
pub mod autostart;
pub mod desktop;
mod etw;
pub mod host;
pub mod inspect;
pub mod lan;
pub mod net;
pub mod redirects;
pub mod shell;
pub mod system;
pub mod tools;
pub mod wire;

// The programs he asks things of, while they run: fetching an update, handing
// it to the installer. Their connections are his own doing, not a program on
// this machine talking, so the sampler leaves them out.
static HELPERS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

pub(super) fn as_helper<T>(child: &mut std::process::Child, wait: impl FnOnce(&mut std::process::Child) -> T) -> T {
    let pid = child.id();
    HELPERS.lock().unwrap_or_else(|e| e.into_inner()).push(pid);
    let out = wait(child);
    HELPERS.lock().unwrap_or_else(|e| e.into_inner()).retain(|&held| held != pid);
    out
}

pub(super) fn keep_as_helper(pid: u32) {
    HELPERS.lock().unwrap_or_else(|e| e.into_inner()).push(pid);
}

pub(super) fn is_helper(pid: u32) -> bool {
    HELPERS.lock().unwrap_or_else(|e| e.into_inner()).contains(&pid)
}
