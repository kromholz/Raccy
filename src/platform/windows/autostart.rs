// The logon task belongs to the package, not to Raccy: the installer writes
// it, elevated, from a definition it lays down beside him in Program Files,
// and Raccy himself never copies a file anywhere nor registers anything. All
// he does here is turn that task on and off, which is a machine-wide change,
// so Windows asks each time.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::{
    IsUserAnAdmin, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

use super::system::{CREATE_NO_WINDOW, wide};

const TASK: &str = "Raccy";

// Where the package puts him.
#[cfg_attr(not(test), allow(dead_code))]
pub fn installed_exe() -> Option<PathBuf> {
    let base = std::env::var_os("ProgramW6432").or_else(|| std::env::var_os("ProgramFiles")).map(PathBuf::from);
    Some(base.unwrap_or_else(|| PathBuf::from(r"C:\Program Files")).join("Raccy").join("raccy.exe"))
}

pub fn enabled() -> bool {
    task_enabled() == Some(true)
}

// False if it did not happen, for example a declined prompt. With no task
// there is nothing to turn on: only the package puts one there.
pub fn set(on: bool) -> bool {
    match task_enabled() {
        None => false,
        Some(already) if already == on => true,
        Some(_) => match unsafe { IsUserAnAdmin() } != 0 {
            true => enable_task(on),
            false => elevated(if on { "--enable-task" } else { "--disable-task" }),
        },
    }
}

// Run elevated: the task is machine-wide.
pub fn enable_task(on: bool) -> bool {
    let switch = if on { "/ENABLE" } else { "/DISABLE" };
    Command::new("schtasks.exe")
        .args(["/Change", "/TN", TASK, switch])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

// Works without admin, and does nothing when the task is not running.
pub fn end_task() {
    let _ = Command::new("schtasks.exe")
        .args(["/End", "/TN", TASK])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// Whether the task is there, and whether it is switched on. Asked for as XML,
// which reads the same in every language where the table form is translated.
fn task_enabled() -> Option<bool> {
    let out = Command::new("schtasks.exe")
        .args(["/Query", "/TN", TASK, "/XML", "ONE"])
        .creation_flags(CREATE_NO_WINDOW)
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    Some(!settings_of(&text_of(&out.stdout))?.contains("<Enabled>false</Enabled>"))
}

// What schtasks writes to a pipe is single bytes, whatever the XML
// declaration inside it says about UTF-16; redirected to a file by some
// shells it comes out as UTF-16 with a mark. The mark is what tells them
// apart, and nothing else does.
fn text_of(bytes: &[u8]) -> String {
    match bytes.starts_with(&[0xFF, 0xFE]) {
        false => String::from_utf8_lossy(bytes).into_owned(),
        true => {
            let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
            String::from_utf16_lossy(&units)
        }
    }
}

// The settings of a task, where its own switch is. A trigger has a switch of
// the same name, so the two must not be read as one.
fn settings_of(xml: &str) -> Option<&str> {
    let after = xml.split_once("<Settings>")?.1;
    Some(after.split_once("</Settings>").map_or(after, |(inside, _)| inside))
}

fn elevated(args: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    unsafe {
        let (verb, file, params) = (wide("runas"), wide(&exe.display().to_string()), wide(args));
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 || info.hProcess.is_null() {
            return false;
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        code == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn he_is_looked_for_where_the_package_puts_him() {
        let exe = installed_exe().expect("a place he is installed");
        assert!(exe.ends_with(r"Raccy\raccy.exe"), "{}", exe.display());
        let program_files = std::env::var_os("ProgramW6432").or_else(|| std::env::var_os("ProgramFiles")).map(PathBuf::from);
        assert!(program_files.is_none_or(|p| exe.starts_with(p)), "{}", exe.display());
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        assert!(local.is_none_or(|p| !exe.starts_with(p)), "nothing the user can write: {}", exe.display());
    }

    // The switch in the settings is the task's own; the one in the trigger
    // says whether that trigger fires and is not the answer.
    #[test]
    fn a_switched_off_task_is_told_from_a_switched_on_one() {
        let task = |settings: &str| {
            format!("<Task><Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers><Settings>{settings}</Settings></Task>")
        };
        let on = task("<Priority>7</Priority><Enabled>true</Enabled>");
        let off = task("<Priority>7</Priority><Enabled>false</Enabled>");
        assert!(!settings_of(&on).expect("settings").contains("<Enabled>false</Enabled>"));
        assert!(settings_of(&off).expect("settings").contains("<Enabled>false</Enabled>"));
        assert_eq!(settings_of("<Task><Triggers/></Task>"), None);
    }

    // What schtasks really writes to a pipe on this machine: single bytes,
    // and an XML declaration that says UTF-16 while being nothing of the
    // sort. Reading it as UTF-16 turned every answer into nonsense and left
    // the switch in the menu saying no to everything.
    #[test]
    fn the_answer_is_read_by_its_mark_and_not_by_what_it_claims() {
        let says = br#"<?xml version="1.0" encoding="UTF-16"?><Task><Settings><Enabled>false</Enabled></Settings></Task>"#;
        let read = text_of(says);
        assert!(read.starts_with("<?xml"), "{read}");
        assert_eq!(settings_of(&read), Some("<Enabled>false</Enabled>"));

        let mut marked = vec![0xFF, 0xFE];
        for unit in "<Task><Settings>x</Settings></Task>".encode_utf16() {
            marked.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(settings_of(&text_of(&marked)), Some("x"), "and the marked form still reads");
    }

    #[test]
    #[ignore]
    fn the_logon_task_says_whether_it_is_on() {
        println!("installed at {:?}", installed_exe());
        println!("task enabled: {:?}", task_enabled());
    }
}
