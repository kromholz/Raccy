use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crate::watch::Finding;

const LOOK_EVERY: Duration = Duration::from_secs(60);

// The name his own launch agent goes by on macOS, which is where it is
// written and here where it must be recognised as his.
pub const AGENT_LABEL: &str = "systems.bohemia.raccy";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    Run,
    Startup,
    Service,
    Task,
}

pub type Seen = Vec<(Place, Option<HashSet<String>>)>;

pub fn start() -> Receiver<Seen> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("raccy-autoruns".into())
        .spawn(move || {
            while tx.send(crate::platform::autoruns::look()).is_ok() {
                std::thread::sleep(LOOK_EVERY);
            }
        })
        .expect("spawn autostart watch");
    rx
}

pub fn notice(known: &mut HashMap<String, HashSet<String>>, seen: &Seen) -> Vec<Finding> {
    let mut found = Vec::new();
    for (place, entries) in seen {
        let Some(entries) = entries else { continue };
        let entries: HashSet<String> = entries.iter().filter(|name| !his_own(name.as_str())).cloned().collect();
        if let Some(mut before) = known.insert(format!("{place:?}"), entries.clone()) {
            // Task names remembered before they lost their GUIDs are the same tasks.
            if *place == Place::Task {
                before = before.iter().map(|name| tidy(name)).collect();
            }
            let mut new: Vec<&String> = entries.difference(&before).collect();
            new.sort();
            found.extend(new.into_iter().map(|name| Finding::NewAutorun { place: *place, name: name.clone() }));
        }
    }
    found
}

// His own entry is named differently in every place it can be: the Run value
// and the task Raccy on Windows, the file raccy.desktop and the command of an
// exec-once line, /usr/bin/raccy, on Linux, the launch agent
// systems.bohemia.raccy on macOS.
fn his_own(entry: &str) -> bool {
    // A scheduled task of his sits at the top of the tree and is called
    // nothing else. One of that name inside somebody's folder is somebody's,
    // and hiding it would be hiding exactly what this watch is for.
    if let Some(task) = entry.strip_prefix('\\') {
        return task.eq_ignore_ascii_case("Raccy");
    }
    // A crontab line carries a schedule and a command; nothing of his is ever
    // written there.
    if entry.starts_with("cron: ") {
        return false;
    }
    // A launchd job is named backwards, from the domain down, and the last
    // word of such a name is no file extension to be dropped.
    if entry.eq_ignore_ascii_case(AGENT_LABEL) {
        return true;
    }
    // A name, a file name, or the command a compositor starts, arguments and
    // all: `raccy`, `Raccy.lnk`, `raccy.desktop`, `/usr/bin/raccy --after`.
    let first = entry.split_whitespace().next().unwrap_or(entry);
    let last = first.rsplit(['\\', '/']).next().unwrap_or(first);
    let base = last.rsplit_once('.').map_or(last, |(base, _)| base);
    base.eq_ignore_ascii_case("raccy")
}

// Task names from schtasks CSV: the first field of each row, quoted.
#[cfg(windows)]
pub(crate) fn parse_schtasks(csv: &str) -> HashSet<String> {
    csv.lines()
        .filter_map(|line| line.strip_prefix("\"\\")?.split('"').next())
        .map(|name| format!("\\{name}"))
        .filter(|name| !windows_own(name.trim_start_matches('\\').split('\\').next().unwrap_or_default()))
        .map(|name| tidy(&name))
        .collect()
}

// Top-level folders of the scheduler that are Windows' own: \Microsoft,
// and \SoftLanding, the Spotlight and tips tasks that come and go with
// names of their own making.
#[cfg(windows)]
pub(crate) fn windows_own(folder: &str) -> bool {
    folder.eq_ignore_ascii_case("Microsoft") || folder.eq_ignore_ascii_case("SoftLanding")
}

// A task name without the GUID and the long numbers updaters put in
// theirs, so an updater that makes its task anew is the same tenant.
pub(crate) fn tidy(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut digits = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let rest: String = chars.by_ref().take_while(|&c| c != '}').collect();
            if rest.len() == 36 && rest.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                continue;
            }
            out.push('{');
            out.push_str(&rest);
            out.push('}');
            continue;
        }
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        if digits.len() < 8 {
            out.push_str(&digits);
        }
        digits.clear();
        out.push(c);
    }
    if digits.len() < 8 {
        out.push_str(&digits);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> Option<HashSet<String>> {
        Some(names.iter().map(|n| n.to_string()).collect())
    }

    #[test]
    fn a_place_is_learned_quietly_then_new_tenants_are_told() {
        let mut known = HashMap::new();
        assert!(
            notice(&mut known, &vec![(Place::Run, set(&["OneDrive"])), (Place::Startup, set(&["waybar"])), (Place::Task, None)]).is_empty(),
            "learned quietly"
        );
        let found = notice(&mut known, &vec![
            (Place::Run, set(&["OneDrive", "Updater", "Raccy", "raccy.desktop"])),
            (Place::Startup, set(&["waybar", "/home/k/.local/bin/raccy"])),
            (Place::Task, set(&["\\Stranger"])),
        ]);
        assert_eq!(found, vec![Finding::NewAutorun { place: Place::Run, name: "Updater".into() }], "tasks read at last are learned; his own entry is no news on either system");
        assert!(notice(&mut known, &vec![(Place::Run, None), (Place::Task, set(&["\\Stranger", "\\Raccy"]))]).is_empty(), "unreadable keeps what was known");
        let found = notice(&mut known, &vec![(Place::Run, set(&["OneDrive", "Updater"])), (Place::Task, set(&["\\Stranger", "\\Other"]))]);
        assert_eq!(found, vec![Finding::NewAutorun { place: Place::Task, name: "\\Other".into() }]);
        known.insert("Task".into(), ["\\Edge{A1B2C3D4-0000-1111-2222-333344445555}".to_string()].into_iter().collect());
        assert!(notice(&mut known, &vec![(Place::Task, set(&["\\Edge"]))]).is_empty());
    }

    #[test]
    #[cfg(windows)]
    fn schtasks_rows_give_task_names_without_windows_own() {
        let csv = "\"\\Raccy\",\"N/A\",\"Ready\"\r\n\"\\Microsoft\\Windows\\Defrag\\ScheduledDefrag\",\"N/A\",\"Ready\"\r\n\"\\Vendor\\Updater, nightly\",\"16.09.2026 2:00:00\",\"Ready\"\r\n\"\\SoftLanding\\S-1-5-21-1-2-3-1001\\SoftLandingTriggerTask-100000000000000001-render-{11112222-3333-4444-5555-666677778888}\",\"N/A\",\"Ready\"\r\n\r\nFolder: \\Vendor\r\n";
        let names = parse_schtasks(csv);
        assert_eq!(names, ["\\Raccy", "\\Vendor\\Updater, nightly"].into_iter().map(String::from).collect());
    }

    #[test]
    fn he_knows_his_own_entry_in_every_shape_it_takes() {
        for mine in [
            "Raccy",
            "raccy",
            "\\Raccy",
            "raccy.exe",
            "raccy.desktop",
            "/usr/bin/raccy",
            "/home/k/.local/bin/Raccy",
            "/usr/bin/raccy --after 123",
            "raccy --quiet",
            AGENT_LABEL,
            "SYSTEMS.BOHEMIA.RACCY",
        ] {
            assert!(his_own(mine), "{mine}");
        }
        for theirs in [
            "",
            "OneDrive",
            "\\Vendor\\Updater, nightly",
            "raccy-tools",
            "/usr/bin/waybar",
            "notraccy.desktop",
            // Somebody else's, wearing his name in a folder of their own.
            "\\Vendor\\Raccy",
            "\\Raccy\\Updater",
            "cron: @reboot /opt/raccy",
            "systems.bohemia.spiz.build",
        ] {
            assert!(!his_own(theirs), "{theirs}");
        }
    }

    #[test]
    fn a_task_name_loses_its_guid_and_serial() {
        assert_eq!(tidy("\\MicrosoftEdgeUpdateTaskMachineCore{A1B2C3D4-0000-1111-2222-333344445555}"), "\\MicrosoftEdgeUpdateTaskMachineCore");
        assert_eq!(tidy("\\OneDrive Standalone Update Task-S-1-5-21-1111111111-2222222222-3333333333-1001"), "\\OneDrive Standalone Update Task-S-1-5-21----1001");
        assert_eq!(tidy("\\Vendor\\Updater, nightly 2"), "\\Vendor\\Updater, nightly 2");
        assert_eq!(tidy("\\Odd{not a guid}"), "\\Odd{not a guid}");
    }

    #[test]
    #[ignore]
    fn autostart_live() {
        for (place, entries) in crate::platform::autoruns::look() {
            println!("{place:?}: {:?}", entries.map(|e| { let mut e: Vec<_> = e.into_iter().collect(); e.sort(); e }));
        }
    }
}
