#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use windows::*;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

use std::path::PathBuf;

pub struct FontFiles {
    pub main: Vec<PathBuf>,
    pub fallback: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Until his window on macOS is built, nothing there makes most of these.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub enum Event {
    Frame,
    Glide,
    Grabbed,
    Dropped,
    Clicked,
    Files(PathBuf, usize),
    Menu,
    PanelClicked,
    PanelCopy,
    PanelWheel(i32),
    Moved,
    DisplayChanged,
    #[cfg(windows)]
    Scaled { dpi: u32, suggested: Option<Rect> },
    SeatMoved(WindowId),
    AutostartDone { on: bool, done: bool },
    EndSession,
    #[cfg(feature = "trace")]
    TestCommand(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect { left, top, right, bottom }
    }

    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub fn contains(&self, (x, y): (i32, i32)) -> bool {
        (self.left..self.right).contains(&x) && (self.top..self.bottom).contains(&y)
    }

    pub fn overlaps(&self, other: &Rect) -> bool {
        self.left < other.right && other.left < self.right && self.top < other.bottom && other.top < self.bottom
    }

    pub fn centre(&self) -> (i32, i32) {
        (self.left + self.width() / 2, self.top + self.height() / 2)
    }

    fn distance(&self, (x, y): (i32, i32)) -> i64 {
        let dx = (self.left - x).max(x - self.right + 1).max(0) as i64;
        let dy = (self.top - y).max(y - self.bottom + 1).max(0) as i64;
        dx * dx + dy * dy
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub isize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MonitorId(pub isize);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub id: MonitorId,
    pub whole: Rect,
    pub work: Rect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: WindowId,
    pub pid: u32,
    pub frame: Rect,
    pub class: String,
    pub maximised: bool,
    pub overlay: bool,
    pub topmost: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Desktop {
    pub monitors: Vec<Monitor>,
    pub windows: Vec<Window>,
    pub foreground: Option<WindowId>,
    pub cursor: Option<(i32, i32)>,
}

impl Desktop {
    pub fn monitor_at(&self, point: (i32, i32)) -> Option<&Monitor> {
        self.monitors.iter().min_by_key(|m| m.whole.distance(point))
    }

    pub fn monitor_of(&self, rect: &Rect) -> Option<&Monitor> {
        self.monitor_at(rect.centre())
    }

    pub fn on_a_monitor(&self, rect: &Rect) -> bool {
        self.monitors.iter().any(|m| m.whole.overlaps(rect))
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn monitor_of_window(&self, id: WindowId) -> Option<&Monitor> {
        self.window(id).and_then(|w| self.monitor_of(&w.frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREAS: [&str; 12] =
        ["shell", "desktop", "system", "autostart", "net", "lan", "wire", "inspect", "tools", "host", "autoruns", "redirects"];

    const SYSTEMS: [&str; 3] = ["windows", "linux", "macos"];

    // They are read as text, since only one of them is compiled at a time.
    #[test]
    fn every_system_offers_the_same_seam() {
        let offered = |system: &str, area: &str| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("src/platform/{system}/{area}.rs"));
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let mut offered: Vec<String> = text
                .lines()
                .filter_map(|line| line.trim_end().strip_prefix("pub "))
                .filter_map(|rest| rest.split_once(' '))
                .filter_map(|(what, rest)| match what {
                    // A function is read whole, argument names and all. Two
                    // halves that call a thing the same and mean something
                    // else by it are exactly what comes apart quietly, and an
                    // argument name is the only place they say what they mean.
                    // An underscore in front of one says only that this half
                    // has no use for it, not that it is a different argument.
                    "fn" => {
                        let signature = rest.trim_end_matches("{}").trim_end_matches('{').trim_end().replace("(_", "(").replace(" _", " ");
                        Some(format!("fn {signature}"))
                    }
                    // What a type holds and what a list is filled with is each
                    // half's own business; that both halves offer it at all is
                    // the whole of the contract.
                    "struct" | "enum" | "type" | "const" => {
                        let name = rest.split(['(', '<', ':', ' ', ';', '{', '=']).next().unwrap_or_default();
                        Some(format!("{what} {name}"))
                    }
                    _ => None,
                })
                .collect();
            offered.sort();
            offered
        };
        for area in AREAS {
            let first = offered(SYSTEMS[0], area);
            for system in &SYSTEMS[1..] {
                assert_eq!(first, offered(system, area), "platform::{area} has come apart between {} and {system}", SYSTEMS[0]);
            }
        }
    }

    fn desk() -> Desktop {
        Desktop {
            monitors: vec![
                Monitor { id: MonitorId(1), whole: Rect::new(0, 0, 2560, 1440), work: Rect::new(0, 0, 2560, 1392) },
                Monitor { id: MonitorId(2), whole: Rect::new(2560, -200, 4480, 880), work: Rect::new(2560, -200, 4480, 880) },
            ],
            windows: vec![Window { id: WindowId(7), pid: 1, frame: Rect::new(2600, 0, 3600, 700), class: "x".into(), maximised: false, overlay: false, topmost: false }],
            foreground: Some(WindowId(7)),
            cursor: None,
        }
    }

    #[test]
    fn a_point_finds_its_monitor_or_the_nearest() {
        let d = desk();
        assert_eq!(d.monitor_at((100, 100)).map(|m| m.id), Some(MonitorId(1)));
        assert_eq!(d.monitor_at((3000, 0)).map(|m| m.id), Some(MonitorId(2)));
        assert_eq!(d.monitor_at((2600, 1400)).map(|m| m.id), Some(MonitorId(1)), "below the second: nearer the first");
        assert_eq!(d.monitor_at((-500, -500)).map(|m| m.id), Some(MonitorId(1)));
        assert_eq!(d.monitor_of_window(WindowId(7)).map(|m| m.id), Some(MonitorId(2)));
        assert!(d.on_a_monitor(&Rect::new(2500, 100, 2700, 200)));
        assert!(!d.on_a_monitor(&Rect::new(5000, 5000, 5100, 5100)));
        assert!(Desktop::default().monitor_at((0, 0)).is_none());
    }

    #[test]
    fn rectangles_measure_and_overlap() {
        let r = Rect::new(10, 20, 110, 70);
        assert_eq!((r.width(), r.height(), r.centre()), (100, 50, (60, 45)));
        assert!(r.contains((10, 20)) && !r.contains((110, 20)));
        assert!(r.overlaps(&Rect::new(100, 60, 200, 100)) && !r.overlaps(&Rect::new(110, 20, 200, 100)));
        assert!(Rect::new(5, 5, 5, 9).is_empty());
        assert_eq!(r.distance((60, 45)), 0);
        assert_eq!(r.distance((0, 20)), 100);
    }
}
