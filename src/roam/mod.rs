use crate::platform::{Desktop, Monitor, Rect, Window, WindowId};
use crate::render::{H, Rng, SPRITE_X, SPRITE_Y, W};
use crate::tuning::tuning;
use crate::render::sprite::{Gait, Idle, SIZE};

// Sprite rows of leg that hang below the edge he sits on.
const LEGS_BELOW: usize = 3;
const PEEK_END: u32 = 50;
// PIPE_AHEAD is in sprite pixels, to the left of where he stands; PIPE_STEP is rows a frame.
const PIPE_AHEAD: i32 = 30;
const PIPE_STEP: u32 = 3;
mod seats;

pub use seats::{Perch, buried, fullscreen_app, home_work_area, perch_under};
use seats::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Patrol,
    // The u32 is a process id.
    Sat(u32),
    Fell,
    Covered,
    Back,
    Hide,
    Escape,
    Travel,
    Arrived,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Tunnel {
    x: i32,
    monitor_bottom: i32,
    work_bottom: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Then {
    Pause,
    Home,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Seat {
    window: WindowId,
    offset: i32,
    frames: u32,
    pinned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Home,
    Walk { to: i32, then: Then },
    Pause { frames: u32 },
    ToSeat(Seat),
    Jump { from: (i32, i32), to: (i32, i32), t: u32, n: u32, onto: Onto },
    Sit(Seat),
    Hiding { x: i32, t: u32 },
    Rising { x: i32, rows: u32 },
    Sinking { x: i32, from: i32, bottom: i32, t: u32, n: u32, onto: Onto },
    Leaving { frames: u32, to: Option<Tunnel> },
    Surfacing { tunnel: Tunnel, rows: u32 },
    Opening { x: i32, onto: Onto },
    Closing { tunnel: Tunnel },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Onto {
    Seat(Seat),
    Edge,
    Hide,
    Tunnel(Tunnel),
}

pub struct Frame<'a> {
    pub desk: &'a Desktop,
    pub own: WindowId,
    pub here: (i32, i32),
    pub home: (i32, i32),
    pub scale: i32,
    pub may_start: bool,
    pub may_stay: bool,
    pub hide: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Move {
    pub to: Option<(i32, i32)>,
    pub gait: Gait,
    pub facing_left: bool,
    pub event: Option<Event>,
    pub look: Option<Idle>,
    pub hidden: bool,
    pub teleport: bool,
    pub home: Option<(i32, i32)>,
    pub ground: Option<i32>,
    pub pipe: Option<Pipe>,
    pub underground: bool,
}

// `x` is a window position, `base` a screen row, `rows` is in sprite pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pipe {
    pub x: i32,
    pub base: i32,
    pub rows: u32,
    rising: bool,
}

pub struct Roam {
    state: State,
    rng: Rng,
    // Per frame: a patrol, a sit, a trip.
    chances: (f64, f64, f64),
    hide_seen: bool,
    steady: u32,
    restless: u32,
    away: Option<(i32, i32)>,
    homeward: bool,
    pipe: Option<Pipe>,
    covered: u32,
}

fn chances() -> (f64, f64, f64) {
    #[cfg(feature = "trace")]
    {
        if std::env::var_os("RACCY_ROAM_TRAVEL").is_some() {
            return (0.0, 0.0, 1.0 / 100.0);
        }
        if std::env::var_os("RACCY_ROAM_OFTEN").is_some() {
            return (1.0 / 600.0, 1.0 / 30.0, 0.0);
        }
    }
    (1.0 / f64::from(tuning().roam.patrol_one_in_frames), 1.0 / f64::from(tuning().roam.sit_one_in_frames), 1.0 / f64::from(tuning().roam.travel_one_in_frames))
}

impl Roam {
    pub fn new(seed: u64) -> Roam {
        Roam { state: State::Home, rng: Rng::new(seed | 1), chances: chances(), hide_seen: false, steady: 0, restless: 0, away: None, homeward: false, pipe: None, covered: 0 }
    }

    pub fn at_home(&self) -> bool {
        self.state == State::Home
    }

    pub fn seat_window(&self) -> Option<WindowId> {
        match self.state {
            State::Sit(seat) => Some(seat.window),
            _ => None,
        }
    }

    pub fn seat_spot(&self, desk: &Desktop, scale: i32) -> Option<(i32, i32)> {
        let State::Sit(seat) = self.state else { return None };
        desk.window(seat.window).and_then(|w| seat_frame(desk, w, scale)).map(|frame| sit_position(frame, seat.offset, scale))
    }

    pub fn grabbed(&mut self) {
        self.state = State::Home;
        (self.away, self.homeward, self.pipe) = (None, false, None);
    }

    fn open_pipe(&mut self, f: &Frame, onto: Onto) {
        let x = f.here.0 - PIPE_AHEAD * f.scale;
        self.pipe = Some(Pipe { x, base: f.here.1 + H as i32 * f.scale, rows: 0, rising: true });
        self.state = State::Opening { x, onto };
    }

    pub fn dropped(&mut self, here: (i32, i32), home: (i32, i32), scale: i32, perch: Option<Perch>) {
        self.grabbed();
        if let Some(Perch { seat, frame }) = perch {
            self.state = jump(here, sit_position(frame, seat.offset, scale), Onto::Seat(seat), scale);
        } else if here != home {
            self.state = jump(here, home, Onto::Edge, scale);
        }
    }

    fn tunnel(&mut self, f: &Frame) -> Option<Tunnel> {
        let monitors = free_monitors(f);
        if monitors.is_empty() {
            return None;
        }
        let monitor = monitors[(self.rng.unit64() * monitors.len() as f64) as usize % monitors.len()];
        let work = monitor.work;
        let span = (work.width() - width(f.scale)).max(0);
        let x = work.left + (self.rng.unit64() * span as f64) as i32;
        Some(Tunnel { x, monitor_bottom: monitor.whole.bottom, work_bottom: work.bottom })
    }

    pub fn step(&mut self, f: &Frame) -> Move {
        let before = std::mem::discriminant(&self.state);
        let mut moved = self.advance(f);
        if std::mem::discriminant(&self.state) != before {
            crate::trace::record(|| format!("roam state {:?}", self.state));
        }
        self.pipe = self.pipe.and_then(|pipe| {
            let rows = if pipe.rising { (pipe.rows + PIPE_STEP).min(crate::render::PIPE_ROWS) } else { pipe.rows.saturating_sub(PIPE_STEP) };
            // Down to nothing for a frame before it goes, so it slides all the way.
            (pipe.rising || pipe.rows > 0).then_some(Pipe { rows, ..pipe })
        });
        moved.pipe = self.pipe;
        moved
    }

    fn advance(&mut self, f: &Frame) -> Move {
        if f.hide == self.hide_seen {
            self.steady = self.steady.saturating_add(1);
        } else {
            (self.hide_seen, self.steady) = (f.hide, 0);
        }
        let settled = self.steady >= tuning().roam.settle_frames;
        let hiding = matches!(self.state, State::Hiding { .. } | State::Jump { onto: Onto::Hide, .. } | State::Sinking { onto: Onto::Hide, .. });
        // The home monitor's edge, even when his window hangs over another monitor.
        let x_on_edge = || home_work_area(f.desk, f.home, f.scale).map_or(f.here.0, |work| on_edge(f.here.0, work, f.scale));
        let leaving = matches!(self.state, State::Leaving { .. });
        let travelling =
            matches!(
                self.state,
                State::Opening { onto: Onto::Tunnel(_), .. }
                    | State::Jump { onto: Onto::Tunnel(_), .. }
                    | State::Sinking { onto: Onto::Tunnel(_), .. }
                    | State::Closing { .. }
                    | State::Surfacing { .. }
            );
        // A jump lands first: leaving mid-air would leave him hanging there.
        let jumping = matches!(self.state, State::Jump { .. });
        if settled && f.hide && !hiding && !leaving && !travelling && !jumping {
            let to = self.tunnel(f);
            if to.is_some() {
                (self.away, self.homeward) = (None, false);
            }
            self.state = State::Leaving { frames: tuning().roam.announce_frames, to };
            let event = if to.is_some() { Event::Escape } else { Event::Hide };
            return Move { event: Some(event), ..Move::default() };
        } else if settled && !f.hide && leaving {
            self.state = match f.here == f.home {
                true => State::Home,
                false => jump(f.here, (x_on_edge(), f.home.1), Onto::Edge, f.scale),
            };
        } else if settled && !f.hide && hiding {
            self.state = match self.state {
                State::Hiding { x, t } => State::Rising { x, rows: peek(t).0 },
                _ => jump(f.here, (x_on_edge(), f.home.1), Onto::Edge, f.scale),
            };
        }
        match self.state {
            State::Home => {
                if !f.may_start {
                    return Move::default();
                }
                if let Some(back) = self.away.filter(|_| self.restless == 0) {
                    let there = monitor_under(f, back);
                    if there.map(|m| m.id) == monitor_under(f, f.home).map(|m| m.id) {
                        self.away = None;
                    } else if let Some(tunnel) = there.and_then(|m| tunnel_to(f.desk, m, back.0, f.scale)) {
                        self.homeward = true;
                        self.open_pipe(f, Onto::Tunnel(tunnel));
                        return Move::default();
                    }
                }
                let roll = self.rng.unit64();
                if (self.chances.0..self.chances.0 + self.chances.1).contains(&roll) {
                    self.restless = tuning().roam.sit_wish_frames;
                }
                if self.restless > 0 {
                    self.restless -= 1;
                    match seat(f) {
                        Some((window, offset)) if !same_monitor(f, window) => {
                            if let Some(tunnel) = tunnel_below(f, window, offset) {
                                self.away.get_or_insert(f.home);
                                self.open_pipe(f, Onto::Tunnel(tunnel));
                                return Move { event: Some(Event::Travel), ..Move::default() };
                            }
                        }
                        Some((window, offset)) => {
                            self.restless = 0;
                            let frames = tuning().roam.sit_min_frames + (self.rng.unit64() * (tuning().roam.sit_max_frames - tuning().roam.sit_min_frames) as f64) as u32;
                            self.state = State::ToSeat(Seat { window, offset, frames, pinned: false });
                            return Move::default();
                        }
                        None if self.restless == tuning().roam.sit_wish_frames - 1 => crate::trace::log(|| seat_report(f)),
                        None => {}
                    }
                }
                if roll < self.chances.0 {
                    let distance = tuning().roam.patrol_min_px + (self.rng.unit64() * (tuning().roam.patrol_max_px - tuning().roam.patrol_min_px) as f64) as i32;
                    let left = self.rng.unit64() < 0.5;
                    let target = own_work_area(f).and_then(|work| patrol_target(f.home.0, work, width(f.scale), distance, left));
                    if let Some(to) = target {
                        self.state = State::Walk { to, then: Then::Pause };
                        return Move { event: Some(Event::Patrol), ..Move::default() };
                    }
                } else if (self.chances.0 + self.chances.1..self.chances.0 + self.chances.1 + self.chances.2).contains(&roll)
                    && let Some(tunnel) = self.tunnel(f)
                {
                    self.away = None;
                    self.open_pipe(f, Onto::Tunnel(tunnel));
                    return Move { event: Some(Event::Travel), ..Move::default() };
                }
                Move::default()
            }
            State::Walk { to, then } => {
                let (to, then) = if f.may_stay { (to, then) } else { (f.home.0, Then::Home) };
                self.state = State::Walk { to, then };
                if cursor_over(f) {
                    return Move::default();
                }
                let x = walk(f.here.0, to, tuning().roam.walk_step_px * f.scale);
                if x == f.here.0 {
                    self.state = match then {
                        Then::Pause => State::Pause { frames: tuning().roam.pause_frames },
                        Then::Home => State::Home,
                    };
                    // Back on the exact spot, whatever the rounding did.
                    let to = (then == Then::Home && f.here != f.home).then_some(f.home);
                    return Move { to, ..Move::default() };
                }
                Move { to: Some((x, f.home.1)), gait: Gait::Walking, facing_left: x < f.here.0, ..Move::default() }
            }
            State::Pause { frames } => {
                self.state = match frames {
                    _ if !f.may_stay => State::Walk { to: f.home.0, then: Then::Home },
                    0 => State::Walk { to: f.home.0, then: Then::Home },
                    n => State::Pause { frames: n - 1 },
                };
                Move::default()
            }
            State::ToSeat(seat) => {
                let (Some(frame), Some(work)) = (still_seat(seat, f), own_work_area(f)) else {
                    self.state = State::Walk { to: f.home.0, then: Then::Home };
                    return Move::default();
                };
                if cursor_over(f) {
                    return Move::default();
                }
                let top = sit_position(frame, seat.offset, f.scale);
                let below = on_edge(top.0, work, f.scale);
                let x = walk(f.here.0, below, tuning().roam.walk_step_px * f.scale);
                if x != f.here.0 {
                    return Move { to: Some((x, f.home.1)), gait: Gait::Walking, facing_left: x < f.here.0, ..Move::default() };
                }
                self.state = jump(f.here, top, Onto::Seat(seat), f.scale);
                Move::default()
            }
            State::Jump { from, to, t, n, onto } => {
                let t = t + 1;
                let to = match onto {
                    Onto::Seat(seat) => still_seat(seat, f).map_or(to, |frame| sit_position(frame, seat.offset, f.scale)),
                    _ => to,
                };
                if t >= n {
                    return match onto {
                        Onto::Seat(seat) if still_seat(seat, f).is_none() => {
                            self.state = jump(to, (x_on_edge(), f.home.1), Onto::Edge, f.scale);
                            Move { to: Some(to), gait: Gait::Jumping, event: Some(Event::Fell), ..Move::default() }
                        }
                        Onto::Seat(seat) => {
                            self.state = State::Sit(seat);
                            let pid = f.desk.window(seat.window).map_or(0, |w| w.pid);
                            Move { to: Some(to), gait: Gait::Sitting, event: Some(Event::Sat(pid)), ..Move::default() }
                        }
                        Onto::Edge => {
                            self.state = State::Walk { to: f.home.0, then: Then::Home };
                            Move { to: Some(to), ..Move::default() }
                        }
                        Onto::Hide | Onto::Tunnel(_) => {
                            let bottom = match self.pipe {
                                Some(pipe) if matches!(onto, Onto::Tunnel(_)) => below(pipe.base, f.scale),
                                _ => home_rect(f).map_or(to.1, |m| hidden_y(m, f.scale)),
                            };
                            let n = ((bottom - to.1) / (tuning().roam.sink_step_px * f.scale)).clamp(tuning().roam.sink_min_frames, tuning().roam.sink_max_frames) as u32;
                            self.state = State::Sinking { x: to.0, from: to.1, bottom, t: 0, n, onto };
                            Move { to: Some(to), ground: ground(onto, self.pipe, f), ..Move::default() }
                        }
                    };
                }
                self.state = State::Jump { from, to, t, n, onto };
                let ground = ground(onto, self.pipe, f);
                Move { to: Some(arc(from, to, t, n, f.scale)), gait: Gait::Jumping, facing_left: to.0 < from.0, ground, ..Move::default() }
            }
            State::Sinking { x, from, bottom, t, n, onto } => {
                let t = t + 1;
                let ground = ground(onto, self.pipe, f);
                if t <= n {
                    self.state = State::Sinking { x, from, bottom, t, n, onto };
                    let y = from + (bottom - from) * t as i32 / n as i32;
                    return Move { to: Some((x, y)), ground, ..Move::default() };
                }
                match onto {
                    Onto::Tunnel(tunnel) => {
                        if let Some(pipe) = &mut self.pipe {
                            pipe.rising = false;
                        }
                        self.state = State::Closing { tunnel };
                        Move { to: Some((x, bottom)), ground, ..Move::default() }
                    }
                    _ => {
                        crate::trace::record(|| "roam behind the edge".to_string());
                        self.state = State::Hiding { x, t: 0 };
                        Move { to: Some((x, bottom)), ground, ..Move::default() }
                    }
                }
            }
            State::Opening { x, onto } => {
                if self.pipe.is_none_or(|pipe| pipe.rows >= crate::render::PIPE_ROWS) {
                    self.state = hop(f.here, x, onto, f.scale);
                }
                Move::default()
            }
            State::Closing { tunnel } => {
                if let Some(pipe) = self.pipe {
                    return Move { ground: Some(pipe.base), underground: true, ..Move::default() };
                }
                crate::trace::record(|| format!("roam tunnel to {tunnel:?}"));
                self.pipe = Some(Pipe { x: tunnel.x, base: tunnel.work_bottom, rows: 0, rising: true });
                self.state = State::Surfacing { tunnel, rows: 0 };
                let below = (tunnel.x, below(tunnel.work_bottom, f.scale));
                Move { to: Some(below), hidden: true, teleport: true, ground: Some(tunnel.work_bottom), underground: true, ..Move::default() }
            }
            State::Sit(seat) => match (seat.frames > 0).then(|| still_seat(seat, f)).flatten() {
                Some(frame) => {
                    let spot = sit_position(frame, seat.offset, f.scale);
                    self.covered = if covered(f, seat.window, spot) { self.covered + 1 } else { 0 };
                    if self.covered >= COVERED_FRAMES {
                        self.covered = 0;
                        self.state = jump(f.here, (x_on_edge(), f.home.1), Onto::Edge, f.scale);
                        return Move { gait: Gait::Sitting, event: Some(Event::Covered), ..Move::default() };
                    }
                    self.state = State::Sit(Seat { frames: seat.frames - 1, ..seat });
                    Move { to: Some(spot), gait: Gait::Sitting, ..Move::default() }
                }
                None => {
                    let fell = seat.frames > 0;
                    self.state = jump(f.here, (x_on_edge(), f.home.1), Onto::Edge, f.scale);
                    Move { gait: Gait::Sitting, event: fell.then_some(Event::Fell), ..Move::default() }
                }
            },
            State::Hiding { x, t } => {
                self.state = State::Hiding { x, t: t.saturating_add(1) };
                let Some(monitor) = home_rect(f) else { return Move::default() };
                let (rows, look) = peek(t);
                let to = (x, hidden_y(monitor, f.scale) - rows as i32 * f.scale);
                Move { to: Some(to), look, hidden: t >= PEEK_END, ground: Some(monitor.bottom), ..Move::default() }
            }
            State::Rising { x, rows } => {
                let Some(monitor) = home_rect(f) else { return Move::default() };
                let rows = (rows + tuning().roam.climb_step_px).min(SIZE as u32);
                let to = (x, hidden_y(monitor, f.scale) - rows as i32 * f.scale);
                if rows < SIZE as u32 {
                    self.state = State::Rising { x, rows };
                    return Move { to: Some(to), ground: Some(monitor.bottom), ..Move::default() };
                }
                self.state = jump(to, (x, f.home.1), Onto::Edge, f.scale);
                Move { to: Some(to), event: Some(Event::Back), ground: Some(monitor.bottom), ..Move::default() }
            }
            State::Leaving { frames: 0, to } => {
                match to {
                    Some(tunnel) => self.open_pipe(f, Onto::Tunnel(tunnel)),
                    None => self.state = hop(f.here, x_on_edge(), Onto::Hide, f.scale),
                }
                Move::default()
            }
            State::Leaving { frames, to } => {
                self.state = State::Leaving { frames: frames - 1, to };
                Move::default()
            }
            State::Surfacing { tunnel, rows } => {
                let (ground, sunk) = (Some(tunnel.work_bottom), below(tunnel.work_bottom, f.scale));
                if self.pipe.is_some_and(|pipe| pipe.rising && pipe.rows < crate::render::PIPE_ROWS) {
                    return Move { to: Some((tunnel.x, sunk)), ground, underground: true, ..Move::default() };
                }
                let rows = (rows + tuning().roam.climb_step_px).min(SIZE as u32);
                let to = (tunnel.x, sunk - rows as i32 * f.scale);
                if rows < SIZE as u32 {
                    self.state = State::Surfacing { tunnel, rows };
                    return Move { to: Some(to), ground, underground: true, ..Move::default() };
                }
                if let Some(pipe) = &mut self.pipe {
                    pipe.rising = false;
                }
                let home = arrival(tunnel, f.scale);
                self.state = jump(to, home, Onto::Edge, f.scale);
                let event = (!self.homeward).then_some(Event::Arrived);
                (self.away, self.homeward) = (self.away.filter(|_| !self.homeward), false);
                Move { to: Some(to), home: Some(home), event, ground, ..Move::default() }
            }
        }
    }
}

fn ground(onto: Onto, pipe: Option<Pipe>, f: &Frame) -> Option<i32> {
    match (onto, pipe) {
        (Onto::Tunnel(_), Some(pipe)) => Some(pipe.base),
        (Onto::Hide | Onto::Tunnel(_), _) => home_rect(f).map(|m| m.bottom),
        (Onto::Seat(_) | Onto::Edge, _) => None,
    }
}

fn tunnel_below(f: &Frame, window: WindowId, offset: i32) -> Option<Tunnel> {
    let w = f.desk.window(window)?;
    let monitor = f.desk.monitor_of(&w.frame)?;
    tunnel_to(f.desk, monitor, sit_position(w.frame, offset, f.scale).0, f.scale)
}

fn tunnel_to(desk: &Desktop, monitor: &Monitor, x: i32, scale: i32) -> Option<Tunnel> {
    if fullscreen_on(desk, monitor).is_some() {
        return None;
    }
    Some(Tunnel { x: on_edge(x, monitor.work, scale), monitor_bottom: monitor.whole.bottom, work_bottom: monitor.work.bottom })
}

pub fn standing(desk: &Desktop, pos: (i32, i32), scale: i32) -> (i32, i32) {
    monitor_at(desk, pos, scale).map_or(pos, |m| stand_on(pos, m.work, scale))
}

fn stand_on(pos: (i32, i32), work: Rect, scale: i32) -> (i32, i32) {
    (on_edge(pos.0, work, scale), work.bottom - H as i32 * scale)
}

fn walk(x: i32, to: i32, step: i32) -> i32 {
    x + (to - x).clamp(-step, step)
}

fn width(scale: i32) -> i32 {
    W as i32 * scale
}

fn on_edge(x: i32, work: Rect, scale: i32) -> i32 {
    x.clamp(work.left, (work.right - width(scale)).max(work.left))
}

fn patrol_target(home_x: i32, work: Rect, width: i32, distance: i32, left: bool) -> Option<i32> {
    let wanted = if left { home_x - distance } else { home_x + distance };
    let to = wanted.clamp(work.left, (work.right - width).max(work.left));
    ((to - home_x).abs() >= tuning().roam.patrol_min_px / 2).then_some(to)
}

fn jump(from: (i32, i32), to: (i32, i32), onto: Onto, scale: i32) -> State {
    let distance = ((to.0 - from.0) as f64).hypot((to.1 - from.1) as f64) as i32;
    let n = (distance / (tuning().roam.jump_step_px * scale)).clamp(tuning().roam.jump_min_frames, tuning().roam.jump_max_frames) as u32;
    State::Jump { from, to, t: 0, n, onto }
}

fn hop(from: (i32, i32), x: i32, onto: Onto, scale: i32) -> State {
    jump(from, (x, from.1), onto, scale)
}

fn arc(from: (i32, i32), to: (i32, i32), t: u32, n: u32, scale: i32) -> (i32, i32) {
    let s = t as f64 / n as f64;
    let lift = (tuning().roam.jump_height_px * scale) as f64 * 4.0 * s * (1.0 - s);
    let x = from.0 as f64 + (to.0 - from.0) as f64 * s;
    let y = from.1 as f64 + (to.1 - from.1) as f64 * s - lift;
    (x.round() as i32, y.round() as i32)
}

fn hidden_y(monitor: Rect, scale: i32) -> i32 {
    below(monitor.bottom, scale)
}

fn below(monitor_bottom: i32, scale: i32) -> i32 {
    monitor_bottom - SPRITE_Y as i32 * scale
}

fn arrival(tunnel: Tunnel, scale: i32) -> (i32, i32) {
    (tunnel.x, tunnel.work_bottom - H as i32 * scale)
}

fn monitor_at(desk: &Desktop, pos: (i32, i32), scale: i32) -> Option<&Monitor> {
    desk.monitor_at((pos.0 + width(scale) / 2, pos.1 + H as i32 * scale / 2))
}

fn monitor_under<'a>(f: &Frame<'a>, pos: (i32, i32)) -> Option<&'a Monitor> {
    monitor_at(f.desk, pos, f.scale)
}

fn free_monitors<'a>(f: &Frame<'a>) -> Vec<&'a Monitor> {
    let mine = monitor_under(f, f.home).map(|m| m.id);
    f.desk.monitors.iter().filter(|m| Some(m.id) != mine && fullscreen_on(f.desk, m).is_none()).collect()
}

// Sprite rows showing: 15 is up to the visor, 7 is down to the ears.
fn peek(t: u32) -> (u32, Option<Idle>) {
    match t {
        0..=4 => (t * 3, None),
        5..=12 => (15, Some(Idle::LookLeft)),
        13..=20 => (15, Some(Idle::LookRight)),
        21..=24 => (15 - (t - 20) * 2, None),
        25..=45 => (7, None),
        46..PEEK_END => (7u32.saturating_sub((t - 45) * 2), None),
        _ => (0, None),
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{MonitorId, Window};

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect::new(left, top, right, bottom)
    }

    fn frame<'a>(desk: &'a Desktop, here: (i32, i32), home: (i32, i32), may_start: bool, hide: bool) -> Frame<'a> {
        Frame { desk, own: WindowId(0), here, home, scale: 4, may_start, may_stay: true, hide }
    }

    // windows are in z-order, front to back.
    fn desk_with(windows: Vec<Window>) -> Desktop {
        Desktop {
            monitors: vec![Monitor { id: MonitorId(1), whole: rect(0, 0, 2560, 1440), work: rect(0, 0, 2560, 1392) }],
            windows,
            foreground: None,
            cursor: None,
        }
    }

    fn window(id: isize, frame: Rect) -> Window {
        Window { id: WindowId(id), pid: id as u32, frame, class: "App".into(), maximised: false, overlay: false, topmost: false }
    }

    #[test]
    fn walking_never_overshoots() {
        assert_eq!(walk(100, 200, 8), 108);
        assert_eq!(walk(100, 104, 8), 104);
        assert_eq!(walk(100, 40, 8), 92);
        assert_eq!(walk(100, 100, 8), 100);
    }

    #[test]
    fn patrols_stay_on_the_monitor() {
        let work = rect(0, 0, 1920, 1040);
        assert_eq!(patrol_target(1000, work, 250, 300, true), Some(700));
        assert_eq!(patrol_target(1000, work, 250, 300, false), Some(1300));
        assert_eq!(patrol_target(1670, work, 250, 400, false), None);
        assert_eq!(patrol_target(50, work, 250, 400, true), None);
        assert_eq!(patrol_target(300, work, 250, 400, true), Some(0));
        assert_eq!(on_edge(-40, work, 4), 0);
        assert_eq!(on_edge(1900, work, 4), 1920 - 256);
    }

    #[test]
    fn jumps_arc_from_start_to_landing() {
        let (from, to) = ((0, 500), (100, 200));
        assert_eq!(arc(from, to, 0, 10, 4), from);
        assert_eq!(arc(from, to, 10, 10, 4), to);
        assert_eq!(arc(from, to, 5, 10, 4), (50, 350 - tuning().roam.jump_height_px * 4));
        let State::Jump { n, .. } = jump((0, 0), (0, 240), Onto::Edge, 4) else { panic!() };
        assert_eq!(n, 10);
        let State::Jump { n, .. } = jump((0, 0), (0, 3000), Onto::Edge, 4) else { panic!() };
        assert_eq!(n, tuning().roam.jump_max_frames as u32);
    }

    #[test]
    fn peeks_from_behind_the_edge_then_vanishes() {
        assert_eq!(hidden_y(rect(0, 0, 2560, 1440), 4) + SPRITE_Y as i32 * 4, 1440);
        assert_eq!(peek(0), (0, None));
        assert_eq!(peek(8), (15, Some(Idle::LookLeft)));
        assert_eq!(peek(16), (15, Some(Idle::LookRight)));
        assert_eq!(peek(30), (7, None));
        assert_eq!(peek(PEEK_END - 1), (0, None));
        assert_eq!(peek(PEEK_END + 100), (0, None));
        assert!((0..PEEK_END).all(|t| peek(t).0 <= 15));
    }

    #[test]
    fn fullscreen_is_the_frontmost_window_covering_the_monitor() {
        let seen = |pid, covers, topmost| Seen { pid, covers, topmost };
        assert_eq!(fullscreen_among([seen(1, false, true), seen(2, true, false)]), Some(2), "an overlay in front is looked past");
        assert_eq!(fullscreen_among([seen(1, false, false), seen(2, true, false)]), None, "a window in front: someone works there");
        assert_eq!(fullscreen_among([]), None);
        let monitor = rect(0, -1440, 2560, 0);
        assert!(covers(rect(-8, -1448, 2568, 8), monitor), "a borderless window hanging over the edges");
        assert!(!covers(rect(0, -1440, 2560, -40), monitor), "short of the taskbar");
    }

    #[test]
    fn fullscreen_is_read_off_the_desktop() {
        let game = window(1, rect(0, 0, 2560, 1440));
        let maximised = Window { maximised: true, ..window(2, rect(0, 0, 2560, 1392)) };
        let raccy = Window { class: "RaccyPet".into(), topmost: true, ..window(3, rect(100, 1100, 356, 1340)) };
        let editor = window(4, rect(200, 200, 1200, 900));
        let monitor = |d: &Desktop| d.monitors[0].clone();
        let d = desk_with(vec![raccy.clone(), game.clone()]);
        assert_eq!(fullscreen_on(&d, &monitor(&d)), Some(1), "looked past his own window");
        assert_eq!(fullscreen_app(&d, (100, 1100), 4), Some(1));
        let d = desk_with(vec![maximised]);
        assert_eq!(fullscreen_on(&d, &monitor(&d)), None, "maximised is not fullscreen");
        let d = desk_with(vec![editor, game]);
        assert_eq!(fullscreen_on(&d, &monitor(&d)), None, "a window in front");
    }

    #[test]
    fn the_seat_is_the_window_in_front_with_room_above() {
        let editor = window(4, rect(200, 300, 1200, 900));
        let mut d = desk_with(vec![editor]);
        d.foreground = Some(WindowId(4));
        let f = frame(&d, (0, 1152), (0, 1152), true, false);
        let (window, offset) = seat(&f).expect("a seat");
        assert_eq!(window, WindowId(4));
        assert_eq!(offset, 300, "three tenths along");
        assert_eq!(sit_position(rect(200, 300, 1200, 900), offset, 4).0 + middle(4), 500);
        d.windows[0].maximised = true;
        assert!(seat(&frame(&d, (0, 1152), (0, 1152), true, false)).is_none(), "maximised");
        d.windows[0].maximised = false;
        d.windows[0].frame = rect(200, 40, 1200, 900);
        assert!(seat(&frame(&d, (0, 1152), (0, 1152), true, false)).is_none(), "no room above");
        d.windows[0].frame = rect(200, 300, 600, 900);
        assert!(seat(&frame(&d, (0, 1152), (0, 1152), true, false)).is_none(), "too narrow");
        d.windows[0].frame = rect(200, 300, 1200, 900);
        d.foreground = Some(WindowId(9));
        assert!(seat(&frame(&d, (0, 1152), (0, 1152), true, false)).is_none(), "the window in front does not show");
    }

    #[test]
    fn caught_hold_of_the_window_under_his_feet() {
        let scale = 4;
        let back = window(4, rect(200, 300, 1200, 900));
        let d = desk_with(vec![back.clone()]);
        let here = (500 - middle(scale), 300 - seat_line(scale) + 8);
        let perch = perch_under(&d, WindowId(0), here, scale).expect("caught hold");
        assert_eq!((perch.seat.window, perch.seat.pinned, perch.seat.frames), (WindowId(4), true, u32::MAX));
        assert_eq!(perch.seat.offset, 300);
        let front = window(5, rect(400, 100, 1000, 700));
        let d = desk_with(vec![front, back.clone()]);
        assert!(perch_under(&d, WindowId(0), here, scale).is_none(), "covered there by the window in front");
        let d = desk_with(vec![Window { maximised: true, ..back }]);
        assert!(perch_under(&d, WindowId(0), here, scale).is_none(), "a maximised window is no seat");
        let d = desk_with(vec![window(4, rect(200, 300, 1200, 900))]);
        assert!(perch_under(&d, WindowId(0), (here.0, here.1 - 400), scale).is_none(), "too far above");
    }

    #[test]
    fn a_window_in_front_of_his_seat_sends_him_down() {
        let scale = 4;
        let seat_window = window(4, rect(200, 300, 1200, 900));
        let seat = Seat { window: WindowId(4), offset: 300, frames: 3000, pinned: true };
        let spot = sit_position(seat_window.frame, seat.offset, scale);
        let sit = |d: &Desktop, roam: &mut Roam| -> Vec<Option<Event>> {
            (0..COVERED_FRAMES + 1).map(|_| roam.step(&frame(d, spot, (0, 1152), false, false)).event).collect()
        };
        let mut roam = Roam::new(1);
        roam.state = State::Sit(seat);
        let mut d = desk_with(vec![window(5, rect(1300, 100, 2000, 700)), seat_window.clone()]);
        d.foreground = Some(WindowId(4));
        assert!(sit(&d, &mut roam).iter().all(Option::is_none), "a window beside him is no bother");
        assert!(matches!(roam.state, State::Sit(_)));
        let mut d = desk_with(vec![window(5, rect(100, 100, 900, 700)), seat_window]);
        d.foreground = Some(WindowId(4));
        let events = sit(&d, &mut roam);
        assert_eq!(events.iter().flatten().collect::<Vec<_>>(), vec![&Event::Covered], "{events:?}");
        assert!(matches!(roam.state, State::Jump { onto: Onto::Edge, .. }), "{:?}", roam.state);
    }

    #[test]
    fn a_window_over_his_own_means_he_is_buried() {
        let him = rect(100, 800, 300, 1000);
        let own = window(1, him);
        let front = window(5, rect(0, 700, 900, 1100));
        let buried_with = |windows: Vec<Window>| buried(&desk_with(windows), WindowId(1), &him);
        assert!(buried_with(vec![front.clone(), own.clone()]));
        assert!(!buried_with(vec![own.clone(), front.clone()]), "behind him is no bother");
        assert!(!buried_with(vec![window(5, rect(1000, 700, 1900, 1100)), own.clone()]), "beside him is no bother");
        assert!(!buried_with(vec![Window { overlay: true, ..front.clone() }, own.clone()]), "an overlay is not watched");
        let furniture = crate::platform::desktop::FURNITURE[0].to_string();
        assert!(!buried_with(vec![Window { class: furniture, ..front }, own]), "the desktop's own furniture is where it is");
    }

    #[test]
    fn a_trip_comes_up_standing_on_the_other_monitor() {
        let tunnel = Tunnel { x: 300, monitor_bottom: -40, work_bottom: -88 };
        assert_eq!(below(tunnel.monitor_bottom, 4) + SPRITE_Y as i32 * 4, -40, "out of sight below its edge");
        assert_eq!(arrival(tunnel, 4), (300, -88 - H as i32 * 4), "feet on its taskbar line");
    }

    #[test]
    fn a_home_let_go_in_the_air_lands_on_the_edge_below() {
        let work = rect(0, -1400, 2560, -48);
        let ground = -48 - H as i32 * 4;
        assert_eq!(stand_on((900, -900), work, 4), (900, ground));
        assert_eq!(stand_on((2500, -900), work, 4), (2560 - width(4), ground), "kept on the monitor");
        let d = Desktop { monitors: vec![Monitor { id: MonitorId(1), whole: rect(0, -1440, 2560, 0), work }], ..Desktop::default() };
        assert_eq!(standing(&d, (900, -900), 4), (900, ground));
        let mut roam = Roam::new(1);
        roam.dropped((900, -900), (900, ground), 4, None);
        assert!(matches!(roam.state, State::Jump { to, onto: Onto::Edge, .. } if to == (900, ground)), "drops to the edge");
        roam.dropped((900, ground), (900, ground), 4, None);
        assert_eq!(roam.state, State::Home, "let go on the edge: already home");
    }

    #[test]
    fn home_again_after_a_seat_on_another_monitor() {
        let scale = 4;
        let here_work = rect(0, 0, 2560, 1392);
        let there_work = rect(2560, 0, 4480, 1032);
        let d = Desktop {
            monitors: vec![
                Monitor { id: MonitorId(1), whole: rect(0, 0, 2560, 1440), work: here_work },
                Monitor { id: MonitorId(2), whole: rect(2560, 0, 4480, 1080), work: there_work },
            ],
            ..Desktop::default()
        };
        let back = stand_on((there_work.left + 400, 0), there_work, scale);
        let mut home = stand_on((here_work.left + 400, 0), here_work, scale);
        let mut at = home;
        let mut roam = Roam::new(1);
        roam.chances = (0.0, 0.0, 0.0);
        roam.away = Some(back);
        let (mut arrival, mut piped) = (None, false);
        for _ in 0..400 {
            let f = frame(&d, at, home, true, false);
            let moved = roam.step(&f);
            piped |= moved.pipe.is_some();
            at = moved.to.unwrap_or(at);
            if let Some(new_home) = moved.home {
                (home, arrival) = (new_home, Some(moved.event));
                break;
            }
        }
        assert_eq!(arrival, Some(None), "came up without a word");
        assert!(piped, "down a pipe and up another");
        assert_eq!(home, back, "home is where it was before the seat");
        assert!(roam.away.is_none() && !roam.homeward);
    }

    #[test]
    #[ignore]
    fn fullscreen_on_each_monitor_live() {
        let desk = crate::platform::desktop::snapshot();
        for monitor in &desk.monitors {
            let whole = monitor.whole;
            let app = fullscreen_on(&desk, monitor).map(crate::net::process_of);
            println!("monitor at {},{} {}x{}: fullscreen {app:?}", whole.left, whole.top, whole.width(), whole.height());
        }
    }

    // The windows of other programs are not read on macOS, so there is
    // nothing there to sit on.
    #[test]
    #[ignore]
    #[cfg(not(target_os = "macos"))]
    fn perch_live() {
        let desk = crate::platform::desktop::snapshot();
        let target = desk.foreground.and_then(|id| desk.window(id)).expect("a window in front");
        let frame = target.frame;
        let scale = std::env::var("RACCY_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
        let feet = (frame.left + frame.width() / 3, frame.top);
        let here = (feet.0 - middle(scale), feet.1 - seat_line(scale));
        println!("target class={} frame={:?} feet={feet:?} scale={scale}", target.class, (frame.left, frame.top, frame.right, frame.bottom));
        for w in &desk.windows {
            let r = w.frame;
            let over = r.contains((feet.0, feet.1 + 1));
            let near = (r.left..r.right).contains(&feet.0) && (feet.1 - r.top).abs() <= PERCH_REACH * scale;
            if over || near {
                println!(
                    "{} class={} overlay={} topmost={} maximised={} frame={:?} near={near} seat={}",
                    if w.id == target.id { "TARGET" } else { "      " },
                    w.class,
                    w.overlay,
                    w.topmost,
                    w.maximised,
                    (r.left, r.top, r.right, r.bottom),
                    seat_frame(&desk, w, scale).is_some(),
                );
            }
            if w.id == target.id {
                break;
            }
        }
        let perch = perch_under(&desk, WindowId(0), here, scale);
        println!("perch: {}", perch.map_or("none".to_string(), |p| format!("offset {} on {:?}", p.seat.offset, desk.window(p.seat.window).map(|w| &w.class))));
    }

    #[test]
    #[ignore]
    fn seats_live() {
        let desk = crate::platform::desktop::snapshot();
        let scale = 4;
        for w in desk.windows.iter().filter(|w| !w.overlay) {
            let Some(work) = desk.monitor_of(&w.frame).map(|m| m.work) else { continue };
            let frame = w.frame;
            let why = match () {
                _ if crate::platform::desktop::FURNITURE.contains(&w.class.as_str()) => "not a seat".to_string(),
                _ if w.maximised => "maximised".to_string(),
                _ if frame.width() < SIT_MIN_WIDTH => "too narrow".to_string(),
                _ if frame.top - body_above(scale) < work.top => format!("no room above: top {} is {} below the monitor's top", frame.top, frame.top - work.top),
                _ => "a seat".to_string(),
            };
            println!("{:<22} frame=({}, {}, {}, {}) monitor_top={} {why}", crate::net::process_of(w.pid), frame.left, frame.top, frame.right, frame.bottom, work.top);
        }
    }

    #[test]
    fn fullscreen_mid_jump_lands_first_then_leaves() {
        let mut roam = Roam::new(1);
        roam.state = jump((0, 100), (0, 500), Onto::Edge, 4);
        let d = Desktop::default();
        let f = frame(&d, (0, 500), (0, 500), false, true);
        let mut landed = false;
        let left = (0..200).find(|_| {
            roam.step(&f);
            landed |= !matches!(roam.state, State::Jump { .. });
            assert!(landed || matches!(roam.state, State::Jump { .. }), "a jump is never cut short");
            matches!(roam.state, State::Leaving { .. })
        });
        assert!(left.is_some(), "never stuck: {:?}", roam.state);
    }

    #[test]
    fn sits_on_the_edge_with_legs_over_it() {
        let scale = 4;
        let (x, y) = sit_position(rect(500, 300, 1500, 900), 300, scale);
        assert_eq!(x + middle(scale), 800);
        assert_eq!(y + (SPRITE_Y + SIZE) as i32 * scale, 300 + LEGS_BELOW as i32 * scale);
    }

    #[test]
    fn sits_clear_of_the_caption_buttons() {
        assert_eq!(sit_offset(479), None);
        assert_eq!(sit_offset(480), Some(144));
        assert_eq!(sit_offset(2000), Some(600));
        for width in SIT_MIN_WIDTH..3000 {
            let offset = sit_offset(width).unwrap();
            assert!(offset >= SIT_LEFT_CLEARANCE && offset <= width - SIT_RIGHT_CLEARANCE);
        }
    }
}
