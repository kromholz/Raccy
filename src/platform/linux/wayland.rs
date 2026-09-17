// A layer surface has no position of its own: anchored to the top left of a
// monitor, its margins are where it stands. Everything outside this file counts
// in screen pixels, so the margins are the wanted spot less the monitor corner,
// divided by the monitor scale.

use std::ffi::{CString, c_char, c_void};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer, wl_keyboard, wl_output, wl_pointer,
    wl_region, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::idle_notify::v1::client::{ext_idle_notification_v1, ext_idle_notifier_v1};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1};

use super::super::Event;
use super::desktop::{self, Screen};
use crate::render::{Canvas, Menu};
use crate::trace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Which {
    Pet,
    Panel,
    // The right-click menu, which a compositor does not open for us.
    Menu,
    Ground,
    // A strip along the bottom of a tiled monitor that the tiles stop above,
    // so that he stands on ground of his own rather than on somebody's work.
    Strip,
}

impl Which {
    fn index(self) -> usize {
        match self {
            Which::Pet => 0,
            Which::Panel => 1,
            Which::Menu => 2,
            Which::Ground => 3,
            Which::Strip => 4,
        }
    }
}

// The buttons the pointer sends, from the kernel's input codes.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const SOLID: u32 = 40;
// How tall he stands, in logical pixels, from the last frame drawn: the
// strip he asks for on a tiled monitor is exactly that and no more.
pub(super) static BODY_LOGICAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
const SCREENS_FOR: Duration = Duration::from_millis(500);

pub(super) struct Wayland {
    conn: Connection,
    queue: EventQueue<State>,
    pub(super) state: State,
}

// Nothing here calls into the app: the events the handlers gather are handed
// to the loop, which dispatches them once the queue is done with.
pub(super) struct State {
    qh: QueueHandle<State>,
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    layer_shell: ZwlrLayerShellV1,
    viewporter: Option<wp_viewporter::WpViewporter>,
    outputs: Vec<(wl_output::WlOutput, String)>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    _data_device: Option<wl_data_device::WlDataDevice>,
    offer: Option<(wl_data_offer::WlDataOffer, bool)>,
    drop_on: Option<Which>,
    // Held so that the compositor keeps telling us when nobody is there.
    _idle: Option<ext_idle_notification_v1::ExtIdleNotificationV1>,
    wins: [Option<Win>; 5],
    // `Some(None)` when it was shut without a choice.
    menu: Option<Menu>,
    menu_done: Option<Option<usize>>,
    over: Option<Which>,
    over_at: (i32, i32),
    drag: Option<Drag>,
    wheel: f64,
    awaiting_frame: bool,
    events: Vec<Event>,
    screens: Vec<Screen>,
    screens_at: Instant,
}

struct Drag {
    offset: (i32, i32),
    moved: bool,
}

impl Wayland {
    pub(super) fn connect() -> Option<Wayland> {
        let conn = Connection::connect_to_env().ok()?;
        let (globals, queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).ok()?;
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).ok()?;
        let layer_shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).ok()?;
        let viewporter: Option<wp_viewporter::WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).ok()?;
        // A compositor without the idle-notify protocol leaves him thinking someone always is.
        let idle: Option<ext_idle_notifier_v1::ExtIdleNotifierV1> = globals.bind(&qh, 1..=1, ()).ok();
        let idle_notification = idle.as_ref().map(|idle| idle.get_idle_notification(super::system::IDLE_AFTER_SECS as u32 * 1000, &seat, &qh, ()));
        let data: Option<wl_data_device_manager::WlDataDeviceManager> = globals.bind(&qh, 1..=3, ()).ok();
        let data_device = data.as_ref().map(|data| data.get_data_device(&seat, &qh, ()));
        let mut outputs = Vec::new();
        for global in globals.contents().clone_list() {
            if global.interface == wl_output::WlOutput::interface().name {
                let version = global.version.min(4);
                let output: wl_output::WlOutput = globals.registry().bind(global.name, version, &qh, outputs.len());
                outputs.push((output, String::new()));
            }
        }
        let state = State {
            qh,
            compositor,
            shm,
            layer_shell,
            viewporter,
            outputs,
            pointer: None,
            keyboard: None,
            _data_device: data_device,
            offer: None,
            drop_on: None,
            _idle: idle_notification,
            wins: [const { None }; 5],
            menu: None,
            menu_done: None,
            over: None,
            over_at: (0, 0),
            drag: None,
            wheel: 0.0,
            awaiting_frame: false,
            events: Vec::new(),
            screens: Vec::new(),
            screens_at: Instant::now() - SCREENS_FOR,
        };
        let mut wayland = Wayland { conn, queue, state };
        // The outputs' names and the seat's pointer come with the first
        // round trip, before any window is made.
        wayland.queue.roundtrip(&mut wayland.state).ok()?;
        trace::record(|| format!("wayland: {} outputs, pointer={}", wayland.state.outputs.len(), wayland.state.pointer.is_some()));
        Some(wayland)
    }

    pub(super) fn pump(&mut self, wait: Duration) -> Vec<Event> {
        let _ = self.queue.flush();
        let _ = self.queue.dispatch_pending(&mut self.state);
        if let Some(guard) = self.conn.prepare_read()
            && readable(self.conn.as_fd().as_raw_fd(), wait)
        {
            let _ = guard.read();
        }
        let _ = self.queue.dispatch_pending(&mut self.state);
        self.state.follow_pointer();
        std::mem::take(&mut self.state.events)
    }

    pub(super) fn ready_for_frame(&self) -> bool {
        !self.state.awaiting_frame
    }
}

impl State {
    fn screens(&mut self) -> &[Screen] {
        if self.screens.is_empty() || self.screens_at.elapsed() >= SCREENS_FOR {
            let screens = desktop::screens();
            if !screens.is_empty() {
                self.screens = screens;
            }
            self.screens_at = Instant::now();
        }
        &self.screens
    }

    fn win(&mut self, which: Which) -> Option<&mut Win> {
        self.wins[which.index()].as_mut()
    }

    pub(super) fn open(&mut self, which: Which, at: (i32, i32), size: (i32, i32)) {
        self.close(which);
        let Some(screen) = desktop::screen_of_device(self.screens(), at).cloned() else {
            return;
        };
        let output = self.outputs.iter().find(|(_, name)| *name == screen.name).map(|(o, _)| o.clone());
        let surface = self.compositor.create_surface(&self.qh, ());
        let name = match which {
            Which::Pet => "raccy",
            Which::Panel => "raccy-panel",
            Which::Menu => "raccy-menu",
            Which::Ground => "raccy-ground",
            Which::Strip => "raccy-strip",
        };
        let layer = self.layer_shell.get_layer_surface(&surface, output.as_ref(), Layer::Overlay, name.to_string(), &self.qh, ());
        layer.set_anchor(Anchor::Top | Anchor::Left);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(match which {
            Which::Menu => KeyboardInteractivity::Exclusive,
            _ => KeyboardInteractivity::None,
        });
        // A buffer drawn in the screen's own pixels, shown at the size the
        // layout wants: that is the only way a scale of 1.25 or 1.5 comes
        // out sharp. Without the viewport there is only the whole-number
        // scale the surface itself can carry.
        let viewport = self.viewporter.as_ref().map(|v| v.get_viewport(&surface, &self.qh, ()));
        let win = Win {
            surface,
            layer,
            viewport,
            output: screen.name.clone(),
            origin: (screen.device.left, screen.device.top),
            scale: screen.scale.max(1.0),
            at,
            size,
            pool: None,
            configured: false,
            shown: true,
            drawn: None,
        };
        if win.viewport.is_none() {
            win.surface.set_buffer_scale(win.scale.round() as i32);
        }
        win.tell_size();
        win.surface.commit();
        self.wins[which.index()] = Some(win);
        trace::record(|| format!("wayland: {which:?} on {} at {at:?} size {size:?}", screen.name));
    }

    pub(super) fn close(&mut self, which: Which) {
        let gone = self.wins[which.index()].take();
        if let Some(win) = gone {
            win.layer.destroy();
            win.surface.destroy();
        }
    }

    // Off its monitor and onto another, the surface is made anew: a layer
    // surface belongs to the monitor it was made on.
    pub(super) fn place(&mut self, which: Which, at: (i32, i32)) {
        let Some(win) = self.win(which) else { return };
        if win.at == at {
            return;
        }
        win.at = at;
        let (size, output) = (win.size, win.output.clone());
        let middle = (at.0 + size.0 / 2, at.1 + size.1 / 2);
        if desktop::screen_of_device(self.screens(), middle).is_some_and(|s| s.name != output) {
            self.open(which, at, size);
            return;
        }
        if let Some(win) = self.win(which) {
            win.tell_size();
            win.surface.commit();
        }
    }

    pub(super) fn present(&mut self, which: Which, canvas: &Canvas) {
        let (shm, qh, compositor) = (self.shm.clone(), self.qh.clone(), self.compositor.clone());
        let State { wins, awaiting_frame, .. } = self;
        let Some(win) = wins[which.index()].as_mut() else { return };
        let awaiting = awaiting_frame;
        let size = (canvas.width as i32, canvas.height as i32);
        if win.size != size {
            win.size = size;
            win.tell_size();
        }
        if win.pool.as_ref().is_none_or(|p| p.size != size) {
            win.pool = Pool::new(&shm, &qh, size);
        }
        let Some(pool) = win.pool.as_mut() else { return };
        let Some(index) = pool.free_one() else { return };
        pool.write(index, canvas);
        win.drawn = Some(index);
        match which {
            Which::Menu => win.surface.set_input_region(None),
            _ => {
                if which == Which::Pet {
                    // The tallest he has stood, not this frame's pose: a strip that
                    // grew and shrank with every hop would have the layout dancing.
                    let body = ((canvas.height - solid_top(canvas)) as f64 / win.scale).round() as i32;
                    BODY_LOGICAL.fetch_max(body, std::sync::atomic::Ordering::Relaxed);
                }
                win.mark_input(&compositor, &qh, canvas)
            }
        }
        if win.configured && win.shown {
            win.attach(index);
            if which == Which::Pet {
                win.surface.frame(&qh, ());
                *awaiting = true;
            }
            win.surface.commit();
        }
    }

    pub(super) fn show(&mut self, which: Which, shown: bool) {
        let Some(win) = self.win(which) else { return };
        if win.shown == shown {
            return;
        }
        win.shown = shown;
        match (shown, win.drawn) {
            (true, Some(index)) => win.attach(index),
            _ => win.surface.attach(None, 0, 0),
        }
        win.surface.commit();
    }

    // The monitor he is drawn on, and its scale.
    pub(super) fn pet_output(&self) -> Option<(String, f64)> {
        self.wins[Which::Pet.index()].as_ref().map(|w| (w.output.clone(), w.scale))
    }

    // Anchored to the bottom edge and asking for that much room, which is
    // how a bar keeps windows off itself. See-through, and no pixel of it
    // takes a click: it is ground, not a thing.
    pub(super) fn open_strip(&mut self, screen: &desktop::Screen, logical_height: i32) {
        let height = (logical_height as f64 * screen.scale).round() as i32;
        self.open(Which::Strip, (screen.device.left, screen.device.bottom - height), (screen.device.width(), height));
        let compositor = self.compositor.clone();
        let qh = self.qh.clone();
        let Some(win) = self.wins[Which::Strip.index()].as_mut() else { return };
        win.layer.set_anchor(Anchor::Bottom | Anchor::Left | Anchor::Right);
        win.layer.set_exclusive_zone(logical_height);
        self.cover(Which::Strip);
        if let Some(win) = self.wins[Which::Strip.index()].as_mut() {
            let nothing = compositor.create_region(&qh, ());
            win.surface.set_input_region(Some(&nothing));
            nothing.destroy();
            win.surface.commit();
        }
    }

    pub(super) fn at(&self, which: Which) -> Option<(i32, i32)> {
        self.wins[which.index()].as_ref().map(|w| w.at)
    }

    pub(super) fn dpi(&mut self) -> Option<u32> {
        let at = self.at(Which::Pet)?;
        let scale = desktop::screen_of_device(self.screens(), at)?.scale;
        Some((96.0 * scale).round() as u32)
    }

    // While he is held, the compositor is asked where the pointer is: the
    // surface moves under it, so its own coordinates are no use.
    fn follow_pointer(&mut self) {
        let Some(drag) = self.drag.as_ref() else { return };
        let offset = drag.offset;
        let Some(cursor) = desktop::cursor(self.screens()) else { return };
        let to = (cursor.0 - offset.0, cursor.1 - offset.1);
        if self.at(Which::Pet) == Some(to) {
            return;
        }
        self.place(Which::Pet, to);
        if let Some(drag) = self.drag.as_mut() {
            drag.moved = true;
        }
        self.events.push(Event::Moved);
    }

    fn button(&mut self, button: u32, pressed: bool) {
        match (self.over, button, pressed) {
            (Some(Which::Pet), BTN_LEFT, true) => {
                let at = self.at(Which::Pet).unwrap_or_default();
                let cursor = desktop::cursor(self.screens()).unwrap_or(at);
                self.drag = Some(Drag { offset: (cursor.0 - at.0, cursor.1 - at.1), moved: false });
                self.events.push(Event::Grabbed);
            }
            (_, BTN_LEFT, false) if self.drag.is_some() => {
                let moved = self.drag.take().is_some_and(|drag| drag.moved);
                self.events.push(if moved { Event::Dropped } else { Event::Clicked });
            }
            // The menu opens when the button comes up, as it does on
            // Windows, so its own release is not what shuts it again.
            (Some(Which::Pet), BTN_RIGHT, false) => self.events.push(Event::Menu),
            (Some(Which::Menu), BTN_LEFT | BTN_RIGHT, false) => {
                let point = self.over_at;
                if let Some(chosen) = self.menu.as_ref().and_then(|menu| menu.row_at(point).map(|row| menu.id(row))) {
                    self.menu_done = Some(chosen);
                }
            }
            (Some(Which::Ground), BTN_LEFT | BTN_RIGHT, false) => self.menu_done = Some(None),
            (Some(Which::Panel), BTN_LEFT, false) => self.events.push(Event::PanelClicked),
            (Some(Which::Panel), BTN_RIGHT, false) => self.events.push(Event::PanelCopy),
            _ => {}
        }
    }

    // Wayland counts a notch as fifteen units down; Raccy counts a hundred
    // and twenty up, as Windows does.
    fn wheel(&mut self, value: f64) {
        if self.over != Some(Which::Panel) {
            return;
        }
        self.wheel += value * -8.0;
        let whole = self.wheel.trunc();
        self.wheel -= whole;
        if whole != 0.0 {
            self.events.push(Event::PanelWheel(whole as i32));
        }
    }

    fn surface_of(&self, is: impl Fn(&Win) -> bool) -> Option<Which> {
        [Which::Pet, Which::Panel, Which::Menu, Which::Ground].into_iter().find(|which| self.wins[which.index()].as_ref().is_some_and(&is))
    }

    fn pointer_at(&mut self, x: f64, y: f64) {
        if self.over != Some(Which::Menu) {
            return self.pointer_off();
        }
        let scale = self.win(Which::Menu).map_or(1.0, |w| w.scale);
        self.over_at = ((x * scale) as i32, (y * scale) as i32);
        let at = self.over_at;
        if let Some(menu) = self.menu.as_mut() {
            let row = menu.row_at(at);
            menu.set_hover(row);
        }
    }

    fn pointer_off(&mut self) {
        if let Some(menu) = self.menu.as_mut() {
            menu.set_hover(None);
        }
    }

    // The ground over the whole monitor is made before the box, so the box
    // stands on top of it.
    pub(super) fn open_menu(&mut self, items: &[crate::render::MenuItem], scale: usize) -> bool {
        let screens = self.screens().to_vec();
        let Some(cursor) = desktop::cursor(&screens).or_else(|| self.at(Which::Pet)) else { return false };
        let Some(screen) = desktop::screen_of_device(&screens, cursor).cloned() else { return false };
        let menu = Menu::new(scale, items);
        let size = menu.size();
        let at = (
            cursor.0.clamp(screen.device.left, (screen.device.right - size.0).max(screen.device.left)),
            cursor.1.clamp(screen.device.top, (screen.device.bottom - size.1).max(screen.device.top)),
        );
        self.over_at = (0, 0);
        self.menu_done = None;
        self.menu = Some(menu);
        self.open(Which::Ground, (screen.device.left, screen.device.top), (screen.device.width(), screen.device.height()));
        self.cover(Which::Ground);
        self.open(Which::Menu, at, size);
        trace::record(|| format!("wayland: menu at {at:?} size {size:?} on {}", screen.name));
        true
    }

    // Fills a window with nothing at all: one see-through pixel, which the
    // viewport stretches over the whole of it. The ground under the menu
    // is that, so covering a monitor costs four bytes and not eight
    // megabytes. A compositor without the viewporter has to have a buffer
    // the size it asked for.
    fn cover(&mut self, which: Which) {
        let (shm, qh) = (self.shm.clone(), self.qh.clone());
        let Some(win) = self.wins[which.index()].as_mut() else { return };
        let size = if win.viewport.is_some() { (1, 1) } else { win.size };
        win.pool = Pool::new(&shm, &qh, size);
        let Some(pool) = win.pool.as_mut() else { return };
        // Shared memory comes back zeroed, and a zero pixel is see-through.
        pool.taken(0);
        win.drawn = Some(0);
        win.surface.set_input_region(None);
        if win.configured && win.shown {
            win.attach(0);
            win.surface.commit();
        }
    }

    pub(super) fn draw_menu(&mut self) {
        let Some(mut menu) = self.menu.take() else { return };
        if let Some(canvas) = menu.frame() {
            self.present(Which::Menu, canvas);
        }
        self.menu = Some(menu);
    }

    pub(super) fn menu_done(&self) -> Option<Option<usize>> {
        self.menu_done
    }

    pub(super) fn close_menu(&mut self) -> Option<usize> {
        let chosen = self.menu_done.take().flatten();
        self.menu = None;
        self.close(Which::Menu);
        self.close(Which::Ground);
        chosen
    }
}

struct Win {
    surface: wl_surface::WlSurface,
    layer: ZwlrLayerSurfaceV1,
    // `None` on a compositor without the viewporter, where the surface
    // carries a whole-number scale instead.
    viewport: Option<wp_viewport::WpViewport>,
    // The monitor it was made on, and that monitor's corner in screen
    // pixels: its margins are counted from there.
    output: String,
    origin: (i32, i32),
    // Screen pixels per logical pixel on that monitor, 1.5 and all.
    scale: f64,
    at: (i32, i32),
    size: (i32, i32),
    pool: Option<Pool>,
    configured: bool,
    shown: bool,
    // The buffer last drawn into, kept to show again after a configure.
    drawn: Option<usize>,
}

impl Win {
    // The size and the spot, both in the layout's logical pixels, and the
    // size the buffer is shown at, which is the same.
    fn tell_size(&self) {
        let logical = |n: i32| (n as f64 / self.scale).round() as i32;
        let (w, h) = (logical(self.size.0).max(1), logical(self.size.1).max(1));
        self.layer.set_size(w as u32, h as u32);
        // Margins may be negative: he goes behind the edges of the screen.
        self.layer.set_margin(logical(self.at.1 - self.origin.1), 0, 0, logical(self.at.0 - self.origin.0));
        if let Some(viewport) = &self.viewport {
            viewport.set_destination(w, h);
        }
    }

    fn attach(&self, index: usize) {
        let Some(pool) = self.pool.as_ref() else { return };
        self.surface.attach(Some(&pool.buffers[index]), 0, 0);
        self.surface.damage_buffer(0, 0, pool.size.0, pool.size.1);
    }

    fn mark_input(&self, compositor: &wl_compositor::WlCompositor, qh: &QueueHandle<State>, canvas: &Canvas) {
        let region = compositor.create_region(qh, ());
        // A region is in the surface's logical pixels, which is the canvas
        // divided by the monitor's own scale, 1.5 and all. The scan takes
        // whole rows of the canvas at a time, which is a separate number.
        let logical = |n: usize| (n as f64 / self.scale).floor() as i32;
        let step = self.scale.ceil().max(1.0) as usize;
        let mut y = 0;
        while y < canvas.height {
            let (mut left, mut right) = (usize::MAX, 0usize);
            for row in y..(y + step).min(canvas.height) {
                for (x, p) in canvas.pixels[row * canvas.width..(row + 1) * canvas.width].iter().enumerate() {
                    if p >> 24 >= SOLID {
                        left = left.min(x);
                        right = right.max(x);
                    }
                }
            }
            if left != usize::MAX {
                let (x, w) = (logical(left), logical(right) - logical(left) + 1);
                region.add(x, logical(y), w, (logical(y + step) - logical(y)).max(1));
            }
            y += step;
        }
        self.surface.set_input_region(Some(&region));
        region.destroy();
    }
}

// The first row with anything solid in it within the sprite's own box, so
// that a bubble beside him or a hop above his line does not count as him.
fn solid_top(canvas: &Canvas) -> usize {
    let unit = canvas.width / crate::render::W;
    let (x0, y0) = (crate::render::SPRITE_X * unit, crate::render::SPRITE_Y * unit);
    (y0..canvas.height)
        .find(|&y| canvas.pixels[y * canvas.width + x0..(y + 1) * canvas.width].iter().any(|p| p >> 24 >= SOLID))
        .unwrap_or(canvas.height)
}

struct Pool {
    map: *mut u8,
    len: usize,
    pool: wl_shm_pool::WlShmPool,
    buffers: Vec<wl_buffer::WlBuffer>,
    // Whether the compositor has given each buffer back.
    free: Vec<bool>,
    size: (i32, i32),
    _fd: OwnedFd,
}

// The whole of the Wayland side lives on the thread that runs the loop;
// the mapping is only ever touched from there.
unsafe impl Send for Pool {}

impl Pool {
    fn new(shm: &wl_shm::WlShm, qh: &QueueHandle<State>, size: (i32, i32)) -> Option<Pool> {
        let (stride, height) = (size.0 * 4, size.1);
        let one = (stride * height) as usize;
        let len = one * 2;
        let name = CString::new("raccy").ok()?;
        let fd = unsafe { memfd_create(name.as_ptr(), MFD_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        if unsafe { ftruncate(fd.as_raw_fd(), len as i64) } != 0 {
            return None;
        }
        let map = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_SHARED, fd.as_raw_fd(), 0) };
        if map == MAP_FAILED {
            return None;
        }
        let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
        let buffers: Vec<wl_buffer::WlBuffer> = (0..2)
            .map(|i| pool.create_buffer((i * one) as i32, size.0, size.1, stride, wl_shm::Format::Argb8888, qh, ()))
            .collect();
        Some(Pool { map: map as *mut u8, len, pool, buffers, free: vec![true; 2], size, _fd: fd })
    }

    fn free_one(&self) -> Option<usize> {
        self.free.iter().position(|free| *free)
    }

    // Both are premultiplied and in the same order, so the rows go straight
    // across.
    fn write(&mut self, index: usize, canvas: &Canvas) {
        let one = self.len / self.buffers.len();
        let bytes = canvas.pixels.len() * 4;
        if bytes > one {
            return;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(canvas.pixels.as_ptr() as *const u8, self.map.add(index * one), bytes);
        }
        self.taken(index);
    }

    fn taken(&mut self, index: usize) {
        self.free[index] = false;
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        for buffer in &self.buffers {
            buffer.destroy();
        }
        self.pool.destroy();
        unsafe { munmap(self.map as *mut c_void, self.len) };
    }
}

const MFD_CLOEXEC: u32 = 1;
const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_SHARED: i32 = 1;
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;
const POLLIN: i16 = 1;

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

unsafe extern "C" {
    fn memfd_create(name: *const c_char, flags: u32) -> i32;
    fn ftruncate(fd: i32, length: i64) -> i32;
    fn mmap(addr: *mut c_void, length: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut c_void;
    fn munmap(addr: *mut c_void, length: usize) -> i32;
    fn poll(fds: *mut PollFd, count: u64, timeout: i32) -> i32;
    fn pipe2(ends: *mut i32, flags: i32) -> i32;
}

fn readable(fd: i32, wait: Duration) -> bool {
    let mut fds = PollFd { fd, events: POLLIN, revents: 0 };
    let millis = wait.as_millis().min(i32::MAX as u128) as i32;
    unsafe { poll(&mut fds, 1, millis) > 0 }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _state: &mut State,
        _registry: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _globals: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<State>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, usize> for State {
    fn event(state: &mut State, _output: &wl_output::WlOutput, event: wl_output::Event, index: &usize, _conn: &Connection, _qh: &QueueHandle<State>) {
        if let wl_output::Event::Name { name } = event
            && let Some(slot) = state.outputs.get_mut(*index)
        {
            slot.1 = name;
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(state: &mut State, seat: &wl_seat::WlSeat, event: wl_seat::Event, _: &(), _conn: &Connection, qh: &QueueHandle<State>) {
        let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event else { return };
        trace::record(|| format!("wayland: seat offers {caps:?}, pointer held {}", state.pointer.is_some()));
        // A device that goes away takes its object with it: one held on to
        // through a re-plug, or through wayvnc's pointer that lives only as
        // long as a client, never delivers another event. It is let go here
        // so that the next offer gets a fresh one.
        if !caps.contains(wl_seat::Capability::Pointer)
            && let Some(pointer) = state.pointer.take()
        {
            pointer.release();
        }
        if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
            state.pointer = Some(seat.get_pointer(qh, ()));
        }
        if !caps.contains(wl_seat::Capability::Keyboard)
            && let Some(keyboard) = state.keyboard.take()
        {
            keyboard.release();
        }
        if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
            state.keyboard = Some(seat.get_keyboard(qh, ()));
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(state: &mut State, _pointer: &wl_pointer::WlPointer, event: wl_pointer::Event, _: &(), _conn: &Connection, _qh: &QueueHandle<State>) {
        match event {
            wl_pointer::Event::Enter { surface, surface_x, surface_y, .. } => {
                state.over = state.surface_of(|w| w.surface.id() == surface.id());
                state.pointer_at(surface_x, surface_y);
            }
            wl_pointer::Event::Leave { .. } => {
                state.over = None;
                state.pointer_off();
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => state.pointer_at(surface_x, surface_y),
            wl_pointer::Event::Button { button, state: WEnum::Value(pressed), .. } => {
                trace::record(|| format!("wayland: button {button} {pressed:?}"));
                state.button(button, pressed == wl_pointer::ButtonState::Pressed);
            }
            wl_pointer::Event::Axis { axis: WEnum::Value(wl_pointer::Axis::VerticalScroll), value, .. } => state.wheel(value),
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(state: &mut State, layer: &ZwlrLayerSurfaceV1, event: zwlr_layer_surface_v1::Event, _: &(), _conn: &Connection, _qh: &QueueHandle<State>) {
        let Some(which) = state.surface_of(|w| w.layer.id() == layer.id()) else { return };
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                layer.ack_configure(serial);
                let Some(win) = state.win(which) else { return };
                win.configured = true;
                match win.drawn {
                    Some(index) if win.shown => win.attach(index),
                    _ => {}
                }
                win.surface.commit();
            }
            zwlr_layer_surface_v1::Event::Closed => state.close(which),
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(state: &mut State, buffer: &wl_buffer::WlBuffer, event: wl_buffer::Event, _: &(), _conn: &Connection, _qh: &QueueHandle<State>) {
        if !matches!(event, wl_buffer::Event::Release) {
            return;
        }
        for win in state.wins.iter_mut().flatten() {
            if let Some(pool) = win.pool.as_mut()
                && let Some(index) = pool.buffers.iter().position(|b| b.id() == buffer.id())
            {
                pool.free[index] = true;
            }
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(state: &mut State, _callback: &wl_callback::WlCallback, _event: wl_callback::Event, _: &(), _conn: &Connection, _qh: &QueueHandle<State>) {
        state.awaiting_frame = false;
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore wl_region::WlRegion);
delegate_noop!(State: ignore ZwlrLayerShellV1);

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, ()> for State {
    fn event(
        _state: &mut State,
        _notification: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<State>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => super::system::set_idle(true),
            ext_idle_notification_v1::Event::Resumed => super::system::set_idle(false),
            _ => {}
        }
    }
}

delegate_noop!(State: ignore ext_idle_notifier_v1::ExtIdleNotifierV1);

// A drop arrives as a list of addresses through a pipe, one a line, with the
// awkward characters written as %20 and the like.
fn dropped_files(text: &str) -> Vec<std::path::PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("file://"))
        .map(|path| std::path::PathBuf::from(unescape(path)))
        .collect()
}

fn unescape(text: &str) -> String {
    let mut out = Vec::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], bytes.get(i + 1).zip(bytes.get(i + 2))) {
            (b'%', Some((high, low))) => match u8::from_str_radix(&format!("{}{}", *high as char, *low as char), 16) {
                Ok(byte) => {
                    out.push(byte);
                    i += 3;
                }
                Err(_) => {
                    out.push(b'%');
                    i += 1;
                }
            },
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_offer(offer: &wl_data_offer::WlDataOffer, conn: &Connection) -> Option<String> {
    let mut ends = [0i32; 2];
    if unsafe { pipe2(ends.as_mut_ptr(), O_CLOEXEC) } != 0 {
        return None;
    }
    let (read_end, write_end) = unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
    offer.receive(URI_LIST.to_string(), write_end.as_fd());
    // The other side cannot start writing until the compositor has heard.
    let _ = conn.flush();
    drop(write_end);
    // The program dropping the file writes the list. It may write nothing and
    // never close, and this runs inside the loop that draws him, so it waits
    // for each piece rather than for the end of the file.
    let mut text = Vec::new();
    let mut file = std::fs::File::from(read_end);
    let until = Instant::now() + Duration::from_millis(500);
    let mut buf = [0u8; 4096];
    while text.len() < 64 * 1024 {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() || !readable(file.as_raw_fd(), left) {
            break;
        }
        match std::io::Read::read(&mut file, &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => text.extend_from_slice(&buf[..n]),
        }
    }
    Some(String::from_utf8_lossy(&text).into_owned())
}

const URI_LIST: &str = "text/uri-list";
const O_CLOEXEC: i32 = 0o2000000;

impl Dispatch<wl_data_device::WlDataDevice, ()> for State {
    fn event(
        state: &mut State,
        _device: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        conn: &Connection,
        _qh: &QueueHandle<State>,
    ) {
        match event {
            // A drag has begun somewhere; whether it is over him comes next.
            wl_data_device::Event::DataOffer { id } => state.offer = Some((id, false)),
            wl_data_device::Event::Enter { serial, surface, id, .. } => {
                let over = state.surface_of(|w| w.surface.id() == surface.id());
                let files = state.offer.as_ref().is_some_and(|(_, files)| *files);
                state.drop_on = over.filter(|_| files);
                if let Some(offer) = id.as_ref() {
                    match state.drop_on {
                        Some(_) => {
                            offer.accept(serial, Some(URI_LIST.to_string()));
                            offer.set_actions(wl_data_device_manager::DndAction::Copy, wl_data_device_manager::DndAction::Copy);
                        }
                        None => offer.accept(serial, None),
                    }
                }
            }
            wl_data_device::Event::Leave => state.drop_on = None,
            wl_data_device::Event::Drop => {
                let Some(_) = state.drop_on.take() else { return };
                let Some((offer, _)) = state.offer.take() else { return };
                let paths = read_offer(&offer, conn).map(|text| dropped_files(&text)).unwrap_or_default();
                offer.finish();
                offer.destroy();
                trace::record(|| format!("wayland: {} files dropped on him", paths.len()));
                if let Some(first) = paths.first() {
                    state.events.push(Event::Files(first.clone(), paths.len() - 1));
                }
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(State, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for State {
    fn event(state: &mut State, _offer: &wl_data_offer::WlDataOffer, event: wl_data_offer::Event, _: &(), _conn: &Connection, _qh: &QueueHandle<State>) {
        if let wl_data_offer::Event::Offer { mime_type } = event
            && mime_type == URI_LIST
            && let Some((_, files)) = state.offer.as_mut()
        {
            *files = true;
        }
    }
}

delegate_noop!(State: ignore wl_data_device_manager::WlDataDeviceManager);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_list_of_files_reads_as_paths() {
        let list = "#comment\r\nfile:///home/pet/a%20note.txt\r\nfile:///tmp/setup.exe\r\n";
        let paths = dropped_files(list);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].to_string_lossy(), "/home/pet/a note.txt");
        assert_eq!(paths[1].to_string_lossy(), "/tmp/setup.exe");
        assert!(dropped_files("https://example.com/x").is_empty(), "only files, not links");
        assert_eq!(unescape("100%"), "100%", "a stray percent is left alone");
        assert_eq!(unescape("h%C3%A1%C4%8Dek"), "háček");
    }
}

delegate_noop!(State: ignore wp_viewporter::WpViewporter);
delegate_noop!(State: wp_viewport::WpViewport);

// The kernel's code for Escape, which is Escape on every layout.
const KEY_ESCAPE: u32 = 1;

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(state: &mut State, _keyboard: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &(), _conn: &Connection, _qh: &QueueHandle<State>) {
        if let wl_keyboard::Event::Key { key: KEY_ESCAPE, state: WEnum::Value(wl_keyboard::KeyState::Pressed), .. } = event
            && state.menu.is_some()
        {
            state.menu_done = Some(None);
        }
    }
}
