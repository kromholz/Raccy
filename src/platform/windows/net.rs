use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::mpsc::Sender;
use std::sync::Once;
use std::thread;
use std::time::Instant;

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::NetworkManagement::Dns::{DNS_TYPE_A, DNS_TYPE_AAAA, DNS_TYPE_CNAME, DnsFree, DnsFreeFlat};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetExtendedTcpTable, GetIfTable2, MIB_IF_TABLE2, TCP_TABLE_OWNER_PID_ALL,
};
use windows_sys::Win32::Networking::WinSock::{GetNameInfoW, SOCKADDR, WSADATA, WSAStartup};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};


use super::etw::Etw;
use crate::net::{Conn, Flow, Kind, LISTENERS_EVERY, LOOKS_PER_TICK, Listener, News, Place, TICK, Tick, place_of};

const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;
const MIB_TCP_STATE_LISTEN: u32 = 2;
const MIB_TCP_STATE_SYN_SENT: u32 = 3;
const MIB_TCP_STATE_ESTAB: u32 = 5;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_OPER_STATUS_UP: i32 = 1;
const NI_NAMEREQD: i32 = 4;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
// The kernel's flows in the first ticks are what was already running.
const FLOW_WARMUP: u64 = 3;

pub fn sample_loop(out: Sender<Tick>) {
    let etw = match Etw::start() {
        Ok(etw) => Some(etw),
        Err(e) => {
            crate::trace::record(|| format!("kernel trace not started, totals only: {e}"));
            None
        }
    };
    let mut procs = ProcessNames::default();
    let mut last_bytes = adapter_bytes();
    let mut news = News::default();
    loop {
        let (mut v4, mut v6) = (Vec::new(), Vec::new());
        let mut active = Vec::new();
        let mut opened: Vec<Conn> = Vec::new();
        let mut attempts: Vec<Conn> = Vec::new();
        for _ in 0..LOOKS_PER_TICK {
            thread::sleep(TICK / LOOKS_PER_TICK);
            (v4, v6) = (tcp_table(AF_INET), tcp_table(AF_INET6));
            active = connections(&v4, &v6, MIB_TCP_STATE_ESTAB, &mut procs);
            for c in connections(&v4, &v6, MIB_TCP_STATE_SYN_SENT, &mut procs) {
                if !attempts.contains(&c) {
                    attempts.push(c);
                }
            }
            opened.extend(news.fresh(&active));
        }
        if !opened.is_empty() {
            crate::trace::log(|| {
                let seen: Vec<_> = opened.iter().map(|c| (c.process.as_str(), c.remote, c.port, c.from_temp)).collect();
                format!("opened {seen:?}")
            });
        }
        let bytes = adapter_bytes();
        // Adapter by adapter, and only for adapters in both looks: one that
        // comes back up would otherwise add its whole counter at once.
        let (mut rx, mut tx) = (0u64, 0u64);
        for (index, (now_rx, now_tx)) in &bytes {
            if let Some((was_rx, was_tx)) = last_bytes.get(index) {
                rx += now_rx.saturating_sub(*was_rx);
                tx += now_tx.saturating_sub(*was_tx);
            }
        }
        last_bytes = bytes;
        let count = news.second();
        let listeners = (count % LISTENERS_EVERY == 1).then(|| listening(&v4, &v6, &mut procs));

        let drain_started = Instant::now();
        let raw = etw.as_ref().map(Etw::drain).unwrap_or_default();
        crate::trace::log(|| format!("drain {} flows in {:?}", raw.len(), drain_started.elapsed()));
        // UDP answers from the LAN, like SSDP and mDNS, are nobody opening anything.
        let udp: std::collections::HashSet<(u32, IpAddr, u16)> = raw.iter().filter(|f| f.udp).map(|f| (f.pid, f.remote, f.port)).collect();
        let flows: Vec<Flow> = raw
            .into_iter()
            // His own talking is not news, nor is what he asked a helper to fetch.
            .filter(|f| f.pid != std::process::id() && !super::is_helper(f.pid))
            .filter_map(|f| {
                let place = place_of(f.remote)?;
                let (process, from_temp) = procs.name(f.pid);
                let kind = Kind::from_port(f.port);
                let conn = Conn { pid: f.pid, process, from_temp, local_port: 0, remote: f.remote, port: f.port, kind, place, incoming: false };
                Some(Flow { conn, rx: f.rx, tx: f.tx })
            })
            .collect();
        opened.retain(|c| {
            let first = !news.told(c);
            news.tell(c);
            first
        });
        // As admin the kernel reports every connection, even one that was
        // over before any look at the table: a destination not open and not
        // told of lately counts as opened. Remote ports this high are usually
        // the far end of a connection to a service here, so those are left out.
        for f in &flows {
            let lan_udp = f.conn.place == Place::Lan && udp.contains(&(f.conn.pid, f.conn.remote, f.conn.port));
            let known = news.told(&f.conn) || news.open_now(&f.conn);
            news.tell(&f.conn);
            if !known && count > FLOW_WARMUP && f.conn.port < 32768 && !lan_udp {
                opened.push(f.conn.clone());
            }
        }
        procs.forget_dead(active.iter().chain(&attempts).map(|c| c.pid).chain(flows.iter().map(|f| f.conn.pid)));

        let tick = Tick { rx, tx, opened, active, attempts, flows, sized: etw.as_ref().is_some_and(Etw::alive), listeners };
        if out.send(tick).is_err() {
            return;
        }
    }
}

// Virtual adapters (VPN tunnels, Hyper-V switches) are skipped: their traffic
// also crosses a physical adapter, and counting both would double it.
fn adapter_bytes() -> std::collections::HashMap<u32, (u64, u64)> {
    let mut bytes = std::collections::HashMap::new();
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIfTable2(&mut table) } != 0 || table.is_null() {
        return bytes;
    }
    unsafe {
        let n = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), n);
        for row in rows {
            let hardware = row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            if hardware && row.OperStatus == IF_OPER_STATUS_UP && row.Type != IF_TYPE_SOFTWARE_LOOPBACK {
                bytes.insert(row.InterfaceIndex, (row.InOctets, row.OutOctets));
            }
        }
        FreeMibTable(table as *const c_void);
    }
    bytes
}

fn tcp_table(family: u32) -> Vec<u8> {
    let mut size = 0u32;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let rc = unsafe {
            GetExtendedTcpTable(buf.as_mut_ptr() as *mut c_void, &mut size, 0, family, TCP_TABLE_OWNER_PID_ALL, 0)
        };
        match rc {
            0 => return buf,
            ERROR_INSUFFICIENT_BUFFER => buf.resize(size as usize + 4096, 0),
            _ => return Vec::new(),
        }
    }
}

fn u32_at(buf: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(buf[at..at + 4].try_into().unwrap())
}

// Ports are stored in network order in the low two bytes of a DWORD.
fn port_at(buf: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([buf[at], buf[at + 1]])
}

// Rows of MIB_TCPTABLE_OWNER_PID (24 bytes: state, local addr, local port,
// remote addr, remote port, pid) as (state, local, local port, remote, remote port, pid).
fn rows_v4(buf: &[u8]) -> impl Iterator<Item = (u32, IpAddr, u16, IpAddr, u16, u32)> + '_ {
    let n = if buf.len() >= 4 { u32_at(buf, 0) as usize } else { 0 };
    (0..n).map(|i| 4 + i * 24).take_while(move |r| r + 24 <= buf.len()).map(move |r| {
        let local = IpAddr::V4(Ipv4Addr::new(buf[r + 4], buf[r + 5], buf[r + 6], buf[r + 7]));
        let remote = IpAddr::V4(Ipv4Addr::new(buf[r + 12], buf[r + 13], buf[r + 14], buf[r + 15]));
        (u32_at(buf, r), local, port_at(buf, r + 8), remote, port_at(buf, r + 16), u32_at(buf, r + 20))
    })
}

// Rows of MIB_TCP6TABLE_OWNER_PID (56 bytes: local addr[16], scope, port,
// remote addr[16], scope, port, state, pid), in the same shape as `rows_v4`.
fn rows_v6(buf: &[u8]) -> impl Iterator<Item = (u32, IpAddr, u16, IpAddr, u16, u32)> + '_ {
    let n = if buf.len() >= 4 { u32_at(buf, 0) as usize } else { 0 };
    let addr = |bytes: &[u8]| {
        let a = Ipv6Addr::from(<[u8; 16]>::try_from(bytes).unwrap());
        a.to_ipv4_mapped().map_or(IpAddr::V6(a), IpAddr::V4)
    };
    (0..n).map(|i| 4 + i * 56).take_while(move |r| r + 56 <= buf.len()).map(move |r| {
        let local = addr(&buf[r..r + 16]);
        let remote = addr(&buf[r + 24..r + 40]);
        (u32_at(buf, r + 48), local, port_at(buf, r + 20), remote, port_at(buf, r + 44), u32_at(buf, r + 52))
    })
}

fn connections(v4: &[u8], v6: &[u8], state: u32, procs: &mut ProcessNames) -> Vec<Conn> {
    // A connection on a port this machine listens on came in.
    let listening: HashSet<u16> = rows_v4(v4).chain(rows_v6(v6)).filter(|row| row.0 == MIB_TCP_STATE_LISTEN).map(|row| row.2).collect();
    rows_v4(v4)
        .chain(rows_v6(v6))
        .filter(|row| row.0 == state && row.5 != std::process::id() && !super::is_helper(row.5))
        .filter_map(|(_, _, local_port, remote, port, pid)| {
            let place = place_of(remote)?;
            let (process, from_temp) = procs.name(pid);
            let incoming = listening.contains(&local_port);
            // What kind of connection it is, is told by the port that was
            // listened on, which for one coming in is this machine's own.
            let kind = Kind::from_port(if incoming { local_port } else { port });
            Some(Conn { pid, process, from_temp, local_port, remote, port, kind, place, incoming })
        })
        .collect()
}

fn listening(v4: &[u8], v6: &[u8], procs: &mut ProcessNames) -> Vec<Listener> {
    let mut seen = HashSet::new();
    rows_v4(v4)
        .chain(rows_v6(v6))
        .filter(|row| row.0 == MIB_TCP_STATE_LISTEN)
        .filter_map(|(_, local, port, _, _, pid)| {
            let exposed = !local.is_loopback();
            seen.insert((pid, port, exposed)).then(|| Listener { process: procs.name(pid).0, port, exposed })
        })
        .collect()
}

pub fn listeners_now() -> Vec<Listener> {
    let mut procs = ProcessNames::default();
    listening(&tcp_table(AF_INET), &tcp_table(AF_INET6), &mut procs)
}

#[derive(Default)]
struct ProcessNames {
    cache: HashMap<u32, (String, bool)>,
}

impl ProcessNames {
    // The process name, and whether it runs out of a temp or downloads folder.
    fn name(&mut self, pid: u32) -> (String, bool) {
        if pid == 4 {
            return ("windows".into(), false);
        }
        // The full path when the process lets us look, for the temp check;
        // otherwise the process list, which names services too.
        self.cache
            .entry(pid)
            .or_insert_with(|| image(pid).or_else(|| listed_name(pid).map(|n| (n, false))).unwrap_or_default())
            .clone()
    }

    // PIDs get reused; drop names for processes no longer seen on the wire.
    fn forget_dead(&mut self, seen: impl Iterator<Item = u32>) {
        let live: HashSet<u32> = seen.collect();
        self.cache.retain(|pid, _| live.contains(pid));
    }
}

pub fn process_of(pid: u32) -> String {
    image(pid).map(|(name, _)| name).or_else(|| listed_name(pid)).unwrap_or_default()
}

fn image(pid: u32) -> Option<(String, bool)> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| describe_image(&String::from_utf16_lossy(&buf[..len as usize])))
    }
}

// The executable name from the process list, which names every process,
// services included, without having to open it.
fn listed_name(pid: u32) -> Option<String> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            if entry.th32ProcessID == pid {
                let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                found = Some(describe_image(&String::from_utf16_lossy(&entry.szExeFile[..end])).0);
                break;
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        found
    }
}

fn describe_image(path: &str) -> (String, bool) {
    let path = path.to_lowercase();
    let from_temp = ["\\appdata\\local\\temp\\", "\\windows\\temp\\", "\\downloads\\"].iter().any(|d| path.contains(d));
    let file = path.rsplit('\\').next().unwrap_or(&path);
    // Also trims renamed leftovers like `claude.exe.old.1789425205392`.
    let name = match file.find(".exe") {
        Some(end) => file[..end].to_string(),
        None => file.to_string(),
    };
    (name, from_temp)
}


#[repr(C)]
struct DnsCacheEntry {
    next: *mut DnsCacheEntry,
    name: *mut u16,
    kind: u16,
    data_len: u16,
    flags: u32,
}

// Undocumented but long-stable; `ipconfig /displaydns` is built on it.
#[link(name = "dnsapi", kind = "raw-dylib")]
unsafe extern "system" {
    fn DnsGetCacheDataTable(entries: *mut *mut DnsCacheEntry) -> i32;
}

// Names in the local DNS cache, and whether each answered through a CNAME.
// Only the names: a cache-only record query for a name the table already
// lists often fails with 9701 right after the lookup, so addresses come from
// resolving the name again, which the cache answers the way it answered the app.
pub fn dns_cache_names() -> HashMap<String, bool> {
    let mut names = HashMap::new();
    unsafe {
        let mut entry: *mut DnsCacheEntry = std::ptr::null_mut();
        if DnsGetCacheDataTable(&mut entry) == 0 {
            return names;
        }
        while !entry.is_null() {
            let e = &*entry;
            if !e.name.is_null() {
                if matches!(e.kind, DNS_TYPE_A | DNS_TYPE_AAAA | DNS_TYPE_CNAME) {
                    let alias = names.entry(wide_to_string(e.name).to_lowercase()).or_insert(false);
                    *alias |= e.kind == DNS_TYPE_CNAME;
                }
                DnsFree(e.name as *const c_void, DnsFreeFlat);
            }
            let next = e.next;
            DnsFree(entry as *const c_void, DnsFreeFlat);
            entry = next;
        }
    }
    names
}

unsafe fn wide_to_string(p: *const u16) -> String {
    unsafe {
        let mut len = 0;
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

#[repr(C)]
struct SockIn4 {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

#[repr(C)]
struct SockIn6 {
    family: u16,
    port: u16,
    flow: u32,
    addr: [u8; 16],
    scope: u32,
}

pub fn reverse_lookup(ip: IpAddr) -> Option<String> {
    static WSA: Once = Once::new();
    WSA.call_once(|| {
        let mut wsa: WSADATA = unsafe { std::mem::zeroed() };
        unsafe { WSAStartup(0x0202, &mut wsa) };
    });
    let mut host = [0u16; 1025];
    let lookup = |sa: *const SOCKADDR, len: usize, host: &mut [u16]| unsafe {
        GetNameInfoW(sa, len as i32, host.as_mut_ptr(), host.len() as u32, std::ptr::null_mut(), 0, NI_NAMEREQD)
    };
    let rc = match ip {
        IpAddr::V4(a) => {
            let sa = SockIn4 { family: AF_INET as u16, port: 0, addr: a.octets(), zero: [0; 8] };
            lookup(&sa as *const _ as *const SOCKADDR, size_of::<SockIn4>(), &mut host)
        }
        IpAddr::V6(a) => {
            let sa = SockIn6 { family: AF_INET6 as u16, port: 0, flow: 0, addr: a.octets(), scope: 0 };
            lookup(&sa as *const _ as *const SOCKADDR, size_of::<SockIn6>(), &mut host)
        }
    };
    if rc != 0 {
        return None;
    }
    let end = host.iter().position(|&c| c == 0).unwrap_or(host.len());
    Some(String::from_utf16_lossy(&host[..end]))
}

// Stops the kernel's network trace session, which outlives the process
// unless it is told to go.
pub fn stop_kernel_trace() {
    super::etw::stop_session();
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn images() {
        assert_eq!(describe_image(r"C:\Program Files\Mozilla Firefox\firefox.exe"), ("firefox".into(), false));
        assert_eq!(describe_image(r"C:\Users\x\AppData\Local\Temp\setup-1234.EXE"), ("setup-1234".into(), true));
        assert_eq!(describe_image(r"C:\Users\x\Downloads\tool.exe"), ("tool".into(), true));
    }

    #[test]
    fn listeners_parse() {
        let mut procs = ProcessNames::default();
        let found = listening(&tcp_table(AF_INET), &tcp_table(AF_INET6), &mut procs);
        let rpc = found.iter().find(|l| l.port == 135).expect("RPC endpoint mapper listens on every Windows box");
        assert_eq!(rpc.process, "svchost", "a service is named without admin");
    }
}
