use std::collections::HashSet;
use std::path::Path;

use super::{command_output, home};
use crate::watch::autoruns::{Place, Seen};

// A place that cannot be read is None.
pub fn look() -> Seen {
    vec![(Place::Run, xdg_autostart()), (Place::Startup, exec_lines()), (Place::Service, services()), (Place::Task, tasks())]
}

fn xdg_autostart() -> Option<HashSet<String>> {
    let dirs = [home().join(".config/autostart"), Path::new("/etc/xdg/autostart").to_path_buf()];
    let mut found = None;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        found.get_or_insert_with(HashSet::new).extend(entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()));
    }
    found
}

fn exec_lines() -> Option<HashSet<String>> {
    let text = std::fs::read_to_string(home().join(".config/hypr/hyprland.conf")).ok()?;
    Some(exec_lines_in(&text))
}

// The commands a compositor config starts: `exec` for every reload of it
// and `exec-once` for the session, either side of the equals sign spaced
// as the writer pleased.
fn exec_lines_in(text: &str) -> HashSet<String> {
    text.lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("exec-once").or_else(|| line.strip_prefix("exec")))
        .filter_map(|rest| rest.trim_start().strip_prefix('='))
        .map(|cmd| cmd.trim().to_string())
        .filter(|cmd| !cmd.is_empty())
        .collect()
}

fn enabled_units(kind: &str, user: bool) -> Option<HashSet<String>> {
    let mut args = vec!["list-unit-files", "--type", kind, "--state", "enabled", "--no-legend", "--plain"];
    if user {
        args.insert(0, "--user");
    }
    let out = command_output("systemctl", &args)?;
    Some(out.lines().filter_map(|l| l.split_whitespace().next()).map(|s| s.to_string()).collect())
}

fn services() -> Option<HashSet<String>> {
    let mut found = enabled_units("service", false)?;
    found.extend(enabled_units("service", true).unwrap_or_default());
    Some(found)
}

fn tasks() -> Option<HashSet<String>> {
    let mut found = enabled_units("timer", false)?;
    found.extend(enabled_units("timer", true).unwrap_or_default());
    if let Some(cron) = command_output("crontab", &["-l"]) {
        found.extend(cron.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(|l| format!("cron: {l}")));
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_starts_what_its_exec_lines_say() {
        let config = "\
# what comes up with the session
exec-once = waybar
exec-once=raccy
  exec = hyprpaper
exec-shutdown = tidy up
execute = not a thing
exec =
monitor = ,preferred,auto,1
";
        let starts = exec_lines_in(config);
        assert_eq!(starts.len(), 3, "{starts:?}");
        for command in ["waybar", "raccy", "hyprpaper"] {
            assert!(starts.contains(command), "{command} is missing from {starts:?}");
        }
        assert!(exec_lines_in("").is_empty());
    }
}
