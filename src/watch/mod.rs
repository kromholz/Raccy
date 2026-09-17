pub mod autoruns;
pub mod lan;
pub mod redirects;
pub mod wire;

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;

use crate::net::{Conn, Names, Place, Tick};
use crate::tuning::tuning;
use crate::detect::detect;
use crate::net::services;

const FORGET_AFTER: u64 = 3600;

// The lists are lowercase; process names on Linux keep their capitals.
fn listed(list: &[String], item: &str) -> bool {
    list.iter().any(|entry| entry.eq_ignore_ascii_case(item))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Note,
    Warn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exposure {
    RemoteDesktop,
    Vnc,
    FileShare,
    Telnet,
    Ftp,
    Database,
    DockerApi,
    WinRm,
    DevServer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cleartext {
    Http,
    Ftp,
    Pop3,
    Imap,
    Ldap,
    Mqtt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Oddity {
    Punycode,
    CheapTld,
    RandomLabel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Remote {
    Ssh,
    Rdp,
    Telnet,
    Vnc,
    WinRm,
}

impl Remote {
    pub fn from_port(port: u16) -> Option<Remote> {
        Some(match port {
            22 => Remote::Ssh,
            3389 => Remote::Rdp,
            23 => Remote::Telnet,
            5900..=5903 => Remote::Vnc,
            5985 | 5986 => Remote::WinRm,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Finding {
    Exposed { process: String, port: u16, what: Exposure, again: bool },
    Unexposed { process: String, port: u16, what: Exposure },
    Cleartext { process: String, remote: IpAddr, what: Cleartext },
    Lolbin { process: String, remote: IpAddr },
    FromTemp { process: String, remote: IpAddr },
    Beacon { process: String, remote: IpAddr, every: u64 },
    Sweep { process: String, hosts: usize, ports: usize },
    Upload { rate: u64, secs: u32, owner: Option<(String, IpAddr)>, away: bool },
    Newcomer { process: String, remote: IpAddr },
    Tor { process: String },
    Mining { process: String, remote: IpAddr },
    OddName { process: String, name: String, why: Oddity },
    Session { process: String, remote: IpAddr, what: Remote, incoming: bool, started: u64, lasted: Option<u64> },
    DnsBypass { process: String, remote: IpAddr },
    NewDevice { ip: std::net::Ipv4Addr, mac: String, name: Option<String>, random: bool },
    GatewaySpoof { gateway: std::net::Ipv4Addr, mac: String, posing: std::net::Ipv4Addr, name: Option<String> },
    GatewayChanged { gateway: std::net::Ipv4Addr, mac: String },
    GatewayFlapped { gateway: std::net::Ipv4Addr, mac: String },
    Offline { started: u64, local: bool },
    Online { started: u64, secs: u64 },
    NewAutorun { place: crate::watch::autoruns::Place, name: String },
    HostsAdded { name: String, ip: String, more: usize },
    ProxySet { via: String },
    DnsChanged { adapter: String, servers: String },
}

impl Finding {
    pub fn severity(&self) -> Severity {
        match self {
            Finding::Exposed { what: Exposure::FileShare | Exposure::DevServer | Exposure::WinRm, .. } => Severity::Note,
            Finding::Exposed { .. } => Severity::Warn,
            Finding::Cleartext { what: Cleartext::Http, .. } => Severity::Note,
            Finding::Cleartext { .. } => Severity::Warn,
            Finding::Lolbin { process, .. } if listed(&detect().watch.shells, process) => Severity::Note,
            Finding::Lolbin { .. } => Severity::Warn,
            Finding::Sweep { .. } | Finding::Mining { .. } | Finding::GatewaySpoof { .. } | Finding::GatewayFlapped { .. } => Severity::Warn,
            Finding::Upload { away: true, .. } => Severity::Warn,
            Finding::Session { incoming: true, lasted: None, remote, .. }
                if crate::net::place_of(*remote) == Some(Place::Internet) =>
            {
                Severity::Warn
            }
            Finding::Session { what: Remote::Telnet, incoming: false, lasted: None, .. } => Severity::Warn,
            _ => Severity::Note,
        }
    }

    pub fn process(&self) -> Option<&str> {
        match self {
            Finding::Exposed { process, .. }
            | Finding::Unexposed { process, .. }
            | Finding::Cleartext { process, .. }
            | Finding::Lolbin { process, .. }
            | Finding::FromTemp { process, .. }
            | Finding::Beacon { process, .. }
            | Finding::Sweep { process, .. }
            | Finding::Newcomer { process, .. }
            | Finding::Tor { process }
            | Finding::Mining { process, .. }
            | Finding::OddName { process, .. }
            | Finding::Session { process, .. }
            | Finding::DnsBypass { process, .. } => Some(process),
            Finding::Upload { owner, .. } => owner.as_ref().map(|(p, _)| p.as_str()),
            Finding::NewDevice { .. }
            | Finding::GatewaySpoof { .. }
            | Finding::GatewayChanged { .. }
            | Finding::GatewayFlapped { .. }
            | Finding::Offline { .. }
            | Finding::Online { .. }
            | Finding::NewAutorun { .. }
            | Finding::HostsAdded { .. }
            | Finding::ProxySet { .. }
            | Finding::DnsChanged { .. } => None,
        }
    }

    pub fn remote(&self) -> Option<IpAddr> {
        match self {
            Finding::Cleartext { remote, .. }
            | Finding::Lolbin { remote, .. }
            | Finding::FromTemp { remote, .. }
            | Finding::Beacon { remote, .. }
            | Finding::Newcomer { remote, .. }
            | Finding::Mining { remote, .. }
            | Finding::Session { remote, .. }
            | Finding::DnsBypass { remote, .. } => Some(*remote),
            Finding::Upload { owner, .. } => owner.as_ref().map(|(_, ip)| *ip),
            Finding::Exposed { .. }
            | Finding::Unexposed { .. }
            | Finding::Sweep { .. }
            | Finding::Tor { .. }
            | Finding::OddName { .. }
            | Finding::NewDevice { .. }
            | Finding::GatewaySpoof { .. }
            | Finding::GatewayChanged { .. }
            | Finding::GatewayFlapped { .. }
            | Finding::Offline { .. }
            | Finding::Online { .. }
            | Finding::NewAutorun { .. }
            | Finding::HostsAdded { .. }
            | Finding::ProxySet { .. }
            | Finding::DnsChanged { .. } => None,
        }
    }

    pub fn key(&self) -> String {
        match self {
            // File sharing listens on several ports at once: one story.
            Finding::Exposed { process, what: Exposure::FileShare, .. } => format!("exposed:{process}:FileShare"),
            Finding::Exposed { process, what, port, .. } => format!("exposed:{process}:{what:?}:{port}"),
            Finding::Unexposed { process, what: Exposure::FileShare, .. } => format!("unexposed:{process}:FileShare"),
            Finding::Unexposed { process, what, port } => format!("unexposed:{process}:{what:?}:{port}"),
            Finding::Cleartext { process, what, .. } => format!("cleartext:{process}:{what:?}"),
            Finding::Lolbin { process, .. } => format!("lolbin:{process}"),
            Finding::FromTemp { process, .. } => format!("temp:{process}"),
            Finding::Beacon { process, remote, .. } => format!("beacon:{process}:{remote}"),
            Finding::Sweep { process, .. } => format!("sweep:{process}"),
            Finding::Upload { owner, away, .. } => format!("upload:{away}:{}", owner.as_ref().map_or("", |(p, _)| p.as_str())),
            Finding::Newcomer { process, .. } => format!("newcomer:{process}"),
            Finding::Tor { process } => format!("tor:{process}"),
            Finding::Mining { process, .. } => format!("mining:{process}"),
            Finding::OddName { name, .. } => format!("odd:{name}"),
            Finding::Session { process, remote, what, started, lasted, .. } => {
                format!("session:{process}:{remote}:{what:?}:{started}:{}", lasted.is_some())
            }
            Finding::DnsBypass { process, .. } => format!("dns:{process}"),
            Finding::NewDevice { mac, .. } => format!("device:{mac}"),
            Finding::GatewaySpoof { mac, .. } => format!("spoof:{mac}"),
            Finding::GatewayChanged { mac, .. } => format!("gateway:{mac}"),
            Finding::GatewayFlapped { mac, .. } => format!("gateway-flapped:{mac}"),
            Finding::Offline { started, .. } => format!("offline:{started}"),
            Finding::Online { started, .. } => format!("online:{started}"),
            Finding::NewAutorun { place, name } => format!("autorun:{place:?}:{name}"),
            Finding::HostsAdded { name, .. } => format!("hosts:{name}"),
            Finding::ProxySet { via } => format!("proxy:{via}"),
            Finding::DnsChanged { adapter, servers } => format!("dns-changed:{adapter}:{servers}"),
        }
    }
}

struct Live {
    started: u64,
    last_seen: u64,
    reported: bool,
}

#[derive(Default)]
pub struct Watch {
    clock: u64,
    opens: HashMap<(String, IpAddr), VecDeque<u64>>,
    reach: HashMap<String, VecDeque<(u64, IpAddr, u16)>>,
    upload_secs: u32,
    upload_bytes: u64,
    upload_owners: HashMap<(String, IpAddr), u64>,
    naming: VecDeque<(Conn, u64)>,
    sessions: HashMap<(String, IpAddr, Remote, bool), Live>,
}

impl Watch {
    // known is the processes seen on the internet before; while learning, newcomers join it without comment.
    pub fn tick(
        &mut self,
        tick: &Tick,
        names: &Names,
        idle_secs: u64,
        known: &mut HashSet<String>,
        learning: bool,
    ) -> Vec<Finding> {
        self.clock += 1;
        let mut out = Vec::new();
        if let Some(listeners) = &tick.listeners {
            for l in listeners.iter().filter(|l| l.exposed) {
                if let Some(what) = exposure(l.port) {
                    out.push(Finding::Exposed { process: l.process.clone(), port: l.port, what, again: false });
                }
            }
        }
        for c in &tick.opened {
            if c.place == Place::Internet && !c.incoming() {
                // Ask for the name now; the odd-name check reads it once it is in.
                names.get(c.remote);
            }
            self.opened(c, known, learning, &mut out);
        }
        self.sessions(&tick.active, &mut out);
        for c in tick.opened.iter().chain(&tick.attempts) {
            self.sweep(c, &mut out);
        }
        for f in &tick.flows {
            let c = &f.conn;
            if c.place == Place::Internet && c.port == 53 && f.tx > 0 && !listed(&detect().watch.resolvers, &c.process) {
                out.push(Finding::DnsBypass { process: c.process.clone(), remote: c.remote });
            }
        }
        self.upload(tick, idle_secs, &mut out);
        self.name_checks(names, &mut out);
        if self.clock.is_multiple_of(600) {
            let clock = self.clock;
            self.opens.retain(|_, times| times.back().is_some_and(|&t| clock - t < FORGET_AFTER));
            self.reach.retain(|_, seen| seen.back().is_some_and(|&(t, _, _)| clock - t < tuning().watch.sweep_window_secs));
        }
        out
    }

    fn opened(&mut self, c: &Conn, known: &mut HashSet<String>, learning: bool, out: &mut Vec<Finding>) {
        if c.place != Place::Internet || c.incoming() {
            return;
        }
        let process = c.process.clone();
        let p = process.as_str();
        if let Some(what) = cleartext(c.port) {
            // Certificate revocation checks and updates use plain http by design.
            let system = listed(&detect().watch.system_itself, p);
            if !(what == Cleartext::Http && system) {
                out.push(Finding::Cleartext { process: process.clone(), remote: c.remote, what });
            }
        }
        if listed(&detect().watch.system_tools, p) || listed(&detect().watch.shells, p) {
            out.push(Finding::Lolbin { process: process.clone(), remote: c.remote });
        }
        if c.from_temp {
            out.push(Finding::FromTemp { process: process.clone(), remote: c.remote });
        }
        if detect().watch.tor_ports.contains(&c.port) || p.eq_ignore_ascii_case("tor") {
            out.push(Finding::Tor { process: process.clone() });
        }
        if detect().watch.mining_ports.contains(&c.port) {
            out.push(Finding::Mining { process: process.clone(), remote: c.remote });
        }
        if matches!(c.port, 53 | 853) && !listed(&detect().watch.resolvers, p) {
            out.push(Finding::DnsBypass { process: process.clone(), remote: c.remote });
        }
        if !p.is_empty() && known.insert(without_hash(&process)) && !learning {
            out.push(Finding::Newcomer { process: process.clone(), remote: c.remote });
        }

        let times = self.opens.entry((process.clone(), c.remote)).or_default();
        times.push_back(self.clock);
        if times.len() > tuning().watch.beacon_opens {
            times.pop_front();
        }
        if times.len() == tuning().watch.beacon_opens
            && let Some(every) = steady(times)
        {
            out.push(Finding::Beacon { process, remote: c.remote, every });
        }
        self.naming.push_back((c.clone(), self.clock));
    }

    fn sessions(&mut self, active: &[Conn], out: &mut Vec<Finding>) {
        let clock = self.clock;
        for c in active {
            let incoming = c.incoming();
            let Some(what) = Remote::from_port(if incoming { c.local_port } else { c.port }) else { continue };
            let key = (c.process.clone(), c.remote, what, incoming);
            let live = self.sessions.entry(key).or_insert(Live { started: clock, last_seen: clock, reported: false });
            live.last_seen = clock;
            if !live.reported && clock - live.started >= tuning().watch.session_confirm_secs {
                live.reported = true;
                let (process, remote, started) = (c.process.clone(), c.remote, live.started);
                out.push(Finding::Session { process, remote, what, incoming, started, lasted: None });
            }
        }
        let ended: Vec<_> =
            self.sessions.iter().filter(|(_, l)| clock - l.last_seen > tuning().watch.session_grace_secs).map(|(k, _)| k.clone()).collect();
        for key in ended {
            let live = self.sessions.remove(&key).expect("listed above");
            if live.reported {
                let (process, remote, what, incoming) = key;
                let lasted = Some(live.last_seen - live.started);
                out.push(Finding::Session { process, remote, what, incoming, started: live.started, lasted });
            }
        }
    }

    fn sweep(&mut self, c: &Conn, out: &mut Vec<Finding>) {
        if c.place != Place::Lan || c.process.is_empty() || c.incoming() {
            return;
        }
        let clock = self.clock;
        let seen = self.reach.entry(c.process.clone()).or_default();
        while seen.front().is_some_and(|&(t, _, _)| clock - t > tuning().watch.sweep_window_secs) {
            seen.pop_front();
        }
        if !seen.iter().any(|&(_, ip, port)| ip == c.remote && port == c.port) {
            seen.push_back((clock, c.remote, c.port));
        }
        let hosts: HashSet<IpAddr> = seen.iter().map(|s| s.1).collect();
        let mut ports_per_host: HashMap<IpAddr, HashSet<u16>> = HashMap::new();
        for &(_, ip, port) in seen.iter() {
            ports_per_host.entry(ip).or_default().insert(port);
        }
        let ports = ports_per_host.values().map(HashSet::len).max().unwrap_or(0);
        if hosts.len() >= tuning().watch.sweep_hosts || ports >= tuning().watch.sweep_ports {
            out.push(Finding::Sweep { process: c.process.clone(), hosts: hosts.len(), ports });
            seen.clear();
        }
    }

    fn upload(&mut self, tick: &Tick, idle_secs: u64, out: &mut Vec<Finding>) {
        if tick.tx <= tuning().watch.upload_bps || tick.tx <= 3 * tick.rx {
            self.upload_secs = 0;
            self.upload_bytes = 0;
            self.upload_owners.clear();
            return;
        }
        self.upload_secs += 1;
        self.upload_bytes += tick.tx;
        for f in tick.flows.iter().filter(|f| f.tx > 0) {
            *self.upload_owners.entry((f.conn.process.clone(), f.conn.remote)).or_default() += f.tx;
        }
        let secs = self.upload_secs;
        if secs == tuning().watch.upload_secs || (secs > tuning().watch.upload_secs && (secs - tuning().watch.upload_secs).is_multiple_of(tuning().watch.upload_again_secs)) {
            let measured: u64 = self.upload_owners.values().sum();
            let owner = self
                .upload_owners
                .iter()
                .max_by_key(|(_, bytes)| **bytes)
                .filter(|(_, bytes)| measured > 0 && **bytes as f64 >= tuning().watch.owner_share * measured as f64)
                .map(|((process, remote), _)| (process.clone(), *remote));
            let rate = self.upload_bytes / secs as u64;
            out.push(Finding::Upload { rate, secs, owner, away: idle_secs >= tuning().watch.away_secs });
        }
    }

    // Names trail connections.
    fn name_checks(&mut self, names: &Names, out: &mut Vec<Finding>) {
        while let Some((c, at)) = self.naming.front() {
            let name = names.get(c.remote);
            if name.is_none() && self.clock - at < tuning().watch.name_wait_secs {
                break;
            }
            let (c, _) = self.naming.pop_front().expect("front exists");
            if let Some(finding) = name.and_then(|n| odd_name(&c, &n)) {
                out.push(finding);
            }
        }
    }
}

// A program name without the hash some builds carry, like cargo's test
// binaries: `raccy-0123456789abcdef` is `raccy`, and not new every build.
fn without_hash(name: &str) -> String {
    match name.rsplit_once('-') {
        Some((base, hash)) if hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()) => base.to_string(),
        _ => name.to_string(),
    }
}

fn exposure(port: u16) -> Option<Exposure> {
    if crate::net::DATABASE_PORTS.contains(&port) {
        return Some(Exposure::Database);
    }
    Some(match port {
        3389 => Exposure::RemoteDesktop,
        5800 | 5900..=5903 => Exposure::Vnc,
        139 | 445 => Exposure::FileShare,
        23 => Exposure::Telnet,
        21 => Exposure::Ftp,
        2375 => Exposure::DockerApi,
        5985 | 5986 => Exposure::WinRm,
        3000 | 4200 | 5173 | 8000 | 8080 | 8888 => Exposure::DevServer,
        _ => return None,
    })
}

fn cleartext(port: u16) -> Option<Cleartext> {
    Some(match port {
        80 | 8080 => Cleartext::Http,
        21 => Cleartext::Ftp,
        110 => Cleartext::Pop3,
        143 => Cleartext::Imap,
        389 => Cleartext::Ldap,
        1883 => Cleartext::Mqtt,
        _ => return None,
    })
}

fn steady(times: &VecDeque<u64>) -> Option<u64> {
    let gaps: Vec<f64> = times.iter().zip(times.iter().skip(1)).map(|(a, b)| (b - a) as f64).collect();
    let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
    if !(tuning().watch.beacon_min_secs..=tuning().watch.beacon_max_secs).contains(&mean) {
        return None;
    }
    let var = gaps.iter().map(|g| (g - mean).powi(2)).sum::<f64>() / gaps.len() as f64;
    (var.sqrt() / mean < tuning().watch.beacon_jitter).then_some(mean.round() as u64)
}

fn odd_name(c: &Conn, name: &str) -> Option<Finding> {
    let name = name.to_lowercase();
    if detect().watch.mining_words.iter().any(|w| name.contains(w.as_str())) {
        return Some(Finding::Mining { process: c.process.clone(), remote: c.remote });
    }
    if spells_address(&name, c.remote) {
        return None;
    }
    if services::by_domain(&name).is_some() {
        return None;
    }
    let labels: Vec<&str> = name.split('.').collect();
    let why = if labels.iter().any(|l| l.starts_with("xn--")) {
        Oddity::Punycode
    } else if labels.last().is_some_and(|tld| listed(&detect().watch.cheap_tlds, tld)) {
        Oddity::CheapTld
    } else if labels.len() > 2 && random_label(labels[0]) {
        Oddity::RandomLabel
    } else {
        return None;
    };
    Some(Finding::OddName { process: c.process.clone(), name, why })
}

// A name that spells out its own IPv4 address, in order or reversed, like
// dynamic-078-048-100-200.pool.example.de: what a provider's reverse DNS gives.
fn spells_address(name: &str, ip: IpAddr) -> bool {
    let IpAddr::V4(ip) = ip else { return false };
    let numbers: Vec<u32> = name.split(|c: char| !c.is_ascii_digit()).filter_map(|s| s.parse().ok()).collect();
    let octets: Vec<u32> = ip.octets().iter().map(|&o| u32::from(o)).collect();
    let reversed: Vec<u32> = octets.iter().rev().copied().collect();
    numbers.windows(4).any(|w| w == octets.as_slice() || w == reversed.as_slice())
}

// Long labels full of digits or with near-random letter spread, the way
// generated or tunnelled names look.
fn random_label(label: &str) -> bool {
    let digits = label.chars().filter(char::is_ascii_digit).count();
    (label.len() >= 20 && digits >= 5) || (label.len() >= 24 && entropy(label) >= 3.9)
}

fn entropy(s: &str) -> f64 {
    let mut counts: HashMap<char, usize> = HashMap::new();
    for ch in s.chars() {
        *counts.entry(ch).or_default() += 1;
    }
    let n = s.chars().count() as f64;
    counts.values().map(|&k| k as f64 / n).map(|p| -p * p.log2()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{Flow, Kind, Listener};

    const MIB: u64 = 1024 * 1024;

    fn conn(process: &str, remote: &str, port: u16) -> Conn {
        let remote: IpAddr = remote.parse().unwrap();
        let lan = match remote {
            IpAddr::V4(a) => a.is_private(),
            IpAddr::V6(_) => false,
        };
        Conn {
            pid: 1,
            process: process.into(),
            from_temp: false,
            local_port: 50000,
            incoming: false,
            remote,
            port,
            kind: Kind::from_port(port),
            place: if lan { Place::Lan } else { Place::Internet },
        }
    }

    fn tick() -> Tick {
        Tick { rx: 0, tx: 0, opened: vec![], active: vec![], flows: vec![], sized: false, attempts: vec![], listeners: None }
    }

    fn run(watch: &mut Watch, t: &Tick) -> Vec<Finding> {
        let mut known: HashSet<String> = ["curl", "certutil", "pwsh", "tool", "app", "miner", "scanner"].map(String::from).into();
        watch.tick(t, &Names::known(&[]), 0, &mut known, true)
    }

    #[test]
    fn exposed_vnc_warns_but_localhost_is_fine() {
        let mut t = tick();
        t.listeners = Some(vec![
            Listener { process: "tvnserver".into(), port: 5900, exposed: true },
            Listener { process: "postgres".into(), port: 5432, exposed: false },
            Listener { process: "system".into(), port: 445, exposed: true },
        ]);
        let found = run(&mut Watch::default(), &t);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].severity(), Severity::Warn);
        assert_eq!(found[1].severity(), Severity::Note, "file sharing listens on every Windows box");
    }

    #[test]
    fn system_http_is_expected_but_ftp_is_not() {
        let mut t = tick();
        t.opened =
            vec![conn("svchost", "104.18.38.233", 80), conn("NetworkManager", "104.18.38.234", 80), conn("curl", "93.184.216.34", 21)];
        let found = run(&mut Watch::default(), &t);
        assert_eq!(found, vec![Finding::Cleartext {
            process: "curl".into(),
            remote: "93.184.216.34".parse().unwrap(),
            what: Cleartext::Ftp
        }]);
        assert_eq!(found[0].severity(), Severity::Warn);
    }

    #[test]
    fn system_tools_online() {
        let mut t = tick();
        t.opened =
            vec![conn("certutil", "203.0.113.9", 443), conn("pwsh", "140.82.121.4", 443), conn("PowerShell", "140.82.121.5", 443)];
        let found = run(&mut Watch::default(), &t);
        let severities: Vec<Severity> = found.iter().map(Finding::severity).collect();
        assert_eq!(severities, vec![Severity::Warn, Severity::Note, Severity::Note], "a name with capitals is the same program");
    }

    #[test]
    fn steady_opens_are_a_beacon_and_jittery_ones_are_not() {
        let mut watch = Watch::default();
        let mut found = vec![];
        for second in 1..=301 {
            let mut t = tick();
            if second % 60 == 1 {
                t.opened = vec![conn("app", "198.51.100.7", 443)];
            }
            found.extend(run(&mut watch, &t));
        }
        assert!(matches!(found.as_slice(), [Finding::Beacon { every: 60, .. }]), "{found:?}");

        let mut watch = Watch::default();
        let mut found = vec![];
        for (i, second) in [1, 40, 130, 150, 260, 270].iter().enumerate() {
            while watch.clock + 1 < *second {
                run(&mut watch, &tick());
            }
            let mut t = tick();
            t.opened = vec![conn("app", "198.51.100.7", 443)];
            found.extend(run(&mut watch, &t));
            let _ = i;
        }
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn attempts_across_the_lan_are_a_sweep() {
        let mut watch = Watch::default();
        let mut t = tick();
        t.attempts = (1..=25).map(|i| conn("scanner", &format!("192.168.1.{i}"), 22)).collect();
        let found = run(&mut watch, &t);
        assert!(found.iter().any(|f| matches!(f, Finding::Sweep { hosts: 20, .. })), "{found:?}");
    }

    #[test]
    fn long_upload_while_away_names_its_owner() {
        let mut watch = Watch::default();
        let mut known = HashSet::new();
        let mut found = vec![];
        let owner = conn("rclone", "198.51.100.20", 443);
        for _ in 0..tuning().watch.upload_secs {
            let mut t = tick();
            t.tx = 5 * MIB;
            t.rx = 100_000;
            t.flows = vec![Flow { conn: owner.clone(), rx: 0, tx: 5 * MIB }];
            found.extend(watch.tick(&t, &Names::known(&[]), tuning().watch.away_secs, &mut known, true));
        }
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].severity(), Severity::Warn);
        assert!(matches!(&found[0], Finding::Upload { owner: Some((p, _)), .. } if p == "rclone"));
    }

    #[test]
    fn a_build_hash_does_not_make_a_newcomer() {
        assert_eq!(without_hash("raccy-0123456789abcdef"), "raccy");
        assert_eq!(without_hash("nordvpn-service"), "nordvpn-service");
    }

    #[test]
    fn newcomers_after_the_learning_day_only() {
        let mut t = tick();
        t.opened = vec![conn("fresh", "198.51.100.30", 443)];
        let mut known = HashSet::new();
        let names = Names::known(&[]);
        assert!(Watch::default().tick(&t, &names, 0, &mut known, true).is_empty());
        assert!(known.contains("fresh"), "learned quietly");
        let mut t2 = tick();
        t2.opened = vec![conn("fresher", "198.51.100.31", 443)];
        let found = Watch::default().tick(&t2, &names, 0, &mut known, false);
        assert!(matches!(found.as_slice(), [Finding::Newcomer { .. }]), "{found:?}");
    }

    #[test]
    fn odd_names() {
        let c = conn("app", "198.51.100.40", 443);
        let why = |name: &str| match odd_name(&c, name) {
            Some(Finding::OddName { why, .. }) => Some(why),
            _ => None,
        };
        assert_eq!(why("xn--80ak6aa92e.com"), Some(Oddity::Punycode));
        assert_eq!(why("login-update.xyz"), Some(Oddity::CheapTld));
        assert_eq!(why("a8f3k29dk3m20dk39vk2.data.example.com"), Some(Oddity::RandomLabel));
        assert_eq!(why("rr3---sn-2gb7sn7k.googlevideo.com"), None, "known service");
        assert_eq!(why("ib.fio.cz"), None, "private, never named");
        assert_eq!(why("www.example.com"), None);
        assert_eq!(why("dynamic-198-051-100-040.pool.example.de"), None, "a provider's name for the address");
        assert_eq!(why("40.100.51.198.static.example.net"), None, "reversed, as reverse DNS names go");
        assert!(matches!(odd_name(&c, "pool.supportxmr.com"), Some(Finding::Mining { .. })));
    }

    fn session_ticks(watch: &mut Watch, c: &Conn, secs: u64) -> Vec<Finding> {
        let mut found = vec![];
        for _ in 0..secs {
            let mut t = tick();
            t.active = vec![c.clone()];
            found.extend(run(watch, &t));
        }
        for _ in 0..=tuning().watch.session_grace_secs {
            found.extend(run(watch, &tick()));
        }
        found
    }

    #[test]
    fn a_quick_ssh_stays_quiet_and_a_real_one_is_reported_both_ways() {
        let ssh = conn("ssh", "192.168.1.20", 22);
        assert!(session_ticks(&mut Watch::default(), &ssh, 3).is_empty(), "git fetch over ssh");
        let found = session_ticks(&mut Watch::default(), &ssh, 40);
        assert!(
            matches!(found.as_slice(), [Finding::Session { lasted: None, .. }, Finding::Session { lasted: Some(39), .. }]),
            "{found:?}"
        );
        assert_eq!(found[0].severity(), Severity::Note);
    }

    #[test]
    fn telnet_out_and_rdp_in_from_the_internet_are_warnings() {
        let telnet = conn("pwsh", "64.13.139.230", 23);
        let found = session_ticks(&mut Watch::default(), &telnet, 6);
        assert_eq!(found[0].severity(), Severity::Warn);
        let mut rdp_in = conn("svchost", "203.0.113.50", 51234);
        (rdp_in.local_port, rdp_in.incoming) = (3389, true);
        let found = session_ticks(&mut Watch::default(), &rdp_in, 6);
        assert!(matches!(&found[0], Finding::Session { incoming: true, what: Remote::Rdp, .. }), "{found:?}");
        assert_eq!(found[0].severity(), Severity::Warn);
        // Through NAT the far end's port is rewritten low; it is still someone coming in.
        let mut through_nat = conn("svchost", "203.0.113.51", 20000);
        (through_nat.local_port, through_nat.incoming) = (3389, true);
        let found = session_ticks(&mut Watch::default(), &through_nat, 6);
        assert!(matches!(&found[0], Finding::Session { incoming: true, what: Remote::Rdp, .. }), "{found:?}");
    }

    #[test]
    fn a_database_port_is_a_database_to_both_readers_of_the_list() {
        for &port in crate::net::DATABASE_PORTS {
            assert_eq!(exposure(port), Some(Exposure::Database), "{port}");
            assert_eq!(Kind::from_port(port), Kind::Database, "{port}");
        }
    }

    #[test]
    fn mining_port_and_dns_bypass() {
        let mut t = tick();
        t.opened = vec![conn("miner", "198.51.100.50", 3333), conn("tool", "8.8.8.8", 53)];
        let found = run(&mut Watch::default(), &t);
        assert!(found.iter().any(|f| matches!(f, Finding::Mining { .. })), "{found:?}");
        assert!(found.iter().any(|f| matches!(f, Finding::DnsBypass { .. })), "{found:?}");
    }
}
