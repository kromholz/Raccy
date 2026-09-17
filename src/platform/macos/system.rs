use std::sync::atomic::{AtomicBool, Ordering};

use super::super::WindowId;

// `CGEventSourceSecondsSinceLastEventType` with the combined event type: how
// long since anybody touched the keyboard, the mouse or the trackpad.
const ANY_INPUT: u32 = 0xFFFF_FFFF;
const HID_SYSTEM_STATE: u32 = 1;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: u32, event: u32) -> f64;
}

pub fn idle_secs() -> u64 {
    let secs = unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT) };
    if secs.is_finite() && secs > 0.0 { secs as u64 } else { 0 }
}

// Do not disturb, which has been a Focus since Monterey: the old key under
// com.apple.notificationcenterui has not existed for years. What the
// notification centre keeps instead is the assertions it is holding, and one
// of those that nothing has invalidated and whose end has not come is a
// Focus that is on right now.
pub fn hushed() -> bool {
    let path = super::home().join("Library/DoNotDisturb/DB/Assertions.json");
    std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok()).is_some_and(|said| a_focus_is_on(&said, apple_now()))
}

fn a_focus_is_on(said: &serde_json::Value, now: f64) -> bool {
    let store = &said["data"][0];
    let gone: Vec<&str> = store["storeInvalidationRecords"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|record| record["invalidationAssertion"]["assertionUUID"].as_str())
        .collect();
    store["storeAssertionRecords"].as_array().into_iter().flatten().any(|record| {
        let held = record["assertionUUID"].as_str().is_some_and(|id| !gone.contains(&id));
        held && record["assertionDetails"]["assertionDetailsUserVisibleEndDate"].as_f64().is_none_or(|end| end > now)
    })
}

// Apple count their seconds from 2001, not from 1970.
fn apple_now() -> f64 {
    let since_1970 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    since_1970 - 978_307_200.0
}

// Nothing to elevate to here: what he reads, he reads as this user.
pub fn may_elevate() -> bool {
    false
}

pub fn restart_elevated(_owner: WindowId) -> bool {
    false
}

pub fn ask(_question: &str) -> bool {
    false
}

pub fn close_running() {}

pub fn wait_for_predecessor() {}

// Told to go: a logout or a shutdown arrives as a signal, and all the handler
// does is set a flag, which is all a signal handler safely can.
static ENDING: AtomicBool = AtomicBool::new(false);

pub(super) fn ending() -> bool {
    ENDING.load(Ordering::Relaxed)
}

pub(super) fn hear_the_end() {
    unsafe extern "C" fn on_signal(_: i32) {
        ENDING.store(true, Ordering::Relaxed);
    }
    unsafe {
        signal(SIGTERM, on_signal as *const () as usize);
        signal(SIGINT, on_signal as *const () as usize);
        signal(SIGHUP, on_signal as *const () as usize);
    }
}

const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

unsafe extern "C" {
    fn signal(sig: i32, handler: usize) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_focus_that_is_held_and_has_not_run_out_is_a_hush() {
        let held = |uuid: &str, end: &str| {
            format!(r#"{{"data":[{{"storeAssertionRecords":[{{"assertionUUID":"{uuid}","assertionDetails":{{{end}}}}}],"storeInvalidationRecords":[{{"invalidationAssertion":{{"assertionUUID":"OLD"}}}}]}}]}}"#)
        };
        let read = |text: String| a_focus_is_on(&serde_json::from_str(&text).expect("json"), 1000.0);
        assert!(read(held("NOW", r#""assertionDetailsUserVisibleEndDate":2000"#)), "on until later");
        assert!(read(held("NOW", r#""assertionDetailsReason":"schedule""#)), "on with no end named");
        assert!(!read(held("NOW", r#""assertionDetailsUserVisibleEndDate":900"#)), "its end has come and gone");
        assert!(!read(held("OLD", r#""assertionDetailsUserVisibleEndDate":2000"#)), "something invalidated it");
        assert!(!read(r#"{"data":[{"storeAssertionRecords":[]}]}"#.to_string()), "nothing is holding one");
        assert!(!read(r#"{"header":{"version":1}}"#.to_string()), "and a file with nothing in it is no hush");
    }

    #[test]
    #[ignore]
    fn what_this_machine_is_doing_live() {
        println!("idle {}s, hushed {}", idle_secs(), hushed());
    }
}
