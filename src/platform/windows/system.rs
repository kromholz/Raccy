use windows_sys::Win32::Foundation::{CloseHandle, HWND};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::Registry::{
    HKEY, KEY_READ, REG_DWORD, REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::Shell::{SHQueryUserNotificationState, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IDYES, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW, PostMessageW, SW_SHOWNORMAL, WM_CLOSE,
};

use super::super::WindowId;

const PROCESS_SYNCHRONIZE: u32 = 0x0010_0000;
const HANDOVER_MS: u32 = 60_000;
// Presentation mode is the one state Windows keeps for the whole desktop. A
// game or film fullscreen is looked at monitor by monitor instead
// (`roam::fullscreen_app`): Windows reports one on any monitor, and Raccy
// on another may still talk.
const QUNS_PRESENTATION_MODE: i32 = 4;

pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// A child process with no console window of its own.
pub(super) const CREATE_NO_WINDOW: u32 = 0x0800_0000;
// Where a program that starts with the session without asking for admin puts
// itself. What writes it and what reads it both take it from here.
pub(super) const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

pub(super) struct RegKey(pub HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

impl RegKey {
    pub fn open(root: HKEY, path: &str) -> Option<RegKey> {
        let path = wide(path);
        let mut key: HKEY = std::ptr::null_mut();
        (unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ, &mut key) } == 0).then_some(RegKey(key))
    }

    pub fn subkeys(&self) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0.. {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let rc = unsafe {
                RegEnumKeyExW(self.0, i, name.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
            };
            if rc != 0 {
                break;
            }
            out.push(String::from_utf16_lossy(&name[..len as usize]));
        }
        out
    }

    // The names of its values, not the unnamed default one.
    pub fn values(&self) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0.. {
            let mut name = [0u16; 16384];
            let mut len = name.len() as u32;
            let rc = unsafe {
                RegEnumValueW(self.0, i, name.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
            };
            if rc != 0 {
                break;
            }
            if len > 0 {
                out.push(String::from_utf16_lossy(&name[..len as usize]));
            }
        }
        out
    }

    pub fn dword(&self, name: &str) -> Option<u32> {
        let name = wide(name);
        let (mut value, mut size, mut kind) = (0u32, 4u32, 0u32);
        let rc = unsafe { RegQueryValueExW(self.0, name.as_ptr(), std::ptr::null(), &mut kind, (&mut value as *mut u32).cast(), &mut size) };
        (rc == 0 && kind == REG_DWORD).then_some(value)
    }

    pub fn string(&self, name: &str) -> Option<String> {
        let name = wide(name);
        let mut buf = [0u16; 2048];
        let (mut size, mut kind) = ((buf.len() * 2) as u32, 0u32);
        let rc = unsafe { RegQueryValueExW(self.0, name.as_ptr(), std::ptr::null(), &mut kind, buf.as_mut_ptr().cast(), &mut size) };
        (rc == 0 && (kind == REG_SZ || kind == REG_EXPAND_SZ)).then(|| String::from_utf16_lossy(&buf[..size as usize / 2]).trim_end_matches('\0').to_string())
    }

    pub fn has(&self, name: &str) -> bool {
        let name = wide(name);
        unsafe { RegQueryValueExW(self.0, name.as_ptr(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) == 0 }
    }
}

// An elevated restart is started by the instance it replaces, and has to
// wait for that one to save and exit before taking over.
pub fn wait_for_predecessor() {
    let args: Vec<String> = std::env::args().collect();
    let Some(pid) = args.windows(2).find(|w| w[0] == "--after").and_then(|w| w[1].parse::<u32>().ok()) else {
        return;
    };
    unsafe {
        let old = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !old.is_null() {
            WaitForSingleObject(old, HANDOVER_MS);
            CloseHandle(old);
        }
    }
}

pub fn idle_secs() -> u64 {
    unsafe {
        let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        if GetLastInputInfo(&mut info) == 0 {
            return 0;
        }
        (GetTickCount().wrapping_sub(info.dwTime) / 1000) as u64
    }
}

// Whether there is anything to restart as: on Windows an administrator,
// who may read the sizes of what every process sends.
pub fn may_elevate() -> bool {
    true
}

pub fn hushed() -> bool {
    let mut state = 0;
    let ok = unsafe { SHQueryUserNotificationState(&mut state) } == 0;
    ok && state == QUNS_PRESENTATION_MODE
}

// A Raccy running as admin ignores this; its task is ended separately.
pub fn close_running() {
    let class = wide("RaccyPet");
    for _ in 0..50 {
        let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        if hwnd.is_null() {
            return;
        }
        unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

// False if the UAC prompt was declined.
pub fn restart_elevated(owner: WindowId) -> bool {
    let hwnd = owner.0 as HWND;
    unsafe {
        let mut buf = [0u16; 32768];
        let len = GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) as usize;
        let exe: Vec<u16> = buf[..len].iter().copied().chain(Some(0)).collect();
        let verb = wide("runas");
        let args = wide(&format!("--after {}", std::process::id()));
        let rc = ShellExecuteW(hwnd, verb.as_ptr(), exe.as_ptr(), args.as_ptr(), std::ptr::null(), SW_SHOWNORMAL);
        rc as isize > 32
    }
}

// A yes or no from whoever is at the machine, for the one thing that is
// asked outside his own window: whether his memory goes with him when the
// package takes him away.
pub fn ask(question: &str) -> bool {
    let (text, title) = (wide(question), wide("Raccy"));
    let answer = unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND) };
    answer == IDYES
}
