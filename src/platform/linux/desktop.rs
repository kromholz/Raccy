// Hyprland counts in the layout's logical pixels, which are the screen's own
// pixels divided by the monitor's scale. Everything read here is turned into
// screen pixels on the way in, and the window is placed in logical ones again
// on the way out.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::super::{Desktop, Monitor, MonitorId, Rect, Window, WindowId};
use super::runtime_dir;

// Started from an SSH session there is no instance in the environment, and
// the newest one running is taken instead.
fn instance() -> Option<String> {
    std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok().or_else(|| {
        let dir = runtime_dir().join("hypr");
        let mut instances: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.file_name()))).collect();
        instances.sort();
        instances.pop().map(|(_, name)| name.to_string_lossy().into_owned())
    })
}

pub(super) fn hyprctl(request: &str) -> Option<String> {
    let signature = instance()?;
    let path = runtime_dir().join("hypr").join(signature).join(".socket.sock");
    let mut stream = UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    stream.write_all(request.as_bytes()).ok()?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok()?;
    Some(answer)
}

fn json(request: &str) -> Option<Value> {
    serde_json::from_str(&hyprctl(request)?).ok()
}

// A window's address, `0x55d1c0ffee`, as its id.
pub(super) fn window_id(address: &str) -> Option<WindowId> {
    isize::from_str_radix(address.trim_start_matches("0x"), 16).ok().map(WindowId)
}

fn int(v: &Value) -> i32 {
    v.as_i64().or_else(|| v.as_f64().map(|f| f.round() as i64)).unwrap_or(0) as i32
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Screen {
    pub id: i64,
    // The compositor's name for it, `HEADLESS-2` or `DP-1`, which is also
    // the name its Wayland output answers to.
    pub name: String,
    pub logical: Rect,
    pub device: Rect,
    pub work: Rect,
    pub scale: f64,
    pub workspace: i64,
}

pub(super) fn screens() -> Vec<Screen> {
    json("j/monitors").as_ref().map(screens_of).unwrap_or_default()
}

fn recent_screens() -> Vec<Screen> {
    static KNOWN: Mutex<Option<(Instant, Vec<Screen>)>> = Mutex::new(None);
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if known.as_ref().is_none_or(|(at, _)| at.elapsed() > Duration::from_millis(500)) {
        let screens = screens();
        if !screens.is_empty() {
            *known = Some((Instant::now(), screens));
        }
    }
    known.as_ref().map(|(_, screens)| screens.clone()).unwrap_or_default()
}

fn screens_of(monitors: &Value) -> Vec<Screen> {
    let mut screens: Vec<Screen> = monitors
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            let scale = m["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0);
            let (x, y) = (int(&m["x"]), int(&m["y"]));
            let (pixels_w, pixels_h) = (int(&m["width"]), int(&m["height"]));
            let (w, h) = ((pixels_w as f64 / scale).round() as i32, (pixels_h as f64 / scale).round() as i32);
            Screen {
                id: m["id"].as_i64().unwrap_or(0),
                name: m["name"].as_str().unwrap_or("").to_string(),
                logical: Rect::new(x, y, x + w, y + h),
                device: Rect::new(0, 0, pixels_w, pixels_h),
                work: Rect::new(0, 0, 0, 0),
                scale,
                workspace: m["activeWorkspace"]["id"].as_i64().unwrap_or(-1),
            }
        })
        .collect();

    let xs = spans(screens.iter().map(|s| (s.logical.left, s.logical.right, s.scale)).collect());
    let ys = spans(screens.iter().map(|s| (s.logical.top, s.logical.bottom, s.scale)).collect());
    for (s, m) in screens.iter_mut().zip(monitors.as_array().into_iter().flatten()) {
        let (dx, dy) = (along(&xs, s.logical.left), along(&ys, s.logical.top));
        s.device = Rect::new(dx, dy, dx + s.device.width(), dy + s.device.height());
        // What a bar has taken, in logical pixels along each edge.
        let reserved: Vec<i32> = m["reserved"].as_array().into_iter().flatten().map(int).collect();
        let scaled = |n: &i32| (*n as f64 * s.scale).round() as i32;
        s.work = match reserved.as_slice() {
            [l, t, r, b] => Rect::new(s.device.left + scaled(l), s.device.top + scaled(t), s.device.right - scaled(r), s.device.bottom - scaled(b)),
            _ => s.device,
        };
    }
    screens
}

// One monitor's logical stretch along an axis, with the scale that holds
// there, sorted and butted up against each other: a stretch no monitor covers
// counts at a scale of one, and where two of them overlap the first wins.
fn spans(mut stretches: Vec<(i32, i32, f64)>) -> Vec<(i32, i32, f64)> {
    stretches.sort_by_key(|s| s.0);
    let mut out: Vec<(i32, i32, f64)> = Vec::new();
    let mut at = stretches.first().map_or(0, |s| s.0);
    for (from, to, scale) in stretches {
        if to <= at {
            continue;
        }
        if from > at {
            out.push((at, from, 1.0));
            at = from;
        }
        out.push((at, to, scale));
        at = to;
    }
    out
}

// The screen pixel a logical coordinate falls on. Each monitor takes as many
// pixels as it has, so two of them at different scales neither overlap nor
// leave a gap, which computing each monitor's corner from its own scale does.
fn along(spans: &[(i32, i32, f64)], at: i32) -> i32 {
    let mut out = 0.0;
    for &(from, to, scale) in spans {
        if at <= from {
            break;
        }
        out += (at.min(to) - from) as f64 * scale;
    }
    out.round() as i32
}

// The monitor a point is on, or failing that the nearest one, measured in
// whichever of the two coordinate systems the caller is counting in.
fn screen_by(screens: &[Screen], of: fn(&Screen) -> Rect, (x, y): (i32, i32)) -> Option<&Screen> {
    screens.iter().find(|s| of(s).contains((x, y))).or_else(|| {
        screens.iter().min_by_key(|s| {
            let (cx, cy) = of(s).centre();
            ((cx - x) as i64).pow(2) + ((cy - y) as i64).pow(2)
        })
    })
}

pub(super) fn screen_at(screens: &[Screen], at: (i32, i32)) -> Option<&Screen> {
    screen_by(screens, |s| s.logical, at)
}

pub(super) fn screen_of_device(screens: &[Screen], at: (i32, i32)) -> Option<&Screen> {
    screen_by(screens, |s| s.device, at)
}

pub(super) fn to_device(screens: &[Screen], (x, y): (i32, i32)) -> (i32, i32) {
    match screen_at(screens, (x, y)) {
        Some(s) => (
            s.device.left + ((x - s.logical.left) as f64 * s.scale).round() as i32,
            s.device.top + ((y - s.logical.top) as f64 * s.scale).round() as i32,
        ),
        None => (x, y),
    }
}

pub(super) fn cursor(screens: &[Screen]) -> Option<(i32, i32)> {
    let at = json("j/cursorpos")?;
    Some(to_device(screens, (int(at.get("x")?), int(at.get("y")?))))
}

pub fn snapshot() -> Desktop {
    let screens = screens();
    let clients = json("j/clients").unwrap_or(Value::Null);
    let active = json("j/activewindow").unwrap_or(Value::Null);

    let mut desk = Desktop { cursor: cursor(&screens), ..Desktop::default() };
    desk.monitors = screens.iter().map(|s| Monitor { id: MonitorId(s.id as isize), whole: s.device, work: s.work }).collect();
    // Only the workspace each monitor shows is on the screen.
    let shown: Vec<(i64, i64)> = screens.iter().map(|s| (s.id, s.workspace)).collect();
    let mut windows: Vec<(bool, i64, Window)> = clients
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["mapped"].as_bool().unwrap_or(false) && !c["hidden"].as_bool().unwrap_or(false))
        .filter(|c| {
            let (monitor, workspace) = (c["monitor"].as_i64().unwrap_or(-1), c["workspace"]["id"].as_i64().unwrap_or(-2));
            shown.contains(&(monitor, workspace))
        })
        .filter_map(|c| {
            let id = window_id(c["address"].as_str()?)?;
            let at = c["at"].as_array()?;
            let size = c["size"].as_array()?;
            let (x, y) = (int(&at[0]), int(&at[1]));
            let (left, top) = to_device(&screens, (x, y));
            let (right, bottom) = to_device(&screens, (x + int(&size[0]), y + int(&size[1])));
            let fullscreen = c["fullscreen"].as_i64().unwrap_or(0);
            let window = Window {
                id,
                pid: c["pid"].as_i64().unwrap_or(0) as u32,
                frame: Rect::new(left, top, right, bottom),
                class: c["class"].as_str().unwrap_or("").to_string(),
                // Hyprland's maximise keeps the bar; its fullscreen covers the monitor.
                maximised: fullscreen == 1,
                // Nothing anybody works in: a menu, a tooltip or an on-screen
                // display, which is what a window with no class of its own is
                // here. A Wayland popup is not a client at all and never
                // reaches this list.
                overlay: c["class"].as_str().unwrap_or("").is_empty(),
                topmost: c["pinned"].as_bool().unwrap_or(false),
            };
            Some((c["floating"].as_bool().unwrap_or(false), c["focusHistoryID"].as_i64().unwrap_or(i64::MAX), window))
        })
        .collect();
    // Front to back: floating windows over tiled ones, the last focused first.
    windows.sort_by_key(|(floating, focus, _)| (!floating, *focus));
    desk.windows = windows.into_iter().map(|(_, _, w)| w).collect();
    desk.foreground = active.get("address").and_then(Value::as_str).and_then(window_id);
    desk
}

// Hyprland is only asked about the window in front, which while a window is
// being dragged is that window; anything else says nothing and the caller
// takes another look at everything.
pub fn window_now(window: WindowId) -> Option<Rect> {
    let active = json("j/activewindow")?;
    if window_id(active.get("address")?.as_str()?)? != window {
        return None;
    }
    let (at, size) = (active.get("at")?.as_array()?, active.get("size")?.as_array()?);
    let (x, y) = (int(&at[0]), int(&at[1]));
    let screens = recent_screens();
    let (left, top) = to_device(&screens, (x, y));
    let (right, bottom) = to_device(&screens, (x + int(&size[0]), y + int(&size[1])));
    Some(Rect::new(left, top, right, bottom))
}

// Hyprland says what it is doing on a second socket, a line each.
pub(super) fn watch_events(told: impl Fn(Told) + Send + 'static) {
    std::thread::spawn(move || {
        loop {
            match events_socket() {
                Some(stream) => read_events(stream, &told),
                None => std::thread::sleep(Duration::from_secs(5)),
            }
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Told {
    Layout,
    Screens,
}

fn events_socket() -> Option<UnixStream> {
    UnixStream::connect(runtime_dir().join("hypr").join(instance()?).join(".socket2.sock")).ok()
}

fn read_events(stream: UnixStream, told: &impl Fn(Told)) {
    let reader = std::io::BufReader::new(stream);
    for line in std::io::BufRead::lines(reader).map_while(Result::ok) {
        if let Some(what) = event_means(&line) {
            told(what);
        }
    }
}

fn event_means(line: &str) -> Option<Told> {
    match line.split_once(">>").map_or(line, |(name, _)| name) {
        "openwindow" | "closewindow" | "movewindow" | "movewindowv2" | "changefloatingmode" | "fullscreen" | "workspace" | "focusedmon" => Some(Told::Layout),
        "monitoradded" | "monitoraddedv2" | "monitorremoved" | "monitorremovedv2" | "configreloaded" => Some(Told::Screens),
        _ => None,
    }
}

// His own surfaces, by the names the compositor knows them under. A bar or a
// launcher is a layer surface and never appears among the clients at all.
pub const FURNITURE: &[&str] = &["raccy", "raccy-panel", "raccy-menu", "raccy-ground"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_event_socket_says_what_moved() {
        assert_eq!(event_means("openwindow>>8f0c,1,kitty,kitty"), Some(Told::Layout));
        assert_eq!(event_means("movewindowv2>>8f0c,2,2"), Some(Told::Layout));
        assert_eq!(event_means("monitorremoved>>DP-1"), Some(Told::Screens));
        assert_eq!(event_means("configreloaded>>"), Some(Told::Screens));
        assert_eq!(event_means("activelayout>>keyboard,cz"), None);
    }

    #[test]
    fn monitors_come_out_in_screen_pixels() {
        let monitors: Value = serde_json::from_str(
            r#"[{"id":0,"name":"HEADLESS-2","x":0,"y":0,"width":1920,"height":1080,"scale":2.0,
                 "reserved":[0,30,0,0],"activeWorkspace":{"id":2}},
                {"id":1,"name":"DP-1","x":960,"y":0,"width":2560,"height":1440,"scale":1.0,
                 "reserved":[0,0,0,0],"activeWorkspace":{"id":3}}]"#,
        )
        .expect("json");
        let screens = screens_of(&monitors);
        assert_eq!(screens[0].logical, Rect::new(0, 0, 960, 540));
        assert_eq!(screens[0].device, Rect::new(0, 0, 1920, 1080));
        assert_eq!(screens[0].work, Rect::new(0, 60, 1920, 1080), "a bar of thirty logical rows is sixty");
        assert_eq!(screens[1].logical, Rect::new(960, 0, 3520, 1440));
        assert_eq!(screens[1].device, Rect::new(1920, 0, 4480, 1440), "it begins where the dense one ends, not where its own scale would put it");
        assert_eq!(to_device(&screens, (100, 100)), (200, 200), "on the dense monitor");
        assert_eq!(to_device(&screens, (1000, 100)), (1960, 100), "on the plain one, past the dense one's pixels");
        assert_eq!(screen_of_device(&screens, (300, 300)).map(|s| s.id), Some(0));
        assert_eq!(screen_of_device(&screens, (2000, 300)).map(|s| s.id), Some(1));
        assert!(screens[0].device.right <= screens[1].device.left, "in screen pixels the two never overlap");
    }
}

