// Microsoft-Windows-Kernel-Network over ETW. Performance Log Users may start a
// session, but only admin may enable this provider (EnableTraceEx2 returns 5
// otherwise), so without elevation `start` fails.
//
// A real-time session outlives the process that started it, so the app stops
// it on the way out and replaces any leftover on the way in.

use std::collections::HashMap;
use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex};
use std::thread;

use super::system::wide;

use windows_sys::Win32::System::Diagnostics::Etw::{
    CONTROLTRACE_HANDLE, ControlTraceW, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_RECORD,
    EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE,
    EnableTraceEx2, OpenTraceW, PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_REAL_TIME,
    PROCESSTRACE_HANDLE, ProcessTrace, StartTraceW, TRACE_LEVEL_INFORMATION, WNODE_FLAG_TRACED_GUID,
};
use windows_sys::core::GUID;

const SESSION: &str = "raccy-kernel-network";
const KERNEL_NETWORK: GUID = GUID::from_u128(0x7dd42a49_5329_4832_8dfd_43d979153a88);
const KEYWORD_IPV4: u64 = 0x10;
const KEYWORD_IPV6: u64 = 0x20;
const ERROR_ALREADY_EXISTS: u32 = 183;
const INVALID_PROCESSTRACE_HANDLE: u64 = u64::MAX;

type Key = (u32, IpAddr, u16, bool);
type Counts = Mutex<HashMap<Key, (u64, u64)>>;

#[derive(Debug)]
pub struct RawFlow {
    pub pid: u32,
    pub remote: IpAddr,
    pub port: u16,
    pub udp: bool,
    pub rx: u64,
    pub tx: u64,
}

pub struct Etw {
    counts: Arc<Counts>,
    // False once the trace stops delivering, say when its session is stopped
    // from outside: byte counts per destination are gone from then on.
    alive: Arc<std::sync::atomic::AtomicBool>,
}

impl Etw {
    pub fn start() -> Result<Etw, String> {
        unsafe {
            let session = start_session().map_err(|rc| format!("StartTraceW: error {rc}"))?;
            let rc = EnableTraceEx2(
                session,
                &KERNEL_NETWORK,
                EVENT_CONTROL_CODE_ENABLE_PROVIDER,
                TRACE_LEVEL_INFORMATION as u8,
                KEYWORD_IPV4 | KEYWORD_IPV6,
                0,
                0,
                std::ptr::null(),
            );
            if rc != 0 {
                stop_session();
                return Err(format!("EnableTraceEx2: error {rc}"));
            }

            let counts: Arc<Counts> = Arc::default();
            // The consumer thread runs for the life of the process, so its
            // reference to the counts is never given back.
            let context = Arc::into_raw(counts.clone()) as *mut c_void;
            let mut name = wide(SESSION);
            let mut logfile: EVENT_TRACE_LOGFILEW = std::mem::zeroed();
            logfile.LoggerName = name.as_mut_ptr();
            logfile.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
            logfile.Anonymous2.EventRecordCallback = Some(on_event);
            logfile.Context = context;
            let trace = OpenTraceW(&mut logfile);
            if trace.Value == INVALID_PROCESSTRACE_HANDLE {
                drop(Arc::from_raw(context as *const Counts));
                stop_session();
                return Err(format!("OpenTraceW: error {}", std::io::Error::last_os_error()));
            }
            let value = trace.Value;
            let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
            let running = alive.clone();
            let started = thread::Builder::new().name("raccy-etw".into()).spawn(move || {
                let handle = PROCESSTRACE_HANDLE { Value: value };
                ProcessTrace(&handle, 1, std::ptr::null(), std::ptr::null());
                running.store(false, std::sync::atomic::Ordering::Relaxed);
            });
            if let Err(e) = started {
                // Nobody is left to read the session, and a real-time one
                // outlives the process that made it.
                drop(Arc::from_raw(context as *const Counts));
                stop_session();
                return Err(format!("spawn: {e}"));
            }
            Ok(Etw { counts, alive })
        }
    }

    pub fn alive(&self) -> bool {
        self.alive.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn drain(&self) -> Vec<RawFlow> {
        let counts = std::mem::take(&mut *self.counts.lock().unwrap_or_else(|e| e.into_inner()));
        counts
            .into_iter()
            .map(|((pid, remote, port, udp), (rx, tx))| RawFlow { pid, remote, port, udp, rx, tx })
            .collect()
    }
}


// EVENT_TRACE_PROPERTIES followed by room for the session name, 8-aligned.
fn properties() -> Vec<u64> {
    let header = size_of::<EVENT_TRACE_PROPERTIES>();
    let bytes = header + 2 * (SESSION.len() + 1) + 16;
    let mut buf = vec![0u64; bytes.div_ceil(8)];
    let p = buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
    unsafe {
        (*p).Wnode.BufferSize = (buf.len() * 8) as u32;
        (*p).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        (*p).Wnode.ClientContext = 1;
        (*p).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        (*p).FlushTimer = 1;
        (*p).LoggerNameOffset = header as u32;
    }
    buf
}

unsafe fn start_session() -> Result<CONTROLTRACE_HANDLE, u32> {
    let name = wide(SESSION);
    let mut rc = 0;
    for _ in 0..2 {
        let mut props = properties();
        let mut handle = CONTROLTRACE_HANDLE { Value: 0 };
        rc = unsafe { StartTraceW(&mut handle, name.as_ptr(), props.as_mut_ptr() as *mut _) };
        match rc {
            0 => return Ok(handle),
            // Left behind by a Raccy that did not get to clean up.
            ERROR_ALREADY_EXISTS => stop_session(),
            _ => return Err(rc),
        }
    }
    Err(rc)
}

pub fn stop_session() {
    let name = wide(SESSION);
    let mut props = properties();
    unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE { Value: 0 },
            name.as_ptr(),
            props.as_mut_ptr() as *mut _,
            EVENT_TRACE_CONTROL_STOP,
        );
    }
}

unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    let record = unsafe { &*record };
    let (v6, send) = match record.EventHeader.EventDescriptor.Id {
        // TCP and UDP, IPv4 and IPv6, send and receive.
        10 | 42 => (false, true),
        11 | 43 => (false, false),
        26 | 58 => (true, true),
        27 | 59 => (true, false),
        _ => return,
    };
    if record.UserData.is_null() || record.UserContext.is_null() {
        return;
    }
    let data = unsafe { std::slice::from_raw_parts(record.UserData as *const u8, record.UserDataLength as usize) };
    let Some(event) = parse(data, v6) else { return };
    let counts = unsafe { &*(record.UserContext as *const Counts) };
    // A panic here would unwind into the kernel's own call and kill the process.
    let mut counts = counts.lock().unwrap_or_else(|e| e.into_inner());
    let udp = matches!(record.EventHeader.EventDescriptor.Id, 42 | 43 | 58 | 59);
    let entry = counts.entry((event.pid, event.remote, event.remote_port, udp)).or_default();
    if send {
        entry.1 += event.size as u64;
    } else {
        entry.0 += event.size as u64;
    }
}

struct Event {
    pid: u32,
    size: u32,
    remote: IpAddr,
    remote_port: u16,
}

// Payload: PID, size, daddr, saddr, dport, sport, then fields Raccy ignores.
// The kernel reports the remote end as daddr/dport in both directions, and
// ports in network byte order.
fn parse(data: &[u8], v6: bool) -> Option<Event> {
    let u32_at = |at: usize| Some(u32::from_ne_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let port_at = |at: usize| Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?));
    let pid = u32_at(0)?;
    let size = u32_at(4)?;
    let (remote, port_offset) = if v6 {
        let bytes: [u8; 16] = data.get(8..24)?.try_into().ok()?;
        let addr = Ipv6Addr::from(bytes);
        (addr.to_ipv4_mapped().map_or(IpAddr::V6(addr), IpAddr::V4), 40)
    } else {
        let bytes: [u8; 4] = data.get(8..12)?.try_into().ok()?;
        (IpAddr::V4(Ipv4Addr::from(bytes)), 16)
    };
    Some(Event { pid, size, remote, remote_port: port_at(port_offset)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // Needs the trace rights.
    #[test]
    #[ignore]
    fn live_download_is_attributed() {
        let etw = Etw::start().unwrap_or_else(|e| panic!("trace session: {e}"));
        thread::sleep(Duration::from_millis(1500));
        etw.drain();
        let status = std::process::Command::new("curl")
            .args(["-s", "-o", "NUL", "--max-time", "8", "https://proof.ovh.net/files/10Mb.dat"])
            .status()
            .expect("curl");
        thread::sleep(Duration::from_millis(2500));
        let mut flows = etw.drain();
        stop_session();
        flows.sort_by_key(|f| std::cmp::Reverse(f.rx + f.tx));
        for f in flows.iter().take(8) {
            println!("{f:?}");
        }
        assert!(status.success());
        let top = &flows[0];
        assert_eq!(top.port, 443, "remote port of the download");
        assert!(top.rx > 5 * 1024 * 1024, "download bytes counted as received");
    }
}
