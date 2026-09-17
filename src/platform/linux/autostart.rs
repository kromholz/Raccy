use std::path::PathBuf;

use super::home;

fn entry() -> PathBuf {
    home().join(".config/autostart/raccy.desktop")
}

pub fn installed_exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

// The entry names the program to start, and one built away since starts
// nobody: such an entry is not switched on, whatever the file says.
pub fn enabled() -> bool {
    std::fs::read_to_string(entry()).is_ok_and(|text| {
        text.lines()
            .find_map(|line| line.strip_prefix("Exec="))
            .and_then(|exec| exec.split_whitespace().next())
            .is_some_and(|program| std::path::Path::new(program).is_file())
    })
}

pub fn set(on: bool) -> bool {
    let entry = entry();
    if !on {
        let _ = std::fs::remove_file(&entry);
        return !entry.exists();
    }
    let Some(exe) = installed_exe() else { return false };
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
