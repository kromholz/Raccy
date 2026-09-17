mod bubble;
mod glide;
mod menu;
mod roaming;

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use crate::clock::{self, clock_text, local_clock};
use crate::tuning::tuning;
use crate::lang::{self, Lang};
use crate::net::{self, Names, Tick};
use crate::pet::{Activity, Pet};
use crate::render::{self, Scene};
use crate::render::sprite::{self, Gait, Idle, Pose, Rgb, Visor};
use crate::platform::system::{hushed, idle_secs};
use crate::platform::{Desktop, Event, Rect};
use crate::talk::{Context, Talk};
use crate::watch::{Severity, Watch};
use crate::watch::{autoruns, lan, redirects, wire};
use crate::{fault, platform, roam, tools, trace, update};

use bubble::ToolPanel;
use glide::Glide;

const FRAME_MS: u32 = 100;
const VISOR_MOVE_FRAMES: u64 = 2;
const VISOR_AFTER_FORCED: u64 = 20;
const SAVE_EVERY_TICKS: u32 = 60;
const SQUASH_FRAMES: u32 = 2;
const HOBBY_SAID_AGAIN_FRAMES: u64 = 6000;
const WIRE_LOST_FRAMES: u32 = 150;
const GOOD_START_FRAMES: u64 = 600;

struct Line {
    text: String,
    frames_left: u32,
    alert: bool,
}

struct App {
    shell: platform::shell::Shell,
    ticks: Receiver<Tick>,
    names: Names,
    lan: Receiver<lan::Sighting>,
    autoruns: Receiver<autoruns::Seen>,
    redirects: Receiver<redirects::Look>,
    pet: Pet,
    talk: Talk,
    watch: Watch,
    wire: wire::Wire,
    scene: Scene,
    lang: Lang,
    scale: usize,
    panel: Option<ToolPanel>,
    tool_job: Option<Receiver<(tools::Report, Option<render::Qr>)>>,
    usage: tools::Usage,
    portrait: Option<Pose>,
    sized: Option<bool>,
    frame: u64,
    line: Option<Line>,
    busy: bool,
    feast: bool,
    morsel: Option<(Rgb, u32)>,
    led: Rgb,
    since_save: u32,
    petted_frames: u32,
    last_gait: Gait,
    last_facing: bool,
    glance_near: u32,
    glance_hold: Option<(Idle, u32)>,
    squash_frames: u32,
    idle: Option<(Idle, u32)>,
    next_idle: u32,
    hobby_said: Option<u64>,
    frames_since_tick: u32,
    wire_lost: bool,
    update_said: bool,
    strokes: Vec<u64>,
    roam: roam::Roam,
    desk: platform::Desktop,
    grabbed: bool,
    hushed: bool,
    fullscreen: Option<u32>,
    hidden: bool,
    glide: Option<Glide>,
    glider: Arc<platform::shell::Glider>,
    shown: render::Canvas,
    clipped: render::Canvas,
    ground: Option<i32>,
    pipe: Option<roam::Pipe>,
    pipe_from: u32,
    pipe_since: Instant,
    last_frame: Instant,
    frame_secs: f32,
    visor_down: bool,
    visor_until: u64,
    visor_moved: u64,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn at_ease(activity: Activity, feast: bool) -> bool {
    activity == Activity::Content || (activity == Activity::Eating && !feast)
}

fn pick_idle(roll: u64) -> (Idle, u32) {
    let mix = &tuning().life.idle_mix;
    let shares = [
        (Idle::LookLeft, 25, mix.look_left),
        (Idle::LookRight, 25, mix.look_right),
        (Idle::Yawn, 15, mix.yawn),
        (Idle::Stretch, 12, mix.stretch),
        (Idle::Tinker, 120, mix.tinker),
        (Idle::Read, 150, mix.read),
        (Idle::Game, 120, mix.game),
        (Idle::Snack, 90, mix.snack),
    ];
    let total: u64 = shares.iter().map(|&(_, _, share)| u64::from(share)).sum();
    let mut left = roll % total.max(1);
    for (idle, frames, share) in shares {
        if left < u64::from(share) {
            return (idle, frames);
        }
        left -= u64::from(share);
    }
    (Idle::Still, 0)
}

fn line_secs(text: &str, alert: bool) -> u32 {
    let base = if alert { tuning().talk.warn_secs } else { tuning().talk.line_secs };
    let extra = (text.chars().count().saturating_sub(40) / 20) as u32;
    (base + extra).min(2 * base)
}

fn puts_hobby_away(gait: Gait, alert: bool, activity: Activity) -> bool {
    matches!(gait, Gait::Walking | Gait::Jumping) || alert || activity == Activity::Sleeping
}

fn scale_for(dpi: u32) -> usize {
    ((4 * dpi) as f32 / 96.0).round().max(2.0) as usize
}

fn place_window(desk: &Desktop, saved: Option<(i32, i32)>, w: i32, h: i32) -> (i32, i32) {
    if let Some((x, y)) = saved
        && desk.on_a_monitor(&Rect::new(x, y, x + w, y + h))
    {
        return (x, y);
    }
    let main = desk.monitor_at((0, 0)).or_else(|| desk.monitors.first());
    main.map_or((0, 0), |m| (m.work.right - w, m.work.bottom - h))
}

// Called from the shell's own loop, never with the app borrowed.
pub(crate) fn dispatch(event: Event) {
    match event {
        Event::Frame => with_app(App::step),
        Event::Glide => with_app(App::glide),
        Event::Grabbed => with_app(|app| (app.grabbed, app.glide) = (true, None)),
        Event::Dropped => with_app(|app| {
            app.grabbed = false;
            trace::record(|| format!("grab moved=true home={:?} at={:?}", app.pet.pos, app.shell.pos()));
            app.dropped();
        }),
        Event::Clicked => with_app(|app| {
            app.grabbed = false;
            app.petted();
        }),
        Event::Files(path, more) => with_app(|app| app.inspect(path, more)),
        Event::Menu => menu::open(),
        Event::PanelClicked => with_app(App::close_panel),
        Event::PanelCopy => with_app(App::copy_panel),
        Event::PanelWheel(delta) => with_app(|app| app.wheel_panel(delta)),
        Event::Moved => with_app(App::follow_panel),
        Event::DisplayChanged => with_app(App::check_home),
        #[cfg(windows)]
        Event::Scaled { dpi, suggested } => with_app(|app| app.fit_dpi(dpi, suggested)),
        Event::SeatMoved(window) => with_app(|app| {
            if app.roam.seat_window() == Some(window) {
                app.follow_seat();
            }
        }),
        Event::AutostartDone { on, done } => with_app(|app| {
            let text = app.lang.autostart_line(on, done);
            app.say(text.into(), false);
        }),
        // The loop never ends normally at a logoff or shutdown: save now.
        Event::EndSession => {
            trace::record(|| "end of session".to_string());
            with_app(|app| {
                app.pet.told.clone_from(&app.talk.told);
                app.pet.save();
            });
            platform::net::stop_kernel_trace();
        }
        #[cfg(feature = "trace")]
        Event::TestCommand(id) => menu::command(id),
    }
}

fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|app| {
        if let Ok(mut app) = app.try_borrow_mut()
            && let Some(app) = app.as_mut()
        {
            f(app);
        }
    });
}

pub fn run() {
    {
        // Two Raccys would fight over one state file and one trace session. An
        // elevated restart may start while the old Raccy still runs: it asks
        // again for a while.
        let handover = std::env::args().any(|a| a == "--after");
        if !platform::shell::claim_instance(handover) {
            trace::record(|| format!("another Raccy is running, pid {} leaves", std::process::id()));
            return;
        }
        platform::shell::prepare_process();


        let scale = scale_for(platform::shell::system_dpi());
        let scene = Scene::new(scale);
        let (w, h) = scene.size();
        let (w, h) = (w as i32, h as i32);
        let mut pet = Pet::load();
        let lang = pet.lang.unwrap_or_else(Lang::detect);
        let desk = platform::desktop::snapshot();
        let (x, y) = roam::standing(&desk, place_window(&desk, pet.pos, w, h), scale as i32);
        pet.pos = Some((x, y));
        let Some(shell) = platform::shell::Shell::create(x, y, w, h) else { return };

        menu::refresh_autostart();
        update::watch();
        let (ticks, names) = net::start();
        let greeting = lang.greeting(clock::unix_now(), local_clock().0);
        let talk = Talk::remembering(pet.told.clone());
        APP.with(|app| {
            *app.borrow_mut() = Some(App {
                shell,
                ticks,
                names,
                lan: lan::start(),
                autoruns: autoruns::start(),
                redirects: redirects::start(),
                pet,
                talk,
                watch: Watch::default(),
                wire: wire::Wire::default(),
                scene,
                lang,
                scale,
                panel: None,
                tool_job: None,
                usage: tools::Usage::default(),
                portrait: None,
                sized: None,
                frame: 0,
                line: None,
                busy: false,
                feast: false,
                morsel: None,
                led: sprite::NEON_CYAN,
                since_save: 0,
                petted_frames: 0,
                last_gait: Gait::Still,
                last_facing: false,
                glance_near: 0,
                glance_hold: None,
                squash_frames: 0,
                idle: None,
                next_idle: tuning().life.idle_every_frames[0],
                hobby_said: None,
                frames_since_tick: 0,
                wire_lost: false,
                update_said: false,
                strokes: Vec::new(),
                roam: roam::Roam::new(clock::unix_now().wrapping_mul(0x9e37_79b9_7f4a_7c15)),
                desk,
                grabbed: false,
                hushed: false,
                fullscreen: None,
                hidden: false,
                glide: None,
                glider: platform::shell::Glider::start(shell),
                shown: render::Canvas { width: 0, height: 0, pixels: Vec::new() },
                clipped: render::Canvas { width: 0, height: 0, pixels: Vec::new() },
                ground: None,
                pipe: None,
                pipe_from: 0,
                pipe_since: Instant::now(),
                last_frame: Instant::now(),
                frame_secs: FRAME_MS as f32 / 1000.0,
                visor_down: false,
                visor_until: tuning().life.visor_up_frames.0,
                visor_moved: 0,
            })
        });
        trace::record(|| format!("start {} pid {}", env!("CARGO_PKG_VERSION"), std::process::id()));
        with_app(|app| {
            if app.pet.introduced {
                app.say(greeting, false);
            } else {
                app.introduce();
            }
            let crashes = fault::crashes();
            for (i, crash) in crashes.iter().enumerate() {
                let text = app.lang.crashed(&fault::place(crash));
                app.pet.note(clock::unix_now(), true, lang::polished(&format!("{text} ({crash})")), true);
                if i + 1 == crashes.len() {
                    app.say(text, false);
                    app.talk.hold();
                }
            }
        });
        let dpi = shell.dpi();
        with_app(|app| app.fit_dpi(dpi, None));
        with_app(App::step);
        shell.show(true);
        shell.run(FRAME_MS);
        platform::net::stop_kernel_trace();
        with_app(|app| app.pet.save());
    }
}

impl App {
    fn step(&mut self) {
        // Taken once so that everything in the frame sees the same monitors and the same windows.
        self.refresh_desk();
        let mut ticked = false;
        while let Ok(tick) = self.ticks.try_recv() {
            ticked = true;
            self.digest(tick);
        }
        self.watch_wire(ticked);
        self.watch_update();
        #[cfg(feature = "trace")]
        self.test_hooks();
        match self.tool_job.as_ref().map(|job| job.try_recv()) {
            Some(Ok((report, qr))) => {
                self.tool_job = None;
                self.show_report(report, qr);
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.close_panel();
                self.say(self.lang.tool_failed().into(), false);
            }
            _ => {}
        }
        self.advance();

        let activity = self.pet.activity();
        let roaming = if self.grabbed { roam::Move { ground: self.ground, ..roam::Move::default() } } else { self.roam_frame(activity) };
        let turned = roaming.gait == Gait::Walking && roaming.facing_left != self.last_facing;
        if (self.last_gait == Gait::Jumping && roaming.gait != Gait::Jumping) || turned {
            self.squash_frames = SQUASH_FRAMES;
        }
        self.last_gait = roaming.gait;
        self.last_facing = roaming.facing_left;
        // Roaming onto a monitor with another scale: the DPI message came while
        // the app was busy moving him, and was dropped.
        let dpi = self.shell.dpi();
        if scale_for(dpi) != self.scale && self.ground.is_none() {
            self.fit_dpi(dpi, None);
        }
        self.idle_moment(activity, &roaming);
        let alert = self.line.as_ref().is_some_and(|l| l.alert);
        let visor = self.visor(activity, alert);
        let glancing = self.glancing(&roaming, activity);
        let pose = Pose {
            activity,
            frame: self.frame,
            busy: self.busy,
            feast: self.feast,
            squash: self.squash_frames > 0,
            morsel: self.morsel.filter(|_| activity != Activity::Sleeping).map(|(c, _)| c),
            led: self.led,
            alert,
            petted: self.petted_frames > 0,
            stage: self.pet.stage(),
            neglect: self.pet.neglect,
            idle: roaming.look.or(self.idle.map(|(idle, _)| idle)).or(glancing).unwrap_or(Idle::Still),
            gait: roaming.gait,
            facing_left: roaming.facing_left,
            visor,
        };
        self.draw(&pose, &roaming);
        self.stay_on_top();
    }

    // A new Raccy is out. Said once, written in the journal, and from then on
    // it is a line in his menu until somebody clicks it.
    fn watch_update(&mut self) {
        // An attempt that came to nothing is said here, on the thread that
        // may speak, rather than by the thread that made it.
        if update::failed() {
            let text = self.lang.update_failed().to_string();
            self.say(text, false);
            self.talk.hold();
            self.update_said = false;
            return;
        }
        let Some(release) = update::waiting().filter(|_| !self.update_said) else { return };
        self.update_said = true;
        trace::record(|| format!("update {} offered", release.version));
        let text = lang::polished(&self.lang.update_found(&release.version, clock::unix_now()));
        self.pet.note(clock::unix_now(), false, text.clone(), !self.hushed);
        self.refresh_journal();
        self.say(text, false);
        self.talk.hold();
    }

    fn watch_wire(&mut self, ticked: bool) {
        if ticked {
            self.frames_since_tick = 0;
            if std::mem::take(&mut self.wire_lost) {
                trace::record(|| "ticks again".to_string());
                self.say(self.lang.wire_back().into(), false);
                self.talk.hold();
            }
            return;
        }
        self.frames_since_tick += 1;
        if self.frames_since_tick == WIRE_LOST_FRAMES {
            self.wire_lost = true;
            trace::record(|| format!("no tick for {WIRE_LOST_FRAMES} frames"));
            let text = lang::polished(self.lang.wire_lost());
            self.pet.note(clock::unix_now(), true, text.clone(), !self.hushed);
            self.refresh_journal();
            self.say(text, false);
            self.talk.hold();
        }
    }

    fn digest(&mut self, tick: Tick) {
        trace::log(|| {
            format!(
                "tick rx={} tx={} flows={} sized={} frame={} activity={:?}",
                tick.rx,
                tick.tx,
                tick.flows.len(),
                tick.sized,
                self.frame,
                self.pet.activity()
            )
        });
        self.sized = Some(tick.sized);
        self.pet.feed(&tick);
        if let Some(stage) = self.pet.level_up() {
            let text = lang::polished(self.lang.level_up(stage));
            let quiet = self.hushed || self.pet.muted_until.is_some();
            self.pet.note(clock::unix_now(), false, text.clone(), !quiet);
            if !quiet {
                self.say(text, false);
                self.talk.hold();
            }
            self.scene.jolt();
            self.refresh_journal();
        }
        let tastes = |incoming: bool| {
            tick.flows
                .iter()
                .map(|f| (sprite::flavour(f.conn.kind), if incoming { f.rx } else { f.tx }))
                .filter(|t| t.1 > 0)
                .collect()
        };
        let (rx, tx) = if self.fullscreen.is_some() { (0, 0) } else { (tick.rx, tick.tx) };
        self.scene.traffic(rx, tx, tastes(true), tastes(false));
        self.usage.record(&tick);
        self.busy = tick.rx + tick.tx > 4 * 1024;
        self.feast = tick.rx + tick.tx > tuning().talk.feast_bps;
        if let Some(conn) = tick.opened.last() {
            self.led = sprite::flavour(conn.kind);
        }

        let (hour, day) = local_clock();
        if let Some(until) = self.pet.muted_until.filter(|&t| clock::unix_now() >= t) {
            self.pet.muted_until = None;
            // Said when it runs out now, not when it ran out while he was closed,
            // nor over a fullscreen window.
            if clock::unix_now() - until < 5 && !self.hushed {
                self.say(self.lang.unmuted().into(), false);
                self.talk.hold();
            }
        }
        let muted = self.pet.muted_until.is_some();
        let cx = Context { idle_secs: idle_secs(), hushed: hushed() || self.fullscreen.is_some() || muted, hour, day, now: clock::unix_now() };
        self.hushed = cx.hushed;
        if at_ease(self.pet.activity(), self.feast) && self.line.is_none() {
            let night = tuning().life.is_night(hour);
            let life = &tuning().life;
            let (wait, chance) = if night { (life.nap_idle_night_secs, life.nap_one_in_night_secs) } else { (life.nap_idle_day_secs, life.nap_one_in_day_secs) };
            let roll = clock::unix_now().wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 33;
            if cx.idle_secs >= wait && roll.is_multiple_of(chance) {
                self.pet.nap((tuning().life.nap_min_secs + (roll >> 12) % (tuning().life.nap_max_secs - tuning().life.nap_min_secs)) as u32);
                trace::record(|| "nap".to_string());
                if !cx.hushed {
                    self.say(self.lang.nap(self.frame), false);
                    self.talk.hold();
                }
            }
        }
        self.pet.count_day(day, tick.rx, tick.tx);
        let learning = self.pet.learning();
        let mut found = self.watch.tick(&tick, &self.names, cx.idle_secs, &mut self.pet.known_processes, learning);
        while let Ok(sighting) = self.lan.try_recv() {
            trace::log(|| format!("lan look: {} machines", sighting.neighbours.len()));
            found.extend(lan::notice(&mut self.pet.networks, &sighting, cx.now));
        }
        found.extend(self.wire.tick(cx.now, platform::wire::reach()));
        while let Ok(seen) = self.autoruns.try_recv() {
            found.extend(autoruns::notice(&mut self.pet.autoruns, &seen));
        }
        while let Ok(look) = self.redirects.try_recv() {
            found.extend(redirects::notice(&mut self.pet.redirects, &look));
        }
        if let Some(said) = self.talk.tick(&tick, &self.names, &self.pet, found, cx, self.lang) {
            if said.severity == Some(Severity::Warn) {
                self.pet.wake_up();
            }
            self.say(said.text, said.severity == Some(Severity::Warn));
            if let Some(kind) = said.kind {
                let taste = sprite::flavour(kind);
                self.morsel = Some((taste, tuning().talk.line_secs * 10));
                self.led = taste;
            }
        }
        for host in std::mem::take(&mut self.talk.visits) {
            self.pet.visit(&host, day);
        }
        for (process, name) in std::mem::take(&mut self.talk.calls) {
            self.pet.call_home(day, &process, &name);
        }
        let noticed = std::mem::take(&mut self.talk.journal);
        if !noticed.is_empty() {
            self.pet.told.clone_from(&self.talk.told);
            for (warn, text, said) in noticed {
                self.pet.note(clock::unix_now(), warn, lang::polished(&text), said);
            }
            self.refresh_journal();
        }
        self.since_save += 1;
        if self.since_save >= SAVE_EVERY_TICKS {
            self.since_save = 0;
            self.pet.told.clone_from(&self.talk.told);
            self.pet.save();
        }
    }

    #[cfg(feature = "trace")]
    fn test_hooks(&mut self) {
        // RACCY_TEST_STEPS="60:journal;120:tool:Ping;200:pet;300:drop=C:\file.exe;400:quit",
        // by frame. Menu items go through the menu's own code, posted so they
        // run outside this borrow.
        if let Ok(steps) = std::env::var("RACCY_TEST_STEPS") {
            for step in steps.split(';') {
                let Some((at, action)) = step.split_once(':') else { continue };
                if at.trim().parse::<u64>().ok() != Some(self.frame) {
                    continue;
                }
                let action = action.trim();
                trace::log(|| format!("test step {action}"));
                match action {
                    "pet" => self.petted(),
                    "close" => self.close_panel(),
                    "nap" => {
                        self.pet.nap(60);
                        self.say(self.lang.nap(self.frame), false);
                    }
                    "stuffed" => self.pet.stuff_for_a_test(),
                    "fed" => self.pet.feed_for_a_test(),
                    "panic" => panic!("a test of the crash path"),
                    "hungry" => self.pet.satiety = 5.0,
                    _ if action.starts_with("home=") => {
                        let mut xy = action["home=".len()..].split(',').filter_map(|v| v.trim().parse::<i32>().ok());
                        if let (Some(x), Some(y)) = (xy.next(), xy.next()) {
                            self.shell.place(x, y);
                            self.glide = None;
                            self.dropped();
                        }
                    }
                    _ if action.starts_with("idle:") => {
                        let idle = match &action["idle:".len()..] {
                            "tinker" => Idle::Tinker,
                            "read" => Idle::Read,
                            "game" => Idle::Game,
                            _ => Idle::Snack,
                        };
                        self.idle = Some((idle, 150));
                        self.say(self.lang.hobby(idle, self.frame), false);
                    }
                    "menu" => self.shell.post_menu(),
                    _ if action.starts_with("say=") => self.say(action["say=".len()..].into(), false),
                    _ if action.starts_with("drop=") => self.inspect(action["drop=".len()..].into(), 0),
                    _ => match menu::test_command(action) {
                        Some(id) => self.shell.post_test_command(id),
                        None => trace::log(|| format!("test step unknown: {action}")),
                    },
                }
            }
        }
        if self.frame != 50 {
            return;
        }
        let wanted = std::env::var("RACCY_TEST_TOOL").unwrap_or_default();
        if let Some(tool) = tools::Tool::ALL.into_iter().find(|t| format!("{t:?}").eq_ignore_ascii_case(&wanted)) {
            self.use_tool(tool);
        }
        if wanted.eq_ignore_ascii_case("journal") {
            self.open_journal();
        }
        if std::env::var("RACCY_TEST_SCANLINES").is_ok_and(|v| v == "off") {
            self.pet.scanlines_off = true;
        }
        if let Ok(path) = std::env::var("RACCY_TEST_DROP") {
            self.inspect(path.into(), 0);
        }
    }

    fn advance(&mut self) {
        let gap = self.last_frame.elapsed().as_secs_f32().clamp(0.05, 0.2);
        self.last_frame = Instant::now();
        self.frame_secs += (gap - self.frame_secs) * 0.2;
        self.frame += 1;
        if self.frame == GOOD_START_FRAMES {
            fault::forgive();
        }
        if self.frame.is_multiple_of(50) {
            trace::log(|| format!("frame {} line={:?}", self.frame, self.line.as_ref().map(|l| l.frames_left)));
        }
        if let Some(line) = &mut self.line {
            line.frames_left = line.frames_left.saturating_sub(1);
            if line.frames_left == 0 {
                self.line = None;
            }
        }
        if let Some((_, left)) = &mut self.morsel {
            *left = left.saturating_sub(1);
        }
        if self.morsel.is_some_and(|(_, left)| left == 0) {
            self.morsel = None;
        }
        self.petted_frames = self.petted_frames.saturating_sub(1);
        self.squash_frames = self.squash_frames.saturating_sub(1);
    }

    fn idle_moment(&mut self, activity: Activity, roaming: &roam::Move) {
        if activity == Activity::Sleeping {
            self.hobby_said = None;
        }
        let alert = self.line.as_ref().is_some_and(|l| l.alert);
        if puts_hobby_away(roaming.gait, alert, activity) && self.idle.is_some_and(|(idle, _)| idle.is_hobby()) {
            trace::log(|| format!("idle put away {:?} gait={:?} alert={alert} activity={activity:?}", self.idle, roaming.gait));
            self.idle = None;
        }
        match &mut self.idle {
            Some((_, left)) if *left > 0 => *left -= 1,
            _ => {
                self.idle = None;
                let calm = at_ease(activity, self.feast) && self.line.is_none() && !matches!(roaming.gait, Gait::Walking | Gait::Jumping);
                if !calm {
                    return;
                }
                if self.next_idle > 0 {
                    self.next_idle -= 1;
                    return;
                }
                let hash = self.frame.wrapping_mul(0x9e37_79b9_7f4a_7c15);
                let [lo, hi] = tuning().life.idle_every_frames;
                self.next_idle = lo + ((hash >> 40) as u32) % hi.saturating_sub(lo).max(1);
                let idle = pick_idle(hash >> 20);
                self.idle = Some(idle);
                trace::log(|| format!("idle {:?} for {} frames", idle.0, idle.1));
                let due = self.hobby_said.is_none_or(|at| self.frame - at >= HOBBY_SAID_AGAIN_FRAMES) || (hash >> 50).is_multiple_of(3);
                if idle.0.is_hobby() && self.pet.muted_until.is_none() && !self.hushed && due {
                    self.hobby_said = Some(self.frame);
                    self.say(self.lang.hobby(idle.0, self.frame), false);
                    self.talk.hold();
                }
            }
        }
    }

    fn visor(&mut self, activity: Activity, alert: bool) -> Visor {
        let forced = match activity {
            _ if alert => Some(true),
            Activity::Stuffed => Some(true),
            Activity::Sleeping => Some(false),
            _ => None,
        };
        let down = forced.unwrap_or(if self.frame >= self.visor_until { !self.visor_down } else { self.visor_down });
        if down != self.visor_down {
            self.visor_down = down;
            self.visor_moved = self.frame;
            let (lo, hi) = if down { tuning().life.visor_down_frames } else { tuning().life.visor_up_frames };
            let hash = self.frame.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 32;
            self.visor_until = self.frame + lo + hash % (hi - lo);
            if forced.is_some() {
                self.visor_until = self.frame + VISOR_AFTER_FORCED;
            }
        }
        if forced.is_some() {
            self.visor_until = self.visor_until.max(self.frame + VISOR_AFTER_FORCED);
        }
        match () {
            _ if self.visor_moved > 0 && self.frame - self.visor_moved < VISOR_MOVE_FRAMES => Visor::Half,
            _ if self.visor_down => Visor::Down,
            _ => Visor::Up,
        }
    }

    fn draw(&mut self, pose: &Pose, roaming: &roam::Move) {
        self.portrait = Some(Pose { gait: Gait::Still, facing_left: false, ..*pose });
        if self.panel.is_some() {
            match self.hidden {
                true => self.close_panel(),
                false => {
                    self.refresh_card();
                    self.paint_panel();
                }
            }
        }
        self.scene.scanlines = !self.pet.scanlines_off;
        let line = self.line.as_ref().filter(|_| self.panel.is_none() && !roaming.underground).map(|l| (l.text.as_str(), l.alert));
        let canvas = self.scene.frame(pose, line);
        // Kept whole: gliding behind an edge shows it cut off at every step.
        self.shown.width = canvas.width;
        self.shown.height = canvas.height;
        self.shown.pixels.clone_from(&canvas.pixels);
        self.set_ground(roaming);
        self.present_shown();
    }

    fn say(&mut self, text: String, alert: bool) {
        let text = lang::polished(&text);
        trace::record(|| format!("say alert={alert} {text}"));
        let secs = line_secs(&text, alert);
        self.talk.showing(u64::from(secs));
        self.line = Some(Line { text, frames_left: secs * 10, alert });
    }

    fn introduce(&mut self) {
        self.say(self.lang.intro().into(), false);
        if let Some(line) = &mut self.line {
            line.frames_left = line.frames_left.max(tuning().talk.warn_secs * 10);
        }
        self.talk.hold();
        self.pet.introduced = true;
        self.pet.save();
    }

    // `suggested` is where Windows wants the window after a move to a monitor with another scale.
    fn fit_dpi(&mut self, dpi: u32, suggested: Option<Rect>) {
        let scale = scale_for(dpi);
        // Behind an edge his window hangs over the monitor below: its scale is
        // not his. He takes it up again once he is out.
        if (scale == self.scale && suggested.is_none()) || self.ground.is_some() {
            return;
        }
        let old_h = self.scene.size().1 as i32;
        let logical = self.logical_pos();
        if scale != self.scale {
            self.scale = scale;
            let flipped = self.scene.flipped;
            self.scene = Scene::new(scale);
            self.scene.flipped = flipped;
            self.rescale_panel();
        }
        let (w, h) = self.scene.size();
        self.glide = None;
        let rise = old_h - h as i32;
        if suggested.is_none()
            && let Some(home) = &mut self.pet.pos
        {
            home.1 += rise;
        }
        let (x, y) = suggested.map_or_else(|| (logical.0, logical.1 + rise + self.layout_offset()), |r| (r.left, r.top));
        self.shell.resize_to(x, y, w as i32, h as i32);
        self.stand_home();
    }

    fn muted_until(&self) -> Option<String> {
        self.pet.muted_until.filter(|&t| clock::unix_now() < t).map(clock_text)
    }

    fn mute(&mut self, until: u64) {
        self.pet.muted_until = Some(until);
        self.pet.save();
        self.say(self.lang.muted(&clock_text(until)), false);
    }

    fn unmute(&mut self) {
        self.pet.muted_until = None;
        self.pet.save();
        self.say(self.lang.unmuted().into(), false);
        self.talk.hold();
    }

    fn toggle_scanlines(&mut self) {
        self.pet.scanlines_off = !self.pet.scanlines_off;
        self.pet.save();
    }

    fn petted(&mut self) {
        let frame = self.frame;
        self.strokes.retain(|&f| frame - f < tuning().life.stroke_window_frames);
        self.strokes.push(frame);
        self.petted_frames = tuning().life.petted_frames;
        self.pet.pet();
        let text = self.lang.petted(self.strokes.len(), frame);
        trace::record(|| format!("petted strokes={}", self.strokes.len()));
        self.say(text, false);
        self.talk.hold();
    }

    fn switch_language(&mut self) {
        self.lang = self.lang.other();
        self.pet.lang = Some(self.lang);
        self.pet.save();
        self.relabel_panel();
        self.say(self.lang.greeting(self.frame, local_clock().0), false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Monitor, MonitorId};

    #[test]
    fn a_line_stays_up_longer_the_longer_it_is() {
        let (line, warn) = (tuning().talk.line_secs, tuning().talk.warn_secs);
        assert_eq!(line_secs("krátká", false), line);
        assert_eq!(line_secs(&"x".repeat(59), false), line);
        assert_eq!(line_secs(&"x".repeat(60), false), line + 1);
        assert_eq!(line_secs(&"ř".repeat(120), false), line + 4);
        assert_eq!(line_secs(&"x".repeat(1000), false), 2 * line, "capped");
        assert_eq!(line_secs(&"x".repeat(100), true), warn + 3);
    }

    #[test]
    fn idle_moments_come_in_the_shares_of_the_mix() {
        let mix = &tuning().life.idle_mix;
        let shares = [mix.look_left, mix.look_right, mix.yawn, mix.stretch, mix.tinker, mix.read, mix.game, mix.snack];
        let total: u64 = shares.iter().map(|&share| u64::from(share)).sum();
        let hobbies = (0..total).filter(|&roll| pick_idle(roll).0.is_hobby()).count() as u64;
        assert_eq!(hobbies, u64::from(mix.tinker + mix.read + mix.game + mix.snack));
        assert!((0..total).all(|roll| pick_idle(roll).1 > 0), "every moment lasts");
    }

    #[test]
    fn a_hobby_goes_on_sitting_on_a_window() {
        assert!(!puts_hobby_away(Gait::Sitting, false, Activity::Content), "up on a window");
        assert!(!puts_hobby_away(Gait::Still, false, Activity::Content));
        assert!(puts_hobby_away(Gait::Walking, false, Activity::Content));
        assert!(puts_hobby_away(Gait::Jumping, false, Activity::Content));
        assert!(puts_hobby_away(Gait::Sitting, true, Activity::Content), "a warning");
        assert!(puts_hobby_away(Gait::Sitting, false, Activity::Sleeping), "asleep");
    }

    #[test]
    fn he_is_drawn_bigger_on_a_screen_of_more_dots() {
        assert_eq!(scale_for(96), 4);
        assert_eq!(scale_for(120), 5, "a screen at 125 per cent");
        assert_eq!(scale_for(144), 6, "at 150");
        assert_eq!(scale_for(192), 8, "at 200");
        assert_eq!(scale_for(1), 2, "never so small he is a smudge");
    }

    #[test]
    fn he_comes_back_where_he_was_left_or_to_the_corner() {
        let monitor = |id: isize, whole: Rect, work: Rect| Monitor { id: MonitorId(id), whole, work };
        let desk = Desktop {
            monitors: vec![
                monitor(1, Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1032)),
                monitor(2, Rect::new(1920, 0, 3840, 1080), Rect::new(1920, 0, 3840, 1080)),
            ],
            windows: Vec::new(),
            foreground: None,
            cursor: None,
        };
        let (w, h) = (256, 256);
        assert_eq!(place_window(&desk, Some((300, 400)), w, h), (300, 400), "where he was left");
        assert_eq!(place_window(&desk, Some((3000, 200)), w, h), (3000, 200), "the other screen counts too");
        assert_eq!(place_window(&desk, Some((-4000, 400)), w, h), (1920 - w, 1032 - h));
        assert_eq!(place_window(&desk, None, w, h), (1920 - w, 1032 - h), "a first start");
        let nowhere = Desktop { monitors: Vec::new(), windows: Vec::new(), foreground: None, cursor: None };
        assert_eq!(place_window(&nowhere, None, w, h), (0, 0), "no screen at all");
    }

    #[test]
    fn a_trickle_leaves_him_at_ease_and_a_feed_does_not() {
        assert!(at_ease(Activity::Content, false));
        assert!(at_ease(Activity::Eating, false), "background traffic");
        assert!(!at_ease(Activity::Eating, true), "a real feed");
        for busy in [Activity::Hungry, Activity::Starving, Activity::Bored, Activity::Stuffed, Activity::Sleeping] {
            assert!(!at_ease(busy, false), "{busy:?}");
        }
    }
}
