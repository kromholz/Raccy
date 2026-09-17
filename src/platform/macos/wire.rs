use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::command_output;
use crate::watch::wire::Reach;

static LAST: Mutex<Option<(Instant, Reach)>> = Mutex::new(None);

// What the system already knows about its own way out: nothing is sent to
// find out. Asked at most every ten seconds.
pub fn reach() -> Reach {
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, reach)) = *last
        && at.elapsed() < Duration::from_secs(10)
    {
        return reach;
    }
    let reach = look();
    *last = Some((Instant::now(), reach));
    reach
}

// Nothing is sent to find out: this is what the system already knows about
// its own way out. `scutil -r` answers with the flags of the route to an
// address, which says whether there is a way out at all; whether anything is
// at the other end of it, it cannot say, and asking would mean sending
// something.
fn look() -> Reach {
    let Some(text) = command_output("scutil", &["-r", "1.1.1.1"]) else { return Reach::Unknown };
    verdict(read(&text), super::lan::default_gateway().is_some())
}

// A verdict on its own, since "Not Reachable" carries the other word inside
// it and reading for that one first gets every answer backwards.
fn read(text: &str) -> Option<bool> {
    match () {
        _ if text.contains("Not Reachable") => Some(false),
        _ if text.contains("Reachable") => Some(true),
        _ => None,
    }
}

// A way out that answers is the internet; one that does not, with a gateway
// still there, is a network with nothing behind it.
fn verdict(reachable: Option<bool>, gateway: bool) -> Reach {
    match (reachable, gateway) {
        (Some(true), _) => Reach::Internet,
        (Some(false), true) => Reach::Local,
        (Some(false), false) => Reach::Nothing,
        (None, _) => Reach::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_is_read_the_right_way_round() {
        assert_eq!(read("Reachable"), Some(true));
        assert_eq!(read("Not Reachable"), Some(false), "the word for no has the word for yes inside it");
        assert_eq!(read("Reachable,Directly Reachable Address"), Some(true));
        assert_eq!(read("scutil: what?"), None);
        assert_eq!(verdict(Some(false), true), Reach::Local, "a network with no way out of it");
        assert_eq!(verdict(Some(false), false), Reach::Nothing);
        assert_eq!(verdict(None, true), Reach::Unknown);
    }
}
