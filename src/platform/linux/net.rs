use std::collections::{HashMap, HashSet};
use std::ffi::c_char;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::mpsc::Sender;
use std::thread;

use super::diag;
use crate::net::{Conn, Flow, Kind, LISTENERS_EVERY, LOOKS_PER_TICK, Listener, News, TICK, Tick, place_of};

const TCP_ESTABLISHED: u8 = 0x01;
const TCP_SYN_SENT: u8 = 0x02;
const TCP_LISTEN: u8 = 0x0A;

struct Row {
    local: IpAddr,
    local_port: u16,
    remote: IpAddr,
    port: u16,
    state: u8,
    inode: u64,
}

pub fn sample_loop(out: Sender<Tick>) {
    let mut procs = ProcessNames::default();
    let mut last_bytes = adapter_bytes();
    let mut last_carried: Option<diag::Carried> = None;
    let mut news = News::default();
    loop {
        let mut rows = Vec::new();
        let mut active = Vec::new();
        let mut opened: Vec<Conn> = Vec::new();
        let mut attempts: Vec<Conn> = Vec::new();
        for _ in 0..LOOKS_PER_TICK {
            thread::sleep(TICK / LOOKS_PER_TICK);
            rows = tcp_rows();
            procs.learn(&rows);
            let listening: HashSet<u16> = rows.iter().filter(|r| r.state == TCP_LISTEN).map(|r| r.local_port).collect();
            active = connections(&rows, TCP_ESTABLISHED, &listening, &mut procs);
            for c in connections(&rows, TCP_SYN_SENT, &listening, &mut procs) {
                if !attempts.contains(&c) {
                    attempts.push(c);
                }
            }
            for c in news.fresh(&active) {
                if !opened.contains(&c) {
                    opened.push(c);
                }
            }
        }
        let count = news.second();
        opened.retain(|c| {
            let first = !news.told(c);
            news.tell(c);
            first
        });
        let bytes = adapter_bytes();
        let (mut rx, mut tx) = (0, 0);
        for (name, (r, t)) in &bytes {
            if let Some((lr, lt)) = last_bytes.get(name) {
                rx += r.saturating_sub(*lr);
                tx += t.saturating_sub(*lt);
            }
        }
        last_bytes = bytes;
        let carried = diag::socket_bytes();
        let mut flows: Vec<Flow> = Vec::new();
        if let (Some(now), Some(before)) = (carried.as_ref(), last_carried.as_ref()) {
            let listening: HashSet<u16> = rows.iter().filter(|r| r.state == TCP_LISTEN).map(|r| r.local_port).collect();
            for row in rows.iter().filter(|r| r.state == TCP_ESTABLISHED) {
                let (Some((rx_now, tx_now)), Some((rx_then, tx_then))) = (now.get(&row.inode), before.get(&row.inode)) else {
                    continue;
                };
                let (rx, tx) = (rx_now.saturating_sub(*rx_then), tx_now.saturating_sub(*tx_then));
                let Some(conn) = (rx + tx > 0).then(|| conn_of(row, &listening, &mut procs)).flatten() else {
                    continue;
                };
                match flows.iter_mut().find(|f| f.conn == conn) {
                    Some(flow) => (flow.rx, flow.tx) = (flow.rx + rx, flow.tx + tx),
                    None => flows.push(Flow { conn, rx, tx }),
                }
            }
        }
        let sized = carried.is_some();
        last_carried = carried;
        let listeners = (count % LISTENERS_EVERY == 1).then(|| listening_of(&rows, &mut procs));
        procs.forget_dead(rows.iter().map(|r| r.inode));
        let tick = Tick { rx, tx, opened, active, attempts, flows, sized, listeners };
        if out.send(tick).is_err() {
            return;
        }
    }
}

// Bytes (in, out) of each interface that is a piece of hardware. A bridge, a
// tunnel or a veth carries traffic that crosses a real adapter as well, and
// counting both would count it twice. Only a real device has a `device` link
// under /sys/class/net.
fn adapter_bytes() -> HashMap<String, (u64, u64)> {
    let mut bytes = HashMap::new();
    let Ok(text) = std::fs::read_to_string("/proc/net/dev") else { return bytes };
    for line in text.lines().skip(2) {
        let Some((name, rest)) = line.split_once(':') else { continue };
        let name = name.trim();
        if !std::path::Path::new(&format!("/sys/class/net/{name}/device")).exists() {
            continue;
        }
        // Parsed by position, so a field that will not read has to keep its
        // place rather than shift every column after it.
        let f: Vec<u64> = rest.split_whitespace().map(|v| v.parse().unwrap_or(0)).collect();
        if f.len() >= 9 {
            bytes.insert(name.to_string(), (f[0], f[8]));
        }
    }
    bytes
}

fn tcp_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (path, v6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        rows.extend(text.lines().skip(1).filter_map(|line| parse_row(line, v6)));
    }
    rows
}

fn parse_row(line: &str, v6: bool) -> Option<Row> {
    let f: Vec<&str> = line.split_whitespace().collect();
    let (local, local_port) = parse_addr(f.get(1)?, v6)?;
    let (remote, port) = parse_addr(f.get(2)?, v6)?;
    let state = u8::from_str_radix(f.get(3)?, 16).ok()?;
    let inode = f.get(9)?.parse().ok()?;
    Some(Row { local, local_port, remote, port, state, inode })
}

// `0100007F:1F90` for 127.0.0.1:8080: the address in host byte order per
// 32-bit word, the port in hex.
fn parse_addr(text: &str, v6: bool) -> Option<(IpAddr, u16)> {
    let (addr, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let ip = if v6 {
        let mut bytes = [0u8; 16];
        for (i, chunk) in addr.as_bytes().chunks(8).enumerate().take(4) {
            let word = u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
            bytes[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        let v6 = Ipv6Addr::from(bytes);
        match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        }
    } else {
        IpAddr::V4(Ipv4Addr::from(u32::from_str_radix(addr, 16).ok()?.to_le_bytes()))
    };
    Some((ip, port))
}

fn connections(rows: &[Row], state: u8, listening: &HashSet<u16>, procs: &mut ProcessNames) -> Vec<Conn> {
    rows.iter().filter(|r| r.state == state).filter_map(|r| conn_of(r, listening, procs)).collect()
}

fn conn_of(r: &Row, listening: &HashSet<u16>, procs: &mut ProcessNames) -> Option<Conn> {
    let place = place_of(r.remote)?;
    let (pid, process, from_temp) = procs.of_inode(r.inode);
    // His own talking is not news, nor is what he asked a helper to fetch.
    if pid == std::process::id() || super::is_helper(pid) {
        return None;
    }
    let incoming = listening.contains(&r.local_port);
    let kind = Kind::from_port(if incoming { r.local_port } else { r.port });
    Some(Conn { pid, process, from_temp, local_port: r.local_port, remote: r.remote, port: r.port, kind, place, incoming })
}

fn listening_of(rows: &[Row], procs: &mut ProcessNames) -> Vec<Listener> {
    // A program bound on both v4 and v6 holds the port twice and is one
    // listener, the way Windows count it.
    let mut seen = HashSet::new();
    rows.iter()
        .filter(|r| r.state == TCP_LISTEN)
        .filter_map(|r| {
            let (_, process, _) = procs.of_inode(r.inode);
            let listener = Listener { process, port: r.local_port, exposed: !r.local.is_loopback() };
            seen.insert((listener.process.clone(), listener.port, listener.exposed)).then_some(listener)
        })
        .collect()
}

pub fn listeners_now() -> Vec<Listener> {
    let rows = tcp_rows();
    let mut procs = ProcessNames::default();
    procs.learn(&rows);
    listening_of(&rows, &mut procs)
}

// Socket inodes to the processes holding them, from /proc/*/fd, which
// this user may read for its own processes; others stay unnamed.
#[derive(Default)]
struct ProcessNames {
    by_inode: HashMap<u64, u32>,
    names: HashMap<u32, (String, bool)>,
}

impl ProcessNames {
    fn learn(&mut self, rows: &[Row]) {
        if rows.iter().all(|r| self.by_inode.contains_key(&r.inode)) {
            return;
        }
        let wanted: HashSet<u64> = rows.iter().filter(|r| !self.by_inode.contains_key(&r.inode)).map(|r| r.inode).collect();
        let Ok(procs) = std::fs::read_dir("/proc") else { return };
        for entry in procs.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else { continue };
            let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else { continue };
            for fd in fds.flatten() {
                let Ok(target) = std::fs::read_link(fd.path()) else { continue };
                let target = target.to_string_lossy();
                if let Some(inode) = target.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']')).and_then(|s| s.parse::<u64>().ok())
                    && wanted.contains(&inode)
                {
                    self.by_inode.insert(inode, pid);
                }
            }
        }
        // A socket of another user's process cannot be traced to it without
        // root, and looking again ten times a second would walk every open
        // file on the machine for ever. It is written down as nobody's, and
        // forgotten with the rest when the socket goes.
        for inode in wanted {
            self.by_inode.entry(inode).or_insert(0);
        }
    }

    // Nobody, as a pid of zero, is the socket that could not be traced to its
    // process: the name is left empty, the way Windows leave it.
    fn of_inode(&mut self, inode: u64) -> (u32, String, bool) {
        let Some(&pid) = self.by_inode.get(&inode).filter(|&&pid| pid != 0) else { return (0, String::new(), false) };
        let (name, from_temp) = self.names.entry(pid).or_insert_with(|| describe(pid)).clone();
        (pid, name, from_temp)
    }

    fn forget_dead(&mut self, seen: impl Iterator<Item = u64>) {
        let live: HashSet<u64> = seen.collect();
        self.by_inode.retain(|inode, _| live.contains(inode));
        let pids: HashSet<u32> = self.by_inode.values().copied().collect();
        self.names.retain(|pid, _| pids.contains(pid));
    }
}

fn describe(pid: u32) -> (String, bool) {
    // A program whose file has been replaced under it, which is every update
    // of anything still running, reads back with a mark on the end.
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default().to_lowercase();
    let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe).to_string();
    // The kernel cuts `comm` off at fifteen characters, so the name of the
    // program it was started from is the better one where there is one.
    let name = std::path::Path::new(&exe)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .or_else(|| std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_lowercase()))
        .unwrap_or_default();
    // The same places Windows count as temporary. A cache directory is not
    // one of them: half the desktop keeps a program there on purpose.
    let from_temp = ["/tmp/", "/var/tmp/", "/downloads/"].iter().any(|d| exe.contains(d));
    (name, from_temp)
}

pub fn process_of(pid: u32) -> String {
    describe(pid).0
}

// Nothing of the resolver's cache can be read here.
pub fn dns_cache_names() -> HashMap<String, bool> {
    HashMap::new()
}

// No kernel trace session to stop here.
pub fn stop_kernel_trace() {}

#[repr(C)]
struct SockAddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

#[repr(C)]
struct SockAddrIn6 {
    family: u16,
    port: u16,
    flow: u32,
    addr: [u8; 16],
    scope: u32,
}

const AF_INET: u16 = 2;
const AF_INET6: u16 = 10;
const NI_NAMEREQD: i32 = 8;

unsafe extern "C" {
    fn getnameinfo(sa: *const u8, salen: u32, host: *mut c_char, hostlen: u32, serv: *mut c_char, servlen: u32, flags: i32) -> i32;
}

pub fn reverse_lookup(ip: IpAddr) -> Option<String> {
    let mut host = [0 as c_char; 1025];
    let rc = match ip {
        IpAddr::V4(a) => {
            let sa = SockAddrIn { family: AF_INET, port: 0, addr: a.octets(), zero: [0; 8] };
            unsafe { getnameinfo(&sa as *const _ as *const u8, size_of::<SockAddrIn>() as u32, host.as_mut_ptr(), host.len() as u32, std::ptr::null_mut(), 0, NI_NAMEREQD) }
        }
        IpAddr::V6(a) => {
            let sa = SockAddrIn6 { family: AF_INET6, port: 0, flow: 0, addr: a.octets(), scope: 0 };
            unsafe { getnameinfo(&sa as *const _ as *const u8, size_of::<SockAddrIn6>() as u32, host.as_mut_ptr(), host.len() as u32, std::ptr::null_mut(), 0, NI_NAMEREQD) }
        }
    };
    if rc != 0 {
        return None;
    }
    let bytes: Vec<u8> = host.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_net_tcp_rows_parse() {
        let row = parse_row("   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 12345 1 0000000000000000 100 0 0 10 0", false).unwrap();
        assert_eq!((row.local, row.local_port, row.state, row.inode), ("127.0.0.1".parse().unwrap(), 8080, TCP_LISTEN, 12345));
        let row = parse_row("   1: 0000000000000000FFFF0000640A0A0A:D2C8 0000000000000000FFFF00000101A8C0:01BB 01 00000000:00000000 00:00000000 00000000  1000        0 777 1 0000000000000000 20 4 30 10 -1", true).unwrap();
        assert_eq!(row.remote, "192.168.1.1".parse::<IpAddr>().unwrap(), "a mapped v4 reads as v4");
        assert_eq!((row.port, row.state), (443, TCP_ESTABLISHED));
    }
}
