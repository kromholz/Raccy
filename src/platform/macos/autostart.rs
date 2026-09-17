use std::path::{Path, PathBuf};

use super::{command_output, home};
use crate::watch::autoruns::AGENT_LABEL as LABEL;

fn plist() -> PathBuf {
    home().join("Library/LaunchAgents").join(format!("{LABEL}.plist"))
}

// Where the bundle puts him. A build directory is somewhere he may be run
// from, never somewhere the login agent should be pointed at.
pub fn installed_exe() -> PathBuf {
    PathBuf::from("/Applications/Raccy.app/Contents/MacOS/raccy")
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
    let exe = installed_exe();
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

    // Switches the agent on, asks launchd whether it took it, and puts the
    // machine back the way it was found.
    #[test]
    #[ignore]
    fn the_login_agent_goes_on_and_off_live() {
        let was = enabled();
        assert!(set(true), "switched on");
        assert!(enabled(), "and the file is there");
        let held = command_output("launchctl", &["print", &format!("gui/{}/{LABEL}", unsafe { libc_uid() })]);
        println!("launchd holds it: {}", held.is_some());
        assert!(set(false), "switched off");
        assert!(!enabled(), "and the file is gone");
        if was {
            set(true);
        }
    }

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

    #[test]
    fn the_login_agent_points_at_the_bundle_and_not_at_a_build() {
        let exe = installed_exe();
        assert!(exe.starts_with("/Applications/Raccy.app"), "{}", exe.display());
        assert!(exe.ends_with("Contents/MacOS/raccy"), "{}", exe.display());
    }
}
