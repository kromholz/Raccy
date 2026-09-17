pub mod services;

use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::platform;

pub const TICK: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    Lan,
    Internet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Web,
    PlainWeb,
    Ssh,
    RemoteDesktop,
    WinRm,
    Dns,
    Mail,
    FileShare,
    Database,
    Git,
    Chat,
    Call,
    Game,
    Other(u16),
}

// Read both by Kind::from_port here and by the watch, which asks a different
// question about the same ports.
pub const DATABASE_PORTS: &[u16] = &[1433, 3306, 5432, 6379, 9200, 11211, 27017];

impl Kind {
    pub fn from_port(port: u16) -> Kind {
        if DATABASE_PORTS.contains(&port) {
            return Kind::Database;
        }
        match port {
            443 | 8443 => Kind::Web,
            80 | 8080 | 8000 => Kind::PlainWeb,
            22 => Kind::Ssh,
            3389 => Kind::RemoteDesktop,
            5985 | 5986 => Kind::WinRm,
            53 | 853 => Kind::Dns,
            25 | 110 | 143 | 465 | 587 | 993 | 995 => Kind::Mail,
            139 | 445 | 2049 => Kind::FileShare,
            9418 => Kind::Git,
            5222 | 5223 | 6667 | 6697 => Kind::Chat,
            3478 | 5349 | 19302..=19309 => Kind::Call,
            27015..=27050 | 3074 | 25565 => Kind::Game,
            p => Kind::Other(p),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Conn {
    pub pid: u32,
    // Empty when the system would not say which process it is.
    pub process: String,
    pub from_temp: bool,
    pub local_port: u16,
    pub remote: IpAddr,
    pub port: u16,
    pub kind: Kind,
    pub place: Place,
    pub incoming: bool,
}

impl Conn {
    pub fn incoming(&self) -> bool {
        self.incoming
    }
}

#[derive(Clone, Debug)]
pub struct Flow {
    pub conn: Conn,
    pub rx: u64,
    pub tx: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Listener {
    pub process: String,
    pub port: u16,
    pub exposed: bool,
}

pub struct Tick {
    pub rx: u64,
    pub tx: u64,
    pub opened: Vec<Conn>,
    pub active: Vec<Conn>,
    pub flows: Vec<Flow>,
    pub sized: bool,
    pub attempts: Vec<Conn>,
    pub listeners: Option<Vec<Listener>>,
}


pub const LOOKS_PER_TICK: u32 = 10;
pub const LISTENERS_EVERY: u64 = 60;
const NEWS_AGAIN_SECS: u64 = 120;

type Key = (u32, IpAddr, u16);

fn key(conn: &Conn) -> Key {
    (conn.pid, conn.remote, conn.port)
}

#[derive(Default)]
pub struct News {
    open: Option<std::collections::HashSet<Key>>,
    told: HashMap<Key, u64>,
    seconds: u64,
}

impl News {
    pub fn fresh(&mut self, active: &[Conn]) -> Vec<Conn> {
        let now: std::collections::HashSet<Key> = active.iter().map(key).collect();
        let fresh = match self.open.take() {
            Some(before) => active.iter().filter(|c| !before.contains(&key(c))).cloned().collect(),
            None => Vec::new(),
        };
        self.open = Some(now);
        fresh
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn open_now(&self, conn: &Conn) -> bool {
        self.open.as_ref().is_some_and(|open| open.contains(&key(conn)))
    }

    pub fn told(&self, conn: &Conn) -> bool {
        self.told.contains_key(&key(conn))
    }

    pub fn tell(&mut self, conn: &Conn) {
        self.told.insert(key(conn), self.seconds);
    }

    pub fn second(&mut self) -> u64 {
        self.seconds += 1;
        let (seconds, kept) = (self.seconds, NEWS_AGAIN_SECS);
        self.told.retain(|_, at| seconds - *at <= kept);
        self.seconds
    }
}
pub fn start() -> (Receiver<Tick>, Names) {
    let (tx, rx) = mpsc::channel();
    let names = Names::start();
    thread::Builder::new().name("raccy-net".into()).spawn(move || platform::net::sample_loop(tx)).expect("spawn sampler");
    (rx, names)
}

pub fn listeners_now() -> Vec<Listener> {
    platform::net::listeners_now()
}

pub fn process_of(pid: u32) -> String {
    platform::net::process_of(pid)
}

pub fn reverse_lookup(ip: IpAddr) -> Option<String> {
    platform::net::reverse_lookup(ip)
}

const OWN_REFRESH: Duration = Duration::from_secs(30);

pub fn is_own(ip: IpAddr) -> bool {
    static OWN: Mutex<Option<(Instant, Vec<IpAddr>)>> = Mutex::new(None);
    if ip.is_loopback() {
        return true;
    }
    {
        let own = OWN.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, ips)) = own.as_ref()
            && at.elapsed() < OWN_REFRESH
        {
            return ips.contains(&ip);
        }
    }
    // Reading the addresses runs subprocesses on Linux and this is called from
    // the drawing thread, so the lock is only held around the cached list. Two
    // threads refreshing at once do the work twice and agree on the answer.
    let ips = crate::tools::own_addresses();
    let own = ips.contains(&ip);
    *OWN.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), ips));
    own
}

pub fn place_of(ip: IpAddr) -> Option<Place> {
    match ip {
        IpAddr::V4(a) if a.is_loopback() || a.is_unspecified() || a.is_multicast() || a.is_broadcast() => None,
        IpAddr::V6(a) if a.is_loopback() || a.is_unspecified() || a.is_multicast() => None,
        IpAddr::V4(a) if a.is_private() || a.is_link_local() => Some(Place::Lan),
        IpAddr::V4(a) if a.octets()[0] == 100 && (a.octets()[1] & 0xC0) == 64 => Some(Place::Lan),
        IpAddr::V6(a) if (a.segments()[0] & 0xFE00) == 0xFC00 => Some(Place::Lan),
        IpAddr::V6(a) if (a.segments()[0] & 0xFFC0) == 0xFE80 => Some(Place::Lan),
        _ => Some(Place::Internet),
    }
}


const NAME_RETRY: Duration = Duration::from_secs(300);
const NAMES_KEPT: usize = 8192;

#[derive(Clone)]
pub struct Names {
    cache: Arc<Mutex<HashMap<IpAddr, Result<String, Instant>>>>,
    ask: Sender<IpAddr>,
}

impl Names {
    fn start() -> Names {
        let cache: Arc<Mutex<HashMap<IpAddr, Result<String, Instant>>>> = Arc::default();
        let (ask, jobs) = mpsc::channel::<IpAddr>();
        let shared = cache.clone();
        thread::Builder::new()
            .name("raccy-dns".into())
            .spawn(move || {
                let mut forward = Forward::default();
                let mut looked_up: HashMap<IpAddr, String> = HashMap::new();
                let mut refreshed: Option<Instant> = None;
                let mut waiting: Vec<(IpAddr, Instant)> = Vec::new();
                loop {
                    match jobs.recv_timeout(Duration::from_millis(250)) {
                        Ok(ip) => waiting.push((ip, Instant::now())),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                    if waiting.is_empty() {
                        continue;
                    }
                    // The name the app itself asked for (github.com) beats the
                    // reverse record of a CDN edge. A fresh lookup lands in the
                    // cache a moment after the connection opens, so keep
                    // checking the cache before settling for reverse DNS.
                    if refreshed.is_none_or(|t| t.elapsed() >= CACHE_REFRESH) {
                        looked_up = forward.refresh();
                        refreshed = Some(Instant::now());
                    }
                    let mut still = Vec::new();
                    for (ip, since) in waiting.drain(..) {
                        let name = match looked_up.get(&ip) {
                            Some(name) => Some(name.clone()),
                            None if since.elapsed() >= REVERSE_AFTER => platform::net::reverse_lookup(ip),
                            None => {
                                still.push((ip, since));
                                continue;
                            }
                        };
                        let found = name.map(|n| n.trim_end_matches('.').to_lowercase()).ok_or_else(Instant::now);
                        shared.lock().unwrap_or_else(|e| e.into_inner()).insert(ip, found);
                    }
                    waiting = still;
                }
            })
            .expect("spawn resolver");
        Names { cache, ask }
    }

    pub fn get(&self, ip: IpAddr) -> Option<String> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match cache.get(&ip) {
            Some(Ok(name)) => Some(name.clone()),
            Some(Err(since)) if since.elapsed() < NAME_RETRY => None,
            _ => {
                if cache.len() >= NAMES_KEPT {
                    cache.clear();
                }
                cache.insert(ip, Err(Instant::now()));
                let _ = self.ask.send(ip);
                None
            }
        }
    }
}

const CACHE_REFRESH: Duration = Duration::from_millis(500);
const REVERSE_AFTER: Duration = Duration::from_millis(1500);

const FORWARD_KEEP: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Forward {
    resolved: HashMap<String, (Vec<IpAddr>, bool, Instant)>,
}

impl Forward {
    fn refresh(&mut self) -> HashMap<IpAddr, String> {
        let table = platform::net::dns_cache_names();
        self.resolved.retain(|name, _| table.contains_key(name));
        for (name, alias) in table {
            let stale = self.resolved.get(&name).is_none_or(|(_, _, at)| at.elapsed() >= FORWARD_KEEP);
            if stale {
                let addrs = (name.as_str(), 0).to_socket_addrs().map(|a| a.map(|s| s.ip()).collect()).unwrap_or_default();
                self.resolved.insert(name, (addrs, alias, Instant::now()));
            }
        }
        let mut map = HashMap::new();
        for (name, (addrs, alias, _)) in &self.resolved {
            for ip in addrs {
                if *alias {
                    map.insert(*ip, name.clone());
                } else {
                    map.entry(*ip).or_insert_with(|| name.clone());
                }
            }
        }
        map
    }
}

pub fn short_domain(host: &str) -> String {
    let labels: Vec<&str> = host.trim_end_matches('.').split('.').collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    let n = labels.len();
    let second = labels[n - 2];
    let keep = if labels[n - 1].len() == 2 && matches!(second, "co" | "com" | "org" | "net" | "ac" | "gov" | "edu") {
        3
    } else {
        2
    };
    labels[n - keep..].join(".")
}

#[cfg(test)]
impl Names {
    pub fn known(pairs: &[(IpAddr, &str)]) -> Names {
        let cache = pairs.iter().map(|(ip, name)| (*ip, Ok(name.to_string()))).collect();
        let (ask, _) = mpsc::channel();
        Names { cache: Arc::new(Mutex::new(cache)), ask }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn news_is_a_connection_that_was_not_there_before() {
        let conn = |pid: u32| Conn {
            pid,
            process: "curl".into(),
            from_temp: false,
            local_port: 0,
            remote: "1.1.1.1".parse().unwrap(),
            port: 443,
            kind: Kind::Web,
            place: Place::Internet,
            incoming: false,
        };
        let (first, second) = (conn(1), conn(2));
        let mut news = News::default();
        assert!(news.fresh(std::slice::from_ref(&first)).is_empty(), "what was open when he started is nobody's news");
        assert!(news.fresh(std::slice::from_ref(&first)).is_empty(), "still open, still not news");
        assert_eq!(news.fresh(&[first.clone(), second.clone()]), vec![second.clone()]);
        assert!(news.open_now(&first) && news.open_now(&second));
        news.second();
        assert!(!news.told(&second));
        news.tell(&second);
        assert!(news.told(&second), "said once is said");
    }

    // Only Windows hand over what their resolver has cached.
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn cached_names_map_back_to_what_was_asked() {
        let mut forward = Forward::default();
        for asked in ["example.com", "www.wikipedia.org"] {
            let addrs: Vec<IpAddr> = (asked, 443).to_socket_addrs().unwrap().map(|a| a.ip()).collect();
            let map = forward.refresh();
            let named: Vec<_> = addrs.iter().map(|ip| map.get(ip)).collect();
            println!("{asked}: {named:?}");
            assert!(named.iter().any(|n| n.is_some_and(|n| n == asked)), "{asked}");
        }
    }

    #[test]
    fn short_domains() {
        assert_eq!(short_domain("lhr48s29-in-f14.1e100.net"), "1e100.net");
        assert_eq!(short_domain("www.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(short_domain("github.com"), "github.com");
        assert_eq!(short_domain("a.b.seznam.cz."), "seznam.cz");
    }

    #[test]
    fn places() {
        assert_eq!(place_of("127.0.0.1".parse().unwrap()), None);
        assert_eq!(place_of("239.255.255.250".parse().unwrap()), None);
        assert_eq!(place_of("192.168.1.20".parse().unwrap()), Some(Place::Lan));
        assert_eq!(place_of("100.64.0.1".parse().unwrap()), Some(Place::Lan));
        assert_eq!(place_of("fe80::1".parse().unwrap()), Some(Place::Lan));
        assert_eq!(place_of("203.0.113.10".parse().unwrap()), Some(Place::Internet));
    }

    #[test]
    fn ports() {
        assert_eq!(Kind::from_port(443), Kind::Web);
        assert_eq!(Kind::from_port(5985), Kind::WinRm);
        assert_eq!(Kind::from_port(12345), Kind::Other(12345));
    }

    #[test]
    fn every_database_port_is_a_database() {
        for &port in DATABASE_PORTS {
            assert_eq!(Kind::from_port(port), Kind::Database, "{port}");
        }
        assert_eq!(Kind::from_port(9200), Kind::Database, "elasticsearch was only on the watch's list");
        assert_eq!(Kind::from_port(11211), Kind::Database, "memcached was only on the watch's list");
    }
}
