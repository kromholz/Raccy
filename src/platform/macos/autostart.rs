use std::path::{Path, PathBuf};

use super::{command_output, home};
use crate::watch::autoruns::AGENT_LABEL as LABEL;

fn plist() -> PathBuf {
    home().join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
}

// The bundle he is running out of, wherever it was dragged to. A build
// directory is somewhere he may be run from, never somewhere the login agent
// should be pointed at, so a program outside a bundle gives nothing.
pub fn installed_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let inside = exe.parent()?.file_name()? == "MacOS" && exe.parent()?.parent()?.file_name()? == "Contents";
    inside.then_some(exe)
}

// The file on its own is not enough: it names the program to start, and a
// build directory that has been cleaned away leaves one pointing at nothing.
// Such an agent starts nobody, so it is not switched on, and saying so lets
// the switch in his menu put it right.
pub fn enabled() -> bool {
    std::fs::read_to_string(plist()).is_ok_and(|text| program_of(&text).is_some_and(|exe| Path::new(exe).is_file()))
}

// The one string inside ProgramArguments.
fn program_of(plist: &str) -> Option<&str> {
    plist.split_once("<key>ProgramArguments</key>")?.1.split_once("<string>")?.1.split_once("</string>").map(|(exe, _)| exe)
}

// A launch agent of this user's own: no administrator anywhere near it.
pub fn set(on: bool) -> bool {
    let plist = plist();
    if !on {
        let _ = command_output("launchctl", &["bootout", &format!("gui/{}/{LABEL}", unsafe { libc_uid() })]);
        let _ = std::fs::remove_file(&plist);
        return !plist.exists();
    }
    let Some(exe) = installed_exe() else { return false };
    let text = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><false/>
  <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#,
        exe.display()
    );
    if let Some(dir) = plist.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(&plist, text).is_err() {
        return false;
    }
    let _ = command_output("launchctl", &["bootstrap", &format!("gui/{}", unsafe { libc_uid() }), &plist.display().to_string()]);
    plist.is_file()
}

unsafe extern "C" {
    #[link_name = "getuid"]
    fn libc_uid() -> u32;
}

// The Windows verbs, no-ops here: a launch agent is a file of this user's own.
pub fn enable_task(_on: bool) -> bool {
    false
}

pub fn end_task() {}

#[cfg(test)]
mod tests {
    use super::*;

    // An agent whose program has been built away starts nobody, so the switch
    // in his menu must not claim it is on.
    #[test]
    fn an_agent_pointing_at_nothing_is_not_switched_on() {
        let plist = |exe: &str| {
            format!("<dict><key>Label</key><string>x</string><key>ProgramArguments</key><array><string>{exe}</string></array></dict>")
        };
        assert_eq!(program_of(&plist("/usr/bin/true")), Some("/usr/bin/true"));
        assert!(Path::new(program_of(&plist("/usr/bin/true")).expect("a program")).is_file());
        assert!(!Path::new(program_of(&plist("/nowhere/raccy")).expect("a program")).is_file());
        assert_eq!(program_of("<dict><key>Label</key><string>x</string></dict>"), None, "nothing to start at all");
    }

    // The test binary is not in a bundle, which is the whole point: a build
    // directory gives nothing to point the agent at.
    #[test]
    fn the_login_agent_takes_a_bundle_and_nothing_else() {
        assert_eq!(installed_exe(), None, "run out of target, so there is no bundle");
        assert!(!set(true), "and nothing to switch on");
    }
}
