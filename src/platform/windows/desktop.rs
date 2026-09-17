use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, IsZoomed, WS_CAPTION, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};
use windows_sys::core::BOOL;

use super::super::{Desktop, Monitor, MonitorId, Rect, Window, WindowId};

fn rect(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

pub fn snapshot() -> Desktop {
    let mut cursor = POINT { x: 0, y: 0 };
    let cursor = (unsafe { GetCursorPos(&mut cursor) } != 0).then_some((cursor.x, cursor.y));
    let foreground = unsafe { GetForegroundWindow() };
    Desktop {
        monitors: monitors().into_iter().filter_map(monitor).collect(),
        windows: windows_front_to_back().into_iter().filter_map(window).collect(),
        foreground: (!foreground.is_null()).then_some(WindowId(foreground as isize)),
        cursor,
    }
}

fn monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        unsafe { (*(data as *mut Vec<HMONITOR>)).push(monitor) };
        1
    }
    let mut monitors: Vec<HMONITOR> = Vec::new();
    unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(collect), &mut monitors as *mut _ as LPARAM) };
    monitors
}

fn monitor(handle: HMONITOR) -> Option<Monitor> {
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    (unsafe { GetMonitorInfoW(handle, &mut info) } != 0)
        .then(|| Monitor { id: MonitorId(handle as isize), whole: rect(info.rcMonitor), work: rect(info.rcWork) })
}

fn windows_front_to_back() -> Vec<HWND> {
    unsafe extern "system" fn collect(window: HWND, data: LPARAM) -> BOOL {
        unsafe { (*(data as *mut Vec<HWND>)).push(window) };
        1
    }
    let mut windows: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect), &mut windows as *mut _ as LPARAM) };
    windows
}

fn window(handle: HWND) -> Option<Window> {
    let shown = unsafe { IsWindowVisible(handle) != 0 && IsIconic(handle) == 0 };
    if !shown || cloaked(handle) {
        return None;
    }
    let frame = frame_of(handle)?;
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(handle, &mut pid) };
    let style = unsafe { GetWindowLongPtrW(handle, GWL_STYLE) } as u32;
    let ex = unsafe { GetWindowLongPtrW(handle, GWL_EXSTYLE) } as u32;
    Some(Window {
        id: WindowId(handle as isize),
        pid,
        frame,
        class: class_name(handle),
        // A maximised browser gone fullscreen drops its caption: fullscreen, not maximised.
        maximised: unsafe { IsZoomed(handle) } != 0 && style & WS_CAPTION == WS_CAPTION,
        overlay: ex & (WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT) != 0,
        topmost: ex & WS_EX_TOPMOST != 0,
    })
}

// Hidden by the shell, like windows on another virtual desktop.
fn cloaked(window: HWND) -> bool {
    let mut cloaked = 0u32;
    let hr = unsafe { DwmGetWindowAttribute(window, DWMWA_CLOAKED as u32, &mut cloaked as *mut u32 as *mut c_void, size_of::<u32>() as u32) };
    hr == 0 && cloaked != 0
}

// The visible frame, without the invisible resize borders.
fn frame_of(window: HWND) -> Option<Rect> {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    let hr = unsafe { DwmGetWindowAttribute(window, DWMWA_EXTENDED_FRAME_BOUNDS as u32, &mut r as *mut RECT as *mut c_void, size_of::<RECT>() as u32) };
    if hr == 0 {
        return Some(rect(r));
    }
    (unsafe { GetWindowRect(window, &mut r) } != 0).then(|| rect(r))
}

fn class_name(window: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(window, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn window_now(window: WindowId) -> Option<Rect> {
    let handle = window.0 as HWND;
    let shown = unsafe { IsWindowVisible(handle) != 0 && IsIconic(handle) == 0 };
    (shown && !cloaked(handle)).then(|| frame_of(handle)).flatten()
}

// The taskbars, the desktop itself, and his own windows: nobody works in one
// of these and he never sits on one.
pub const FURNITURE: &[&str] = &["Shell_TrayWnd", "Shell_SecondaryTrayWnd", "Progman", "WorkerW", "RaccyPet", "RaccyPanel"];
