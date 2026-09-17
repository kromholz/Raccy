use std::collections::HashSet;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

use super::system::{CREATE_NO_WINDOW, RUN_KEY, RegKey};
use crate::watch::autoruns::{Place, Seen, parse_schtasks, tidy, windows_own};

const RUN_ONCE: &str = r"Software\Microsoft\Windows\CurrentVersion\RunOnce";
const WOW_RUN: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const SERVICES: &str = r"SYSTEM\CurrentControlSet\Services";
const TASKS: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Schedule\TaskCache\Tree";
// A service that Windows starts on its own at boot.
const AUTO_START: u32 = 2;
// Services of their own process or a shared one; not drivers, and not the
// per-user template services Windows makes copies of.
const WIN32_SERVICE: u32 = 0x10 | 0x20;
const USER_SERVICE: u32 = 0x40 | 0x80;

// A place that cannot be read is None.
pub fn look() -> Seen {
    vec![(Place::Run, run_entries()), (Place::Startup, startup_entries()), (Place::Service, services()), (Place::Task, tasks())]
}

fn run_entries() -> Option<HashSet<String>> {
    let keys = [(HKEY_CURRENT_USER, RUN_KEY), (HKEY_CURRENT_USER, RUN_ONCE), (HKEY_LOCAL_MACHINE, RUN_KEY), (HKEY_LOCAL_MACHINE, RUN_ONCE), (HKEY_LOCAL_MACHINE, WOW_RUN)];
    let mut found = None;
    for (root, path) in keys {
        if let Some(key) = RegKey::open(root, path) {
            found.get_or_insert_with(HashSet::new).extend(key.values());
        }
    }
    found
}

fn startup_entries() -> Option<HashSet<String>> {
    let folders = [("APPDATA", r"Microsoft\Windows\Start Menu\Programs\Startup"), ("ProgramData", r"Microsoft\Windows\Start Menu\Programs\StartUp")];
    let mut found = None;
    for (root, path) in folders {
        let Some(dir) = std::env::var_os(root).map(|root| PathBuf::from(root).join(path)) else { continue };
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        found
            .get_or_insert_with(HashSet::new)
            .extend(entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|name| !name.eq_ignore_ascii_case("desktop.ini")));
    }
    found
}

fn services() -> Option<HashSet<String>> {
    let root = RegKey::open(HKEY_LOCAL_MACHINE, SERVICES)?;
    let started = |name: &String| {
        let Some(service) = RegKey::open(root.0, name) else { return false };
        let kind = service.dword("Type").unwrap_or(0);
        service.dword("Start") == Some(AUTO_START) && kind & WIN32_SERVICE != 0 && kind & USER_SERVICE == 0
    };
    Some(root.subkeys().into_iter().filter(started).collect())
}

// The scheduled tasks: from the scheduler's tree in the registry, which an
// administrator can read, and otherwise as schtasks lists them to this
// user. Windows' own, under \Microsoft, come and go with its updates.
fn tasks() -> Option<HashSet<String>> {
    if let Some(tree) = RegKey::open(HKEY_LOCAL_MACHINE, TASKS) {
        let mut found = HashSet::new();
        walk(&tree, "", &mut found);
        return Some(found);
    }
    let listing = Command::new("schtasks.exe")
        .args(["/Query", "/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    Some(parse_schtasks(&String::from_utf8_lossy(&listing.stdout)))
}

fn walk(folder: &RegKey, path: &str, found: &mut HashSet<String>) {
    for name in folder.subkeys() {
        if path.is_empty() && windows_own(&name) {
            continue;
        }
        let Some(child) = RegKey::open(folder.0, &name) else { continue };
        let full = format!("{path}\\{name}");
        if child.has("Id") {
            found.insert(tidy(&full));
        }
        walk(&child, &full, found);
    }
}
