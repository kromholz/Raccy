use std::os::windows::ffi::OsStringExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, GlobalFree, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows_sys::Win32::Graphics::Dwm::DwmFlush;
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::super::{Event, Rect, WindowId};
use super::system::wide;
use crate::app::dispatch;
use crate::render::{Canvas, MenuItem};

const WM_GLIDE: u32 = WM_APP + 1;
#[cfg(feature = "trace")]
const WM_TEST_COMMAND: u32 = WM_APP + 2;
// Bit 0 on, bit 1 done.
const WM_AUTOSTART_DONE: u32 = WM_APP + 3;
// Carries the dropped file list between processes; windows-sys has no name for it.
const WM_COPYGLOBALDATA: u32 = 0x0049;

const PET_CLASS: &str = "RaccyPet";
const PANEL_CLASS: &str = "RaccyPanel";

// Per-monitor DPI, before any window is made.
pub fn prepare_process() {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

pub fn system_dpi() -> u32 {
    unsafe { GetDpiForSystem() }
}

// With `handover`, an elevated restart may start while the old Raccy still
// holds the mutex, so it asks again for a while.
pub fn claim_instance(handover: bool) -> bool {
    let name = wide("Local\\raccy-desktop-pet");
    for attempt in 0.. {
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        // An elevated Raccy's mutex cannot even be opened from here: taken all the same.
        let error = unsafe { GetLastError() };
        if !(error == ERROR_ALREADY_EXISTS || (mutex.is_null() && error == ERROR_ACCESS_DENIED)) {
            return true;
        }
        if !mutex.is_null() {
            unsafe { CloseHandle(mutex) };
        }
        if !handover || attempt >= 50 {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shell {
    hwnd: isize,
}

impl Shell {
    fn hwnd(&self) -> HWND {
        self.hwnd as HWND
    }

    pub fn create(x: i32, y: i32, w: i32, h: i32) -> Option<Shell> {
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = wide(PET_CLASS);
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                // Resource 1, embedded by build.rs.
                hIcon: LoadIconW(instance, std::ptr::without_provenance(1)),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);
            let title = wide("Raccy");
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_ACCEPTFILES,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                x,
                y,
                w,
                h,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return None;
            }
            // Raccy often runs elevated: let files dropped from Explorer through,
            // and the installer ask him to close.
            for message in [WM_DROPFILES, WM_COPYDATA, WM_COPYGLOBALDATA, WM_CLOSE] {
                ChangeWindowMessageFilterEx(hwnd, message, MSGFLT_ALLOW, std::ptr::null_mut());
            }
            Some(Shell { hwnd: hwnd as isize })
        }
    }

    pub fn id(&self) -> WindowId {
        WindowId(self.hwnd)
    }

    pub fn pos(&self) -> (i32, i32) {
        let mut r: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(self.hwnd(), &mut r) };
        (r.left, r.top)
    }

    pub fn place(&self, x: i32, y: i32) {
        unsafe { SetWindowPos(self.hwnd(), std::ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    pub fn resize_to(&self, x: i32, y: i32, w: i32, h: i32) {
        unsafe { SetWindowPos(self.hwnd(), std::ptr::null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    pub fn show(&self, shown: bool) {
        unsafe { ShowWindow(self.hwnd(), if shown { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
    }

    // Another window of the topmost band, a player's overlay or a pinned tool,
    // climbs over him when it is clicked and stays there; this puts him back
    // over it.
    pub fn raise(&self) {
        unsafe { SetWindowPos(self.hwnd(), HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    }

    pub fn present(&self, canvas: &Canvas) {
        present(self.hwnd(), canvas);
    }

    pub fn dpi(&self) -> u32 {
        unsafe { GetDpiForWindow(self.hwnd()) }
    }

    // Runs its own message loop: never call it with the app borrowed.
    pub fn menu(&self, items: &[MenuItem], _scale: usize) -> Option<usize> {
        unsafe {
            let menu = build_menu(items);
            let mut pt: POINT = std::mem::zeroed();
            GetCursorPos(&mut pt);
            SetForegroundWindow(self.hwnd());
            let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, self.hwnd(), std::ptr::null());
            DestroyMenu(menu);
            (chosen > 0).then_some(chosen as usize)
        }
    }

    // Private text, like a Wi-Fi password, is kept out of clipboard history and
    // cloud sync.
    pub fn copy(&self, text: &str, private: bool) -> bool {
        // Windows want their line endings on the clipboard; everything Raccy
        // writes is in plain newlines.
        let units: Vec<u16> = text.replace('\n', "\r\n").encode_utf16().chain(Some(0)).collect();
        unsafe {
            if OpenClipboard(self.hwnd()) == 0 {
                return false;
            }
            EmptyClipboard();
            let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
            let mut done = false;
            if !memory.is_null() {
                let target = GlobalLock(memory) as *mut u16;
                if !target.is_null() {
                    std::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
                    GlobalUnlock(memory);
                    // CF_UNICODETEXT. Once the clipboard takes the memory, it owns it.
                    done = !SetClipboardData(13, memory).is_null();
                }
                if !done {
                    GlobalFree(memory);
                }
            }
            if done && private {
                let name = wide("ExcludeClipboardContentFromMonitorProcessing");
                let format = RegisterClipboardFormatW(name.as_ptr());
                let flag = GlobalAlloc(GMEM_MOVEABLE, 4);
                if !flag.is_null() && (format == 0 || SetClipboardData(format, flag).is_null()) {
                    GlobalFree(flag);
                }
            }
            CloseClipboard();
            done
        }
    }

    pub fn watch(&self, window: Option<WindowId>) {
        let mut hook = SEAT_HOOK.lock().unwrap_or_else(|e| e.into_inner());
        if hook.map(|(seat, _)| seat) == window.map(|w| w.0) {
            return;
        }
        if let Some((_, handle)) = hook.take() {
            unsafe { UnhookWinEvent(handle as HWINEVENTHOOK) };
        }
        let Some(window) = window else { return };
        let mut pid = 0;
        let thread = unsafe { GetWindowThreadProcessId(window.0 as HWND, &mut pid) };
        let handle = unsafe {
            SetWinEventHook(EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE, std::ptr::null_mut(), Some(seat_moved), pid, thread, WINEVENT_OUTOFCONTEXT)
        };
        if !handle.is_null() {
            *hook = Some((window.0, handle as isize));
        }
    }

    pub fn post_autostart_done(&self, on: bool, done: bool) {
        unsafe { PostMessageW(self.hwnd(), WM_AUTOSTART_DONE, on as usize | (done as usize) << 1, 0) };
    }

    #[cfg(feature = "trace")]
    pub fn post_test_command(&self, id: usize) {
        unsafe { PostMessageW(self.hwnd(), WM_TEST_COMMAND, id, 0) };
    }

    #[cfg(feature = "trace")]
    pub fn post_menu(&self) {
        unsafe { PostMessageW(self.hwnd(), WM_NCRBUTTONUP, 0, 0) };
    }

    pub fn run(&self, frame_ms: u32) {
        unsafe {
            SetTimer(self.hwnd(), 1, frame_ms, None);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    pub fn quit(&self) {
        unsafe { DestroyWindow(self.hwnd()) };
    }
}

static SEAT_HOOK: Mutex<Option<(isize, isize)>> = Mutex::new(None);

unsafe extern "system" fn seat_moved(_hook: HWINEVENTHOOK, _event: u32, window: HWND, object: i32, child: i32, _thread: u32, _time: u32) {
    if object == OBJID_WINDOW && child == 0 {
        let watched = SEAT_HOOK.lock().map(|h| h.map(|(seat, _)| seat)).unwrap_or(None);
        if watched == Some(window as isize) {
            dispatch(Event::SeatMoved(WindowId(window as isize)));
        }
    }
}

unsafe fn build_menu(items: &[MenuItem]) -> HMENU {
    unsafe {
        let menu = CreatePopupMenu();
        for item in items {
            match item {
                MenuItem::Item { id, label, checked, grayed } => {
                    let flags = MF_STRING | if *checked { MF_CHECKED } else { 0 } | if *grayed { MF_GRAYED } else { 0 };
                    AppendMenuW(menu, flags, *id, wide(label).as_ptr());
                }
                MenuItem::Submenu { label, items } => {
                    let sub = build_menu(items);
                    AppendMenuW(menu, MF_POPUP, sub as usize, wide(label).as_ptr());
                }
                MenuItem::Separator => {
                    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
                }
            }
        }
        menu
    }
}

fn window_pos(hwnd: HWND) -> (i32, i32) {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd, &mut r) };
    (r.left, r.top)
}

// Puts a premultiplied canvas on a layered window.
fn present(hwnd: HWND, canvas: &Canvas) {
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: canvas.width as i32,
            biHeight: -(canvas.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if !bmp.is_null() && !bits.is_null() {
            std::ptr::copy_nonoverlapping(canvas.pixels.as_ptr(), bits as *mut u32, canvas.pixels.len());
            let old = SelectObject(mem, bmp);
            let size = SIZE { cx: canvas.width as i32, cy: canvas.height as i32 };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            UpdateLayeredWindow(hwnd, screen, std::ptr::null(), &size, mem, &src, 0, &blend, ULW_ALPHA);
            SelectObject(mem, old);
            DeleteObject(bmp);
        }
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            dispatch(Event::Frame);
            0
        }
        WM_GLIDE => {
            GLIDE_QUEUED.store(false, Ordering::Relaxed);
            dispatch(Event::Glide);
            0
        }
        // Every visible pixel is a handle: drag Raccy anywhere.
        WM_NCHITTEST => HTCAPTION as LRESULT,
        // Windows returns from a press on Raccy only after the release, drag
        // or not; if the window stayed put, it was a click.
        WM_NCLBUTTONDOWN => {
            dispatch(Event::Grabbed);
            let before = window_pos(hwnd);
            let result = unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            dispatch(if window_pos(hwnd) != before { Event::Dropped } else { Event::Clicked });
            result
        }
        WM_WINDOWPOSCHANGED => {
            dispatch(Event::Moved);
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            dispatch(Event::DisplayChanged);
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_DROPFILES => {
            let drop = wparam as HDROP;
            let count = unsafe { DragQueryFileW(drop, u32::MAX, std::ptr::null_mut(), 0) };
            let len = unsafe { DragQueryFileW(drop, 0, std::ptr::null_mut(), 0) } as usize;
            let mut name = vec![0u16; len + 1];
            if count > 0 {
                unsafe { DragQueryFileW(drop, 0, name.as_mut_ptr(), name.len() as u32) };
            }
            unsafe { DragFinish(drop) };
            if count > 0 {
                let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&name[..len]));
                dispatch(Event::Files(path, count as usize - 1));
            }
            0
        }
        #[cfg(feature = "trace")]
        WM_TEST_COMMAND => {
            dispatch(Event::TestCommand(wparam));
            0
        }
        WM_NCRBUTTONUP => {
            dispatch(Event::Menu);
            0
        }
        WM_DPICHANGED => {
            let dpi = (wparam & 0xFFFF) as u32;
            let r = unsafe { *(lparam as *const RECT) };
            dispatch(Event::Scaled { dpi, suggested: Some(Rect::new(r.left, r.top, r.right, r.bottom)) });
            0
        }
        WM_NCLBUTTONDBLCLK => {
            dispatch(Event::Clicked);
            0
        }
        WM_AUTOSTART_DONE => {
            dispatch(Event::AutostartDone { on: wparam & 1 != 0, done: wparam & 2 != 0 });
            0
        }
        // Shutting down or logging off: the message loop never ends normally.
        WM_ENDSESSION if wparam != 0 => {
            dispatch(Event::EndSession);
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub struct PanelWindow {
    hwnd: HWND,
}

impl PanelWindow {
    pub fn create(width: i32, height: i32) -> Option<PanelWindow> {
        unsafe {
            static REGISTERED: std::sync::Once = std::sync::Once::new();
            let class = wide(PANEL_CLASS);
            let instance = GetModuleHandleW(std::ptr::null());
            REGISTERED.call_once(|| {
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(panel_proc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: instance,
                    hIcon: std::ptr::null_mut(),
                    hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                    hbrBackground: std::ptr::null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class.as_ptr(),
                };
                RegisterClassW(&wc);
            });
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return None;
            }
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Some(PanelWindow { hwnd })
        }
    }

    pub fn place(&self, x: i32, y: i32) {
        unsafe { SetWindowPos(self.hwnd, std::ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    pub fn present(&self, canvas: &Canvas) {
        present(self.hwnd, canvas);
    }

    pub fn raise(&self) {
        unsafe { SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    }
}

impl Drop for PanelWindow {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.hwnd) };
    }
}

unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_MOUSEWHEEL => {
            dispatch(Event::PanelWheel(((wparam >> 16) & 0xFFFF) as u16 as i16 as i32));
            0
        }
        WM_LBUTTONUP => {
            dispatch(Event::PanelClicked);
            0
        }
        WM_RBUTTONUP => {
            dispatch(Event::PanelCopy);
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

static GLIDE_QUEUED: AtomicBool = AtomicBool::new(false);

pub struct Glider {
    moving: AtomicBool,
}

impl Glider {
    pub fn moving(&self, on: bool) {
        self.moving.store(on, Ordering::Relaxed);
    }

    pub fn start(shell: Shell) -> Arc<Glider> {
        let glider = Arc::new(Glider { moving: AtomicBool::new(false) });
        let shared = Arc::clone(&glider);
        std::thread::spawn(move || {
            loop {
                if !shared.moving.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                // Blocks until the next composed frame.
                if unsafe { DwmFlush() } != 0 {
                    std::thread::sleep(Duration::from_millis(7));
                }
                if !GLIDE_QUEUED.swap(true, Ordering::Relaxed) {
                    unsafe { PostMessageW(shell.hwnd(), WM_GLIDE, 0, 0) };
                }
            }
        });
        glider
    }
}
