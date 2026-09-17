use std::path::PathBuf;

use super::home;

fn entry() -> PathBuf {
    home().join(".config/autostart/raccy.desktop")
}

pub fn installed_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

pub fn enabled() -> bool {
    entry().is_file()
}

pub fn set(on: bool) -> bool {
    let entry = entry();
    if !on {
        let _ = std::fs::remove_file(&entry);
        return !entry.exists();
    }
    let exe = installed_exe();
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=Raccy\nComment=A raccoon that eats your network traffic\nExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    if let Some(dir) = entry.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&entry, text).is_ok()
}

// The Windows verbs, no-ops here: a desktop entry is a file of this user's
// own and needs nobody's permission.
pub fn enable_task(_on: bool) -> bool {
    false
}

pub fn end_task() {}
