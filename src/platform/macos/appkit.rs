// His window on macOS: a borderless panel that neither takes the keyboard nor
// brings the app forward when it is clicked, with his canvas on its layer.
//
// A Mac insists its windows are made and touched on the main thread; asked
// from anywhere else, everything here does nothing and says so by its answer.

use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDidChangeScreenParametersNotification, NSBackingStoreType, NSColor,
    NSDraggingInfo, NSEvent, NSEventMask, NSMenu, NSMenuItem, NSPanel, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypeString,
    NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFData, CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo};
use objc2_foundation::{NSArray, NSDate, NSDefaultRunLoopMode, NSNotification, NSNotificationCenter, NSObject, NSString, NSURL};

use super::super::Event;
use super::desktop;
use crate::app::dispatch;
use crate::render::{Canvas, MenuItem};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Pet,
    Panel,
}

// Above every ordinary window and above a full-screen one, and still under
// the menus a program puts up. `NSStatusWindowLevel`.
const OVER_EVERYTHING: isize = 25;

// What a press turned into: where the pointer and the window were when it
// started, and whether it has become a drag.
struct Drag {
    mouse: CGPoint,
    origin: CGPoint,
    moved: bool,
}

static DRAG: Mutex<Option<Drag>> = Mutex::new(None);
// The item the menu was left on, put here by the menu's own target.
static CHOSEN: AtomicIsize = AtomicIsize::new(-1);

define_class!(
    // A view that answers the mouse itself rather than leaving it to the
    // window: every pixel of him is a handle, and the right button is the
    // menu wherever it lands.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RaccyView"]
    #[ivars = Cell<bool>]
    struct RaccyView;

    impl RaccyView {
        // A click reaches him even while another program is in front, which
        // is the whole point of a pet that sits on top of the work.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            if self.is_panel() {
                return;
            }
            let origin = self.window().map(|w| w.frame().origin).unwrap_or(CGPoint::new(0.0, 0.0));
            *drag() = Some(Drag { mouse: NSEvent::mouseLocation(), origin, moved: false });
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, _event: &NSEvent) {
            let now = NSEvent::mouseLocation();
            let mut held = drag();
            let Some(held_drag) = held.as_mut() else { return };
            let to = CGPoint::new(held_drag.origin.x + now.x - held_drag.mouse.x, held_drag.origin.y + now.y - held_drag.mouse.y);
            // Nothing has moved until the pointer leaves where it went down:
            // a click that shakes by a pixel is still a click.
            if !held_drag.moved && (now.x - held_drag.mouse.x).abs() < 3.0 && (now.y - held_drag.mouse.y).abs() < 3.0 {
                return;
            }
            let first = !held_drag.moved;
            held_drag.moved = true;
            drop(held);
            if first {
                dispatch(Event::Grabbed);
            }
            if let Some(window) = self.window() {
                window.setFrameOrigin(to);
            }
            dispatch(Event::Moved);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            if self.is_panel() {
                dispatch(Event::PanelClicked);
                return;
            }
            let moved = drag().take().is_some_and(|d| d.moved);
            dispatch(if moved { Event::Dropped } else { Event::Clicked });
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, _event: &NSEvent) {
            dispatch(if self.is_panel() { Event::PanelCopy } else { Event::Menu });
        }

        // A Mac counts a scroll in lines, or in points when it comes off a
        // trackpad; the panel counts it in the hundred and twenty a wheel
        // gives for one notch, which is what the other two send. Ten points
        // under a finger is worth that one notch.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            if !self.is_panel() {
                return;
            }
            let by = event.scrollingDeltaY();
            let step = if event.hasPreciseScrollingDeltas() { 12.0 } else { 120.0 };
            let notches = (by * step) as i32;
            if notches != 0 {
                dispatch(Event::PanelWheel(notches));
            }
        }

        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> usize {
            // NSDragOperationCopy.
            1
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let board = sender.draggingPasteboard();
            dropped_files(&board)
        }
    }
);

impl RaccyView {
    fn new(mtm: MainThreadMarker, panel: bool) -> Retained<RaccyView> {
        let this = RaccyView::alloc(mtm).set_ivars(Cell::new(panel));
        let this: Retained<RaccyView> = unsafe { msg_send![super(this), init] };
        this.setWantsLayer(true);
        this
    }

    fn is_panel(&self) -> bool {
        self.ivars().get()
    }
}

fn drag() -> std::sync::MutexGuard<'static, Option<Drag>> {
    DRAG.lock().unwrap_or_else(|e| e.into_inner())
}

// The names of what was let go of over him. Only the first is opened; the
// rest are counted, the same as on the other two.
fn dropped_files(board: &NSPasteboard) -> bool {
    let Some(items) = board.pasteboardItems() else { return false };
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for item in items.iter() {
        let Some(text) = (unsafe { item.stringForType(NSPasteboardTypeFileURL) }) else { continue };
        let url = NSURL::URLWithString(&text);
        if let Some(path) = url.and_then(|url| url.path()) {
            paths.push(std::path::PathBuf::from(path.to_string()));
        }
    }
    let Some(first) = paths.first() else { return false };
    dispatch(Event::Files(first.clone(), paths.len() - 1));
    true
}

define_class!(
    // The one object the system talks to about what is nobody's window in
    // particular: the item a menu was left on, and the screens having been
    // rearranged.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RaccyAgent"]
    struct Agent;

    impl Agent {
        #[unsafe(method(chosen:))]
        fn chosen(&self, item: &NSMenuItem) {
            CHOSEN.store(item.tag(), Ordering::SeqCst);
        }

        #[unsafe(method(screensChanged:))]
        fn screens_changed(&self, _note: &NSNotification) {
            dispatch(Event::DisplayChanged);
        }
    }
);

struct Pane {
    window: Retained<NSPanel>,
    view: Retained<RaccyView>,
    // What he was last drawn at, in his own pixels.
    size: (usize, usize),
}

thread_local! {
    static PANES: std::cell::RefCell<Vec<(u8, Pane)>> = const { std::cell::RefCell::new(Vec::new()) };
    static AGENT: std::cell::RefCell<Option<Retained<Agent>>> = const { std::cell::RefCell::new(None) };
}

impl Which {
    fn name(self) -> &'static str {
        match self {
            Which::Pet => "pet",
            Which::Panel => "panel",
        }
    }
}

fn slot(which: Which) -> u8 {
    match which {
        Which::Pet => 0,
        Which::Panel => 1,
    }
}

// Whatever is to be done to a window, done on the thread a Mac allows it on.
fn with<R>(which: Which, off_main: R, f: impl FnOnce(&mut Pane, MainThreadMarker) -> R) -> R {
    let Some(mtm) = MainThreadMarker::new() else { return off_main };
    PANES.with_borrow_mut(|panes| match panes.iter_mut().find(|(at, _)| *at == slot(which)) {
        Some((_, pane)) => f(pane, mtm),
        None => off_main,
    })
}

// A window the system puts up on a program's behalf goes to whoever is in
// front, and an accessory is in front of nobody: it would be raised for
// somebody else or not at all.
pub fn step_forward(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}

pub fn stand_aside(mtm: MainThreadMarker) {
    NSApplication::sharedApplication(mtm).setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}

pub fn start() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    // An accessory has no icon in the Dock and no menu bar of its own: he is
    // a thing on the desktop, not a program somebody switches to.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();
    AGENT.with_borrow_mut(|held| {
        if held.is_some() {
            return;
        }
        let agent: Retained<Agent> = unsafe { msg_send![Agent::alloc(mtm), init] };
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &agent,
                sel!(screensChanged:),
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
            );
        }
        *held = Some(agent);
    });
}

pub fn open(which: Which, at: (i32, i32), size: (i32, i32)) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let scale = desktop::scale();
    let points = CGSize::new(size.0 as f64 / scale, size.1 as f64 / scale);
    let frame = CGRect::new(desktop::appkit_origin(at, size.1 as f64), points);
    // A non-activating panel takes the mouse without putting the program in
    // front of whatever the person was working in.
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    let window = NSPanel::initWithContentRect_styleMask_backing_defer(NSPanel::alloc(mtm), frame, style, NSBackingStoreType::Buffered, false);
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setHasShadow(false);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    window.setFloatingPanel(true);
    window.setBecomesKeyOnlyIfNeeded(true);
    // After the panel is told it floats, which sets a level of its own.
    window.setLevel(OVER_EVERYTHING);
    // He is moved by the code that watches where he is, never by the system
    // taking the window for a ride of its own.
    window.setMovableByWindowBackground(false);
    window.setMovable(false);
    let view = RaccyView::new(mtm, which == Which::Panel);
    if which == Which::Pet {
        unsafe { view.registerForDraggedTypes(&NSArray::from_slice(&[NSPasteboardTypeFileURL])) };
    }
    window.setContentView(Some(&view));
    window.orderFrontRegardless();
    // What the window server made of it, rather than what it was asked for.
    crate::trace::record(|| {
        let frame = window.frame();
        let (o, s) = (frame.origin, frame.size);
        format!(
            "window {} at {},{} {}x{} points of {}x{} pixels at {}x, level {}, visible {}",
            which.name(),
            o.x,
            o.y,
            s.width,
            s.height,
            size.0,
            size.1,
            scale,
            window.level(),
            window.isVisible()
        )
    });
    PANES.with_borrow_mut(|panes| {
        panes.retain(|(at, _)| *at != slot(which));
        panes.push((slot(which), Pane { window, view, size: (size.0.max(0) as usize, size.1.max(0) as usize) }));
    });
}

pub fn close(which: Which) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    PANES.with_borrow_mut(|panes| {
        if let Some(at) = panes.iter().position(|(at, _)| *at == slot(which)) {
            panes.remove(at).1.window.close();
        }
    });
}

// The name the window server knows it by, which is the same name the window
// list gives, so that he can tell himself from everybody else in it.
pub fn number(which: Which) -> Option<isize> {
    with(which, None, |pane, _| Some(pane.window.windowNumber()))
}

pub fn at(which: Which) -> Option<(i32, i32)> {
    with(which, None, |pane, _| Some(desktop::raccy_pos(pane.window.frame())))
}

pub fn place(which: Which, at: (i32, i32)) {
    with(which, (), |pane, _| {
        let height = pane.window.frame().size.height * desktop::scale();
        pane.window.setFrameOrigin(desktop::appkit_origin(at, height));
    });
}

pub fn show(which: Which, shown: bool) {
    with(which, (), |pane, _| match shown {
        true => pane.window.orderFrontRegardless(),
        false => pane.window.orderOut(None),
    });
}

pub fn raise(which: Which) {
    with(which, (), |pane, _| pane.window.orderFrontRegardless());
}

pub fn present(which: Which, canvas: &Canvas) {
    let Some(image) = image_of(canvas) else { return };
    let scale = desktop::scale();
    with(which, (), |pane, _| {
        if pane.size != (canvas.width, canvas.height) {
            // The window grows from its top left, which is the corner the
            // rest of the program counts from; a Mac counts from the other.
            let at = desktop::raccy_pos(pane.window.frame());
            let (w, h) = (canvas.width as f64, canvas.height as f64);
            let frame = CGRect::new(desktop::appkit_origin(at, h), CGSize::new(w / scale, h / scale));
            pane.window.setFrame_display(frame, false);
            pane.size = (canvas.width, canvas.height);
        }
        let Some(layer) = pane.view.layer() else { return };
        // The layer holds his pixels one for one: the scale says how many of
        // them go into a point of the screen.
        let fresh = unsafe { layer.contents() }.is_none();
        layer.setContentsScale(scale);
        unsafe { layer.setContents(Some(&*(CFRetained::as_ptr(&image).as_ptr() as *const AnyObject))) };
        if fresh {
            crate::trace::record(|| format!("window {} has his picture: {}", which.name(), unsafe { layer.contents() }.is_some()));
        }
    });
}

// The canvas as a picture the window server can hold: the pixels are already
// premultiplied and in the order a Mac reads a little-endian word as ARGB.
fn image_of(canvas: &Canvas) -> Option<CFRetained<CGImage>> {
    let bytes = unsafe { std::slice::from_raw_parts(canvas.pixels.as_ptr().cast::<u8>(), canvas.pixels.len() * 4) };
    // The picture takes its own copy of what it is given.
    let data = unsafe { CFData::new(None, bytes.as_ptr(), bytes.len() as isize) }?;
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    let space = CGColorSpace::new_device_rgb()?;
    let order = CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::PremultipliedFirst.0;
    unsafe {
        CGImage::new(
            canvas.width,
            canvas.height,
            8,
            32,
            canvas.width * 4,
            Some(&space),
            CGBitmapInfo(order),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}

// Everything the system has to say, until the time is up. The first wait is
// the only one that sleeps: after it, whatever else is already queued is
// taken without waiting again.
pub fn pump(wait: Duration) {
    let Some(mtm) = MainThreadMarker::new() else {
        std::thread::sleep(wait);
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let mut until = NSDate::dateWithTimeIntervalSinceNow(wait.as_secs_f64());
    loop {
        let event = unsafe { app.nextEventMatchingMask_untilDate_inMode_dequeue(NSEventMask::Any, Some(&until), NSDefaultRunLoopMode, true) };
        let Some(event) = event else { return };
        app.sendEvent(&event);
        until = NSDate::distantPast();
    }
}

// The system's own menu, which is what a Mac user expects to see, put up
// where the pointer is. It does not come back until the menu is gone.
pub fn menu(items: &[MenuItem]) -> Option<usize> {
    let mtm = MainThreadMarker::new()?;
    let agent = AGENT.with_borrow(|held| held.clone())?;
    CHOSEN.store(-1, Ordering::SeqCst);
    let menu = build(items, &agent, mtm);
    let at = NSEvent::mouseLocation();
    menu.popUpMenuPositioningItem_atLocation_inView(None, at, None);
    match CHOSEN.swap(-1, Ordering::SeqCst) {
        id if id > 0 => Some(id as usize),
        _ => None,
    }
}

fn build(items: &[MenuItem], agent: &Agent, mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    for item in items {
        match item {
            MenuItem::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
            MenuItem::Item { id, label, checked, grayed } => {
                let entry = entry(label, Some(sel!(chosen:)), mtm);
                unsafe {
                    entry.setTag(*id as isize);
                    entry.setTarget(Some(agent));
                    // NSControlStateValueOn, NSControlStateValueOff.
                    entry.setState(if *checked { 1 } else { 0 });
                }
                entry.setEnabled(!*grayed);
                menu.addItem(&entry);
            }
            MenuItem::Submenu { label, items } => {
                let entry = entry(label, None, mtm);
                menu.addItem(&entry);
                menu.setSubmenu_forItem(Some(&build(items, agent, mtm)), &entry);
            }
        }
    }
    menu
}

fn entry(label: &str, action: Option<Sel>, mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(label),
            action,
            &NSString::from_str(""),
        )
    }
}

// A password is put on the board under the type the password keepers watch
// for, so that what reads the clipboard for its history leaves it alone.
pub fn copy(text: &str, private: bool) -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }
    let board = NSPasteboard::generalPasteboard();
    let concealed = NSString::from_str("org.nspasteboard.ConcealedType");
    let text = NSString::from_str(text);
    unsafe {
        let types: Vec<&NSString> = match private {
            true => vec![NSPasteboardTypeString, &concealed],
            false => vec![NSPasteboardTypeString],
        };
        board.declareTypes_owner(&NSArray::from_slice(&types), None);
        let put = board.setString_forType(&text, NSPasteboardTypeString);
        if private {
            board.setString_forType(&text, &concealed);
        }
        put
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_graphics::{CGBitmapContextCreate, CGContext};

    use super::*;

    // What the window server is handed has to be the canvas and not a
    // rearrangement of it: the same colours, in the same corners, with the
    // channels in the order a Mac reads them.
    #[test]
    fn the_canvas_becomes_a_picture_the_right_way_round() {
        // Opaque red, green, blue, white, as 0xAARRGGBB reading downwards.
        let canvas = Canvas { width: 2, height: 2, pixels: vec![0xFFFF_0000, 0xFF00_FF00, 0xFF00_00FF, 0xFFFF_FFFF] };
        let image = image_of(&canvas).expect("a picture");
        assert_eq!((CGImage::width(Some(&image)), CGImage::height(Some(&image))), (2, 2));

        // Drawn into a sheet whose bytes are plainly alpha, red, green, blue
        // in that order, so that what comes out says which channel went where.
        let mut out = [0u8; 16];
        let space = CGColorSpace::new_device_rgb().expect("colours");
        // kCGImageAlphaPremultipliedFirst with the big-endian order, which is
        // the order the bytes are written in.
        let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Big.0;
        let sheet = unsafe { CGBitmapContextCreate(out.as_mut_ptr().cast(), 2, 2, 8, 8, Some(&space), info) }.expect("a sheet");
        CGContext::draw_image(Some(&sheet), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(2.0, 2.0)), Some(&image));
        drop(sheet);

        let pixel = |at: usize| (out[at * 4], out[at * 4 + 1], out[at * 4 + 2], out[at * 4 + 3]);
        // Row for row and pixel for pixel: the first row of the canvas is the
        // first row of the picture, which is what a layer puts at its top.
        assert_eq!(pixel(0), (0xFF, 0xFF, 0x00, 0x00), "red is the top left");
        assert_eq!(pixel(1), (0xFF, 0x00, 0xFF, 0x00), "green the top right");
        assert_eq!(pixel(2), (0xFF, 0x00, 0x00, 0xFF), "blue the bottom left");
        assert_eq!(pixel(3), (0xFF, 0xFF, 0xFF, 0xFF), "white the bottom right");
    }
}
