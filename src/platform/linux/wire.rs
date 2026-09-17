use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::command_output;
use crate::watch::wire::Reach;

static LAST: Mutex<Option<(Instant, Reach)>> = Mutex::new(None);

pub fn reach() -> Reach {
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, reach)) = *last
        && at.elapsed() < Duration::from_secs(10)
    {
        return reach;
    }
    let reach = match command_output("nmcli", &["-t", "networking", "connectivity"]).as_deref().map(str::trim) {
        Some("full") => Reach::Internet,
        Some("limited") | Some("portal") => Reach::Local,
        Some("none") => Reach::Nothing,
        _ => Reach::Unknown,
    };
    *last = Some((Instant::now(), reach));
    reach
}
