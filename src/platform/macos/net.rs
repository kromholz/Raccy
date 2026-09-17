use std::collections::{HashMap, HashSet};
use std::ffi::c_char;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use super::{command_output, is_helper};
use crate::net::{Conn, Flow, Kind, LISTENERS_EVERY, LOOKS_PER_TICK, Listener, News, TICK, Tick, place_of};

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Established,
    SynSent,
    Listen,
}

struct Row {
    local: IpAddr,
    local_port: u16,
    remote: IpAddr,
    port: u16,
    state: State,
}

// What both netstat and lsof name a socket by, since nothing here has an
// inode to hold the two lists together the way /proc does.
type Key = (u16, IpAddr, u16);

fn key(row: &Row) -> Key {
    (row.local_port, row.remote, row.port)
}

pub fn sample_loop(out: Sender<Tick>) {
    let mut procs = ProcessNames::default();
    let mut last_bytes = adapter_bytes();
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
            let listening: HashSet<u16> = rows.iter().filter(|r| r.state == State::Listen).map(|r| r.local_port).collect();
            active = connections(&rows, State::Established, &listening, &mut procs);
            for c in connections(&rows, State::SynSent, &listening, &mut procs) {
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
        let listeners = (count % LISTENERS_EVERY == 1).then(|| listening_of(&rows, &mut procs));
        procs.forget_dead(rows.iter().map(key));
        // The kernel keeps no per-socket counters a program may simply read
        // here, so the whole of a tick's traffic is the adapters' and none of
        // it is laid at any one connection's door.
        let tick = Tick { rx, tx, opened, active, attempts, flows: Vec::<Flow>::new(), sized: false, listeners };
        if out.send(tick).is_err() {
            return;
        }
    }
}

// Bytes (in, out) of each interface that is a piece of hardware. `en` is what
// the system calls one: a bridge, a tunnel or the hotspot carries traffic that
// crosses a real adapter as well, and counting both would count it twice.
fn adapter_bytes() -> HashMap<String, (u64, u64)> {
    let mut bytes = HashMap::new();
    let Some(text) = command_output("netstat", &["-ibn"]) else { return bytes };
    let mut lines = text.lines();
    // Which column holds what has moved between releases, so the heading is
    // what says where the two counters are.
    let heading: Vec<&str> = lines.next().unwrap_or_default().split_whitespace().collect();
    let at = |name: &str| heading.iter().position(|h| *h == name);
    let (Some(rx_at), Some(tx_at)) = (at("Ibytes"), at("Obytes")) else { return bytes };
    for line in lines {
        let f: Vec<&str> = line.split_whitespace().collect();
        // One row per address the interface holds, all with the same counters;
        // the one against its own link address is the interface itself. A row
        // short of a field is a row whose columns no longer line up.
        if f.len() != heading.len() || f.get(2).is_none_or(|n| !n.starts_with("<Link#")) {
            continue;
        }
        let name = f[0];
        if let (true, Some(rx), Some(tx)) = (hardware(name), f[rx_at].parse().ok(), f[tx_at].parse().ok()) {
            bytes.insert(name.to_string(), (rx, tx));
        }
    }
    bytes
}

fn hardware(name: &str) -> bool {
    name.strip_prefix("en").is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

fn tcp_rows() -> Vec<Row> {
    let Some(text) = command_output("netstat", &["-an", "-p", "tcp"]) else { return Vec::new() };
    text.lines().filter_map(parse_row).collect()
}

// `tcp4  0  0  192.168.1.10.52341  17.253.144.10.443  ESTABLISHED`, where the
// port is written after a dot rather than a colon, and an address that is not
// bound to anything at all is a star.
fn parse_row(line: &str) -> Option<Row> {
    let f: Vec<&str> = line.split_whitespace().collect();
    if !f.first()?.starts_with("tcp") {
        return None;
    }
    let state = match *f.get(5)? {
        "ESTABLISHED" => State::Established,
        "SYN_SENT" => State::SynSent,
        "LISTEN" => State::Listen,
        _ => return None,
    };
    let (local, local_port) = parse_addr(f.get(3)?)?;
    let (remote, port) = parse_addr(f.get(4)?)?;
    Some(Row { local, local_port, remote, port, state })
}

fn parse_addr(text: &str) -> Option<(IpAddr, u16)> {
    let (addr, port) = text.rsplit_once('.')?;
    let port = if port == "*" { 0 } else { port.parse().ok()? };
    // A v6 address carries the interface it is scoped to; the name is not
    // part of the address.
    let addr = addr.split('%').next().unwrap_or(addr);
    let ip = match addr {
        "*" => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        _ => addr.parse().ok()?,
    };
    Some((ip, port))
}

fn connections(rows: &[Row], state: State, listening: &HashSet<u16>, procs: &mut ProcessNames) -> Vec<Conn> {
    rows.iter().filter(|r| r.state == state).filter_map(|r| conn_of(r, listening, procs)).collect()
}

fn conn_of(r: &Row, listening: &HashSet<u16>, procs: &mut ProcessNames) -> Option<Conn> {
    let place = place_of(r.remote)?;
    let (pid, process, from_temp) = procs.of(key(r))?;
    // His own talking is not news, nor is what he asked a helper to fetch.
    if pid == std::process::id() || is_helper(pid) {
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
        .filter(|r| r.state == State::Listen)
        .filter_map(|r| {
            let (_, process, _) = procs.of(key(r)).unwrap_or_default();
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

// Sockets to the processes holding them, from lsof, which this user may ask
// about its own processes; others stay unnamed.
#[derive(Default)]
struct ProcessNames {
    by_socket: HashMap<Key, u32>,
    names: HashMap<u32, (String, bool)>,
    asked: Option<Instant>,
}

// Walking every open file on the machine is what asking costs here, and a
// connection that opens between two asks is named a fraction of a second late
// rather than ten times a second for ever.
const ASK_EVERY: Duration = Duration::from_secs(1);

impl ProcessNames {
    fn learn(&mut self, rows: &[Row]) {
        if rows.iter().all(|r| self.by_socket.contains_key(&key(r))) || self.asked.is_some_and(|at| at.elapsed() < ASK_EVERY) {
            return;
        }
        self.asked = Some(Instant::now());
        if let Some(said) = command_output("lsof", &["-nP", "-iTCP", "-Fpcn"]) {
            self.by_socket.extend(parse_lsof(&said));
        }
        // A socket of another user's process cannot be traced to it without
        // root. It is written down as nobody's, and forgotten with the rest
        // when the socket goes.
        for row in rows {
            self.by_socket.entry(key(row)).or_insert(0);
        }
    }

    // Nothing at all for a socket nobody has been asked about yet, since a
    // connection told of before its name is known would be told of a second
    // time under that name. Nobody, as a pid of zero, is the one that was
    // asked about and could not be traced: the name is left empty, the way
    // Windows leave it.
    fn of(&mut self, socket: Key) -> Option<(u32, String, bool)> {
        let &pid = self.by_socket.get(&socket)?;
        if pid == 0 {
            return Some((0, String::new(), false));
        }
        let (name, from_temp) = self.names.entry(pid).or_insert_with(|| describe(pid)).clone();
        Some((pid, name, from_temp))
    }

    fn forget_dead(&mut self, seen: impl Iterator<Item = Key>) {
        let live: HashSet<Key> = seen.collect();
        self.by_socket.retain(|socket, _| live.contains(socket));
        let pids: HashSet<u32> = self.by_socket.values().copied().collect();
        self.names.retain(|pid, _| pids.contains(pid));
    }
}

// lsof in fields: a `p` line opens a process, every `n` line under it is one
// of its sockets, written `local->remote` or, for a listener, on its own.
fn parse_lsof(said: &str) -> HashMap<Key, u32> {
    let mut out = HashMap::new();
    let mut pid = 0;
    for line in said.lines() {
        // A name that begins with a character of more than one byte is no
        // field of ours.
        let Some((field, rest)) = line.split_at_checked(1) else { continue };
        match field {
            "p" => pid = rest.parse().unwrap_or(0),
            "n" if pid != 0 => {
                if let Some(socket) = lsof_socket(rest) {
                    out.insert(socket, pid);
                }
            }
            _ => {}
        }
    }
    out
}

fn lsof_socket(name: &str) -> Option<Key> {
    let (here, there) = match name.split_once("->") {
        Some((here, there)) => (here, Some(there)),
        None => (name, None),
    };
    let port = |text: &str| -> Option<u16> { text.rsplit_once(':')?.1.parse().ok() };
    let local_port = port(here)?;
    let Some(there) = there else { return Some((local_port, IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)) };
    let (addr, remote_port) = there.rsplit_once(':')?;
    let addr = addr.trim_start_matches('[').trim_end_matches(']').split('%').next().unwrap_or(addr);
    Some((local_port, addr.parse().ok()?, remote_port.parse().ok()?))
}

// The whole path of a running program, which the kernel keeps and hands over
// without any right beyond being able to see the process at all.
fn describe(pid: u32) -> (String, bool) {
    let mut buf = [0u8; 4096];
    let len = unsafe { proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    let path = String::from_utf8_lossy(&buf[..len.max(0) as usize]).to_lowercase();
    let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // The same places Windows count as temporary. A cache directory is not
    // one of them: half the desktop keeps a program there on purpose.
    let from_temp = ["/tmp/", "/var/tmp/", "/downloads/"].iter().any(|d| path.contains(d));
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

// Every address on this system carries its own length before the family, and
// both are a byte.
#[repr(C)]
struct SockAddrIn {
    len: u8,
    family: u8,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

#[repr(C)]
struct SockAddrIn6 {
    len: u8,
    family: u8,
    port: u16,
    flow: u32,
    addr: [u8; 16],
    scope: u32,
}

const AF_INET: u8 = 2;
const AF_INET6: u8 = 30;
const NI_NAMEREQD: i32 = 4;

unsafe extern "C" {
    fn getnameinfo(sa: *const u8, salen: u32, host: *mut c_char, hostlen: u32, serv: *mut c_char, servlen: u32, flags: i32) -> i32;
    fn proc_pidpath(pid: i32, buffer: *mut c_char, size: u32) -> i32;
}

pub fn reverse_lookup(ip: IpAddr) -> Option<String> {
    let mut host = [0 as c_char; 1025];
    let rc = match ip {
        IpAddr::V4(a) => {
            let size = size_of::<SockAddrIn>();
            let sa = SockAddrIn { len: size as u8, family: AF_INET, port: 0, addr: a.octets(), zero: [0; 8] };
            unsafe { getnameinfo(&sa as *const _ as *const u8, size as u32, host.as_mut_ptr(), host.len() as u32, std::ptr::null_mut(), 0, NI_NAMEREQD) }
        }
        IpAddr::V6(a) => {
            let size = size_of::<SockAddrIn6>();
            let sa = SockAddrIn6 { len: size as u8, family: AF_INET6, port: 0, flow: 0, addr: a.octets(), scope: 0 };
            unsafe { getnameinfo(&sa as *const _ as *const u8, size as u32, host.as_mut_ptr(), host.len() as u32, std::ptr::null_mut(), 0, NI_NAMEREQD) }
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
    fn netstat_rows_parse_with_the_port_after_a_dot() {
        let row = parse_row("tcp4       0      0  192.168.1.10.52341     17.253.144.10.443      ESTABLISHED").expect("a whole row");
        assert_eq!((row.local_port, row.remote, row.port), (52341, "17.253.144.10".parse::<IpAddr>().unwrap(), 443));
        assert!(row.state == State::Established);
        let row = parse_row("tcp6       0      0  *.22                   *.*                    LISTEN").expect("a listener");
        assert_eq!((row.local_port, row.port), (22, 0));
        assert!(row.state == State::Listen);
        assert!(parse_row("tcp4       0      0  10.0.0.1.80            10.0.0.9.5555          TIME_WAIT").is_none(), "a closing socket is nobody talking");
        assert!(parse_row("Active Internet connections (including servers)").is_none());
        let scoped = parse_addr("fe80::1%en0.51000").expect("a scoped address");
        assert_eq!(scoped, ("fe80::1".parse::<IpAddr>().unwrap(), 51000));
    }

    #[test]
    fn lsof_fields_give_the_process_of_each_socket() {
        let said = "p501\ncracoon\nf12\nn192.168.1.10:52341->17.253.144.10:443\nf13\nn*:22\np777\ncother\nf9\nn[fe80::1]:51000->[2606:4700::1]:443\n";
        let found = parse_lsof(said);
        assert_eq!(found.get(&(52341, "17.253.144.10".parse().unwrap(), 443)), Some(&501));
        assert_eq!(found.get(&(22, IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)), Some(&501), "a listener is named by its port alone");
        assert_eq!(found.get(&(51000, "2606:4700::1".parse().unwrap(), 443)), Some(&777), "and a v6 address loses its brackets");
    }

    #[test]
    fn only_a_real_adapter_is_counted() {
        assert!(hardware("en0") && hardware("en12"));
        for shared in ["bridge0", "utun3", "awdl0", "lo0", "ap1", "llw0", "en"] {
            assert!(!hardware(shared), "{shared}");
        }
    }
}
