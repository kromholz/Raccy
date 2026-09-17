use std::collections::HashSet;

use super::{command_output, home};
use crate::watch::autoruns::{Place, Seen};

// Every place, as it is now; a place that cannot be read is None.
pub fn look() -> Seen {
    vec![(Place::Run, login_items()), (Place::Startup, agents()), (Place::Service, daemons()), (Place::Task, loaded())]
}

// What the Dock's own list starts when this user logs in.
fn login_items() -> Option<HashSet<String>> {
    let text = command_output("osascript", &["-e", "tell application \"System Events\" to get the name of every login item"])?;
    Some(text.split(',').map(|name| name.trim().to_string()).filter(|name| !name.is_empty()).collect())
}

// The agents of this user and the ones the machine keeps for everybody.
fn agents() -> Option<HashSet<String>> {
    let dirs = [home().join("Library/LaunchAgents"), "/Library/LaunchAgents".into()];
    plists(&dirs)
}

fn daemons() -> Option<HashSet<String>> {
    plists(&["/Library/LaunchDaemons".into()])
}

fn plists(dirs: &[std::path::PathBuf]) -> Option<HashSet<String>> {
    let mut found = None;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        found
            .get_or_insert_with(HashSet::new)
            .extend(entries.flatten().map(|e| e.file_name().to_string_lossy().trim_end_matches(".plist").to_string()));
    }
    found
}

// What launchd is actually holding, which catches what was loaded without a
// file of its own left behind.
fn loaded() -> Option<HashSet<String>> {
    let text = command_output("launchctl", &["list"])?;
    Some(
        text.lines()
            .skip(1)
            .filter_map(|l| l.split_whitespace().nth(2))
            .filter(|label| !label.starts_with("com.apple."))
            .map(str::to_string)
            .collect(),
    )
}
