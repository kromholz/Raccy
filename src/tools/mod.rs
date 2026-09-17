pub mod inspect;

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::lang::Lang;
use crate::platform;
use crate::net::{self, Names, Tick};
use crate::net::services;
use crate::talk;

const PINGS: usize = 3;
const PING_TIMEOUT_MS: u32 = 1000;
const MAX_HOPS: u8 = 15;
const SILENT_HOPS: u8 = 4;
const USAGE_SECS: usize = 60;
const TOP: usize = 8;
const INTERNET: Ipv4Addr = Ipv4Addr::new(1, 1, 1, 1);
const NAME_CHECK: &str = "example.com";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Network,
    WiFi,
    ShareWifi,
    Neighbours,
    Listening,
    Usage,
    CallsHome,
    Ping,
}

impl Tool {
    // In menu order.
    pub const ALL: [Tool; 8] = [
        Tool::Network,
        Tool::WiFi,
        Tool::ShareWifi,
        Tool::Neighbours,
        Tool::Listening,
        Tool::Usage,
        Tool::CallsHome,
        Tool::Ping,
    ];
}

pub struct Report {
    pub title: String,
    pub text: String,
    pub line: String,
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    NoNetwork,
    NoGateway,
    NoInternet,
    NoDns,
    Fine { gateway: Option<u32>, internet: u32 },
}

// "Who eats the bandwidth" is answered by Usage::report, "who calls home" by calls_report.
pub fn run(tool: Tool, lang: Lang) -> Report {
    match tool {
        Tool::Network => network_report(lang),
        Tool::WiFi => wifi_report(lang),
        Tool::ShareWifi => share_wifi_report(lang).0,
        Tool::Neighbours => neighbours_report(lang),
        Tool::Listening => listening_report(lang),
        Tool::Ping => ping_report(lang),
        Tool::Usage => Report { title: lang.tool_name(tool).into(), text: String::new(), line: lang.usage_line(None) },
        Tool::CallsHome => Report { title: lang.tool_name(tool).into(), text: String::new(), line: lang.calls_line(None, 0) },
    }
}

fn salt() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
}

pub fn row(text: &mut String, label: &str, value: &str) {
    text.push_str(&format!("{label:<16}{value}\n"));
}

// A reverse lookup waits on a server and they do not depend on one another,
// so a thread apiece makes a list of them as slow as the slowest, not the sum.
fn names_of(ips: impl IntoIterator<Item = Option<Ipv4Addr>>) -> Vec<Option<String>> {
    let ips: Vec<Option<Ipv4Addr>> = ips.into_iter().collect();
    std::thread::scope(|s| {
        let jobs: Vec<_> = ips.iter().map(|ip| s.spawn(move || net::reverse_lookup(IpAddr::V4((*ip)?)))).collect();
        jobs.into_iter().map(|job| job.join().ok().flatten()).collect()
    })
}

pub fn mac_text(mac: [u8; 6]) -> String {
    mac.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

// A locally administered MAC.
pub fn is_random(mac: [u8; 6]) -> bool {
    mac[0] & 0x02 != 0
}

fn speed(bits: u64) -> String {
    match bits {
        b if b >= 1_000_000_000 => format!("{:.1} Gb/s", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{} Mb/s", b / 1_000_000),
        b => format!("{} kb/s", b / 1000),
    }
}

// ---------------------------------------------------------------- adapters

pub(crate) struct Adapter {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) mac: Option<[u8; 6]>,
    pub(crate) addresses: Vec<(IpAddr, u8)>,
    pub(crate) gateways: Vec<IpAddr>,
    pub(crate) dns: Vec<IpAddr>,
    pub(crate) dhcp: bool,
    pub(crate) speed: u64,
}

impl Adapter {
    fn ipv4(&self) -> Option<(Ipv4Addr, u8)> {
        self.addresses.iter().find_map(|&(ip, prefix)| match ip {
            IpAddr::V4(a) => Some((a, prefix)),
            IpAddr::V6(_) => None,
        })
    }

    fn gateway4(&self) -> Option<Ipv4Addr> {
        first_v4(&self.gateways)
    }

    fn dns4(&self) -> Option<Ipv4Addr> {
        first_v4(&self.dns)
    }
}

fn first_v4(ips: &[IpAddr]) -> Option<Ipv4Addr> {
    ips.iter().find_map(|ip| match ip {
        IpAddr::V4(a) if !a.is_unspecified() => Some(*a),
        _ => None,
    })
}

pub fn dns_servers() -> Vec<IpAddr> {
    platform::tools::adapters().into_iter().flat_map(|a| a.dns).collect()
}

// A network is told by the adapter and its gateways' MACs, which differ
// between two routers on the same address.
pub fn dns_by_network() -> Vec<(String, String, Vec<String>)> {
    let sorted = |mut texts: Vec<String>| {
        texts.sort();
        texts
    };
    let gateway = |ip: &IpAddr| match ip {
        IpAddr::V4(v4) => crate::platform::lan::mac_of(*v4).map_or_else(|| ip.to_string(), mac_text),
        IpAddr::V6(_) => ip.to_string(),
    };
    platform::tools::adapters()
        .into_iter()
        .filter(|a| !a.gateways.is_empty())
        .map(|a| {
            let gateways = sorted(a.gateways.iter().map(gateway).collect());
            let dns = sorted(a.dns.iter().map(IpAddr::to_string).collect());
            (format!("{}|{}", a.name, gateways.join(",")), a.name.clone(), dns)
        })
        .collect()
}

pub fn own_addresses() -> Vec<IpAddr> {
    platform::tools::adapters().into_iter().flat_map(|a| a.addresses.into_iter().map(|(ip, _)| ip)).collect()
}

// Where the caller looks like from outside, which is the one thing about a
// network a machine behind a router cannot see for itself. Asked of the same
// address the packages come from, so there is nobody else in this to trust.
const MIRROR: &str = "https://updates.bohemia.systems/ip";

// The sixth version hides nobody behind anything: an address of the global
// range sitting on an adapter is already the address the world sees, so
// nothing is sent to find that one out.
fn public_ips() -> (Option<Ipv4Addr>, Option<Ipv6Addr>) {
    let v6 = own_addresses().into_iter().find_map(|ip| match ip {
        IpAddr::V6(a) if (a.segments()[0] & 0xE000) == 0x2000 => Some(a),
        _ => None,
    });
    (platform::tools::http_get(MIRROR).and_then(|s| s.trim().parse().ok()), v6)
}

fn network_report(lang: Lang) -> Report {
    let adapters = platform::tools::adapters();
    let (v4, v6) = public_ips();
    let none = lang.tool_label("none");
    let mut text = String::new();
    row(&mut text, lang.tool_label("public_ipv4"), &v4.map_or(none.into(), |ip| ip.to_string()));
    row(&mut text, lang.tool_label("public_ipv6"), &v6.map_or(none.into(), |ip| ip.to_string()));
    if let Some(host) = platform::host::computer_name() {
        row(&mut text, lang.tool_label("computer"), &host);
    }
    let list = |ips: &[IpAddr]| ips.iter().filter(|ip| !ip.is_unspecified()).map(IpAddr::to_string).collect::<Vec<_>>().join(", ");
    for a in adapters.iter().filter(|a| !a.addresses.is_empty()) {
        text.push_str(&format!("\n{}  ({})\n", a.name, a.description));
        for (ip, prefix) in &a.addresses {
            row(&mut text, if ip.is_ipv4() { "  IPv4" } else { "  IPv6" }, &format!("{ip}/{prefix}"));
        }
        let gateways = list(&a.gateways);
        if !gateways.is_empty() {
            row(&mut text, &format!("  {}", lang.tool_label("gateway")), &gateways);
        }
        let dns = list(&a.dns);
        if !dns.is_empty() {
            row(&mut text, "  DNS", &dns);
        }
        if let Some(mac) = a.mac {
            row(&mut text, "  MAC", &mac_text(mac));
        }
        row(&mut text, "  DHCP", if a.dhcp { lang.tool_label("yes") } else { lang.tool_label("static") });
        if a.speed > 0 && a.speed < u64::MAX {
            row(&mut text, &format!("  {}", lang.tool_label("link_speed")), &speed(a.speed));
        }
    }
    let public = v4.map(|ip| ip.to_string()).or(v6.map(|ip| ip.to_string()));
    Report { title: lang.tool_name(Tool::Network).into(), text, line: lang.network_line(public.as_deref(), salt()) }
}

// ---------------------------------------------------------------- wi-fi

pub(crate) enum WifiState {
    NoAdapter,
    Disconnected,
    // Windows 11 24H2 and later, and macOS, give the network name only to
    // apps with location access. Nothing asks that of a program on Linux.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Denied,
    Connected(Wifi),
}

pub(crate) struct Wifi {
    pub(crate) ssid: String,
    pub(crate) profile: String,
    pub(crate) bssid: [u8; 6],
    pub(crate) signal: u32,
    pub(crate) rssi: Option<i32>,
    pub(crate) frequency: Option<u32>,
    pub(crate) channel: Option<u32>,
    pub(crate) rx_mbps: u32,
    pub(crate) tx_mbps: u32,
    pub(crate) auth: i32,
    pub(crate) cipher: i32,
    pub(crate) secured: bool,
}

// DOT11_CIPHER_ALGO_WEP40, WEP104 and WEP.
fn is_wep(cipher: i32) -> bool {
    matches!(cipher, 1 | 5 | 257)
}

fn security(lang: Lang, auth: i32, cipher: i32, secured: bool) -> (String, bool) {
    let wep = is_wep(cipher);
    // DOT11_CIPHER_ALGO_TKIP.
    let tkip = cipher == 2;
    // DOT11_AUTH_ALGO_*: 1 open, 2 shared key, 3 WPA, 4 WPA-PSK, 6 RSNA (WPA2),
    // 7 RSNA-PSK, 8 WPA3 Enterprise 192, 9 WPA3-SAE, 10 OWE, 11 WPA3 Enterprise.
    let (name, weak) = match auth {
        _ if !secured => (lang.tool_label("open_no_password").to_string(), true),
        1 | 2 if wep => ("WEP".to_string(), true),
        1 => (lang.tool_label("open").to_string(), true),
        3 | 4 => ("WPA".to_string(), true),
        6 | 7 if tkip => ("WPA2 + TKIP".to_string(), true),
        6 | 7 => ("WPA2".to_string(), false),
        8 | 9 | 11 => ("WPA3".to_string(), false),
        10 => ("OWE".to_string(), false),
        other => (format!("auth {other}"), false),
    };
    let enterprise = !weak && matches!(auth, 6 | 8 | 11);
    (if enterprise { format!("{name} Enterprise") } else { name }, weak)
}

fn band(mhz: u32) -> String {
    match mhz {
        2400..=2500 => "2.4 GHz".into(),
        4900..=5924 => "5 GHz".into(),
        5925..=7125 => "6 GHz".into(),
        other => format!("{other} MHz"),
    }
}

fn wifi_report(lang: Lang) -> Report {
    let title = lang.tool_name(Tool::WiFi).to_string();
    match platform::tools::wifi() {
        WifiState::Connected(w) => {
            let (security, weak) = security(lang, w.auth, w.cipher, w.secured);
            let mut text = String::new();
            row(&mut text, "SSID", &w.ssid);
            if w.profile != w.ssid {
                row(&mut text, lang.tool_label("profile"), &w.profile);
            }
            row(&mut text, "BSSID", &mac_text(w.bssid));
            let rssi = w.rssi.map_or(String::new(), |r| format!(" ({r} dBm)"));
            row(&mut text, lang.tool_label("signal"), &format!("{} %{rssi}", w.signal));
            let mut radio = w.frequency.map_or(String::new(), band);
            if let Some(channel) = w.channel {
                radio += &format!("{}{} {channel}", if radio.is_empty() { "" } else { ", " }, lang.tool_label("channel"));
            }
            if !radio.is_empty() {
                row(&mut text, lang.tool_label("band"), &radio);
            }
            row(&mut text, lang.tool_label("rate"), &format!("↓{} ↑{} Mb/s", w.rx_mbps, w.tx_mbps));
            row(&mut text, lang.tool_label("security"), &security);
            if weak {
                text.push_str(&format!("\n{}\n", lang.tool_label("weak_security")));
            }
            let line = lang.wifi_line(&w.ssid, w.signal, &security, weak);
            Report { title, text, line }
        }
        // No line of his own: what he would say stands in the window above it.
        WifiState::Denied => {
            Report { title, text: format!("{}\n", lang.tool_label(if cfg!(windows) { "location_needed" } else { "location_needed_here" })), line: String::new() }
        }
        WifiState::Disconnected => Report { title, text: format!("{}\n", lang.tool_label("not_connected")), line: lang.wifi_off().into() },
        WifiState::NoAdapter => Report { title, text: format!("{}\n", lang.tool_label("no_adapter")), line: lang.wifi_none().into() },
    }
}

// ---------------------------------------------------------------- sharing the wi-fi

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sharing {
    Shared,
    Open,
    // Windows give the password in the clear only to an administrator.
    NeedsAdmin,
    Enterprise,
    Unknown,
}

#[derive(Debug, PartialEq)]
enum Key {
    Open,
    Plain(String),
    Protected,
    Enterprise,
}

fn key_of(xml: &str) -> Key {
    let tag = |name: &str| {
        let (open, close) = (format!("<{name}>"), format!("</{name}>"));
        let start = xml.find(&open)? + open.len();
        let end = start + xml[start..].find(&close)?;
        Some(&xml[start..end])
    };
    match (tag("keyMaterial"), tag("protected"), tag("authentication")) {
        (Some(_), Some("true"), _) => Key::Protected,
        (Some(key), _, _) => Key::Plain(xml_text(key)),
        // Enhanced Open (OWE) encrypts without a password: open all the same.
        (None, _, Some("open" | "OWE")) => Key::Open,
        (None, _, _) => Key::Enterprise,
    }
}

fn xml_text(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn wifi_qr_text(ssid: &str, key: Option<&str>, wep: bool) -> String {
    let escape = |s: &str| {
        let mut out = String::new();
        for c in s.chars() {
            if matches!(c, '\\' | ';' | ',' | '"' | ':') {
                out.push('\\');
            }
            out.push(c);
        }
        out
    };
    match key {
        None => format!("WIFI:T:nopass;S:{};;", escape(ssid)),
        Some(key) => format!("WIFI:T:{};S:{};P:{};;", if wep { "WEP" } else { "WPA" }, escape(ssid), escape(key)),
    }
}

fn qr_of(text: &str) -> Option<crate::render::Qr> {
    let code = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium).ok()?;
    let size = code.size() as usize;
    let modules = (0..size * size).map(|i| code.get_module((i % size) as i32, (i / size) as i32)).collect();
    Some(crate::render::Qr { size, modules })
}

pub fn share_wifi_report(lang: Lang) -> (Report, Option<crate::render::Qr>) {
    let title = lang.tool_name(Tool::ShareWifi).to_string();
    let WifiState::Connected(w) = platform::tools::wifi() else {
        let report = wifi_report(lang);
        return (Report { title, text: report.text, line: report.line }, None);
    };
    let (security, _) = security(lang, w.auth, w.cipher, w.secured);
    let mut text = String::new();
    row(&mut text, "SSID", &w.ssid);
    row(&mut text, lang.tool_label("security"), &security);
    let wep = is_wep(w.cipher);
    let (sharing, qr) = match platform::tools::wifi_profile_xml(&w.profile).map(|xml| key_of(&xml)) {
        Some(Key::Plain(key)) => {
            row(&mut text, lang.tool_label("password"), &key);
            (Sharing::Shared, qr_of(&wifi_qr_text(&w.ssid, Some(&key), wep)))
        }
        Some(Key::Open) => {
            row(&mut text, lang.tool_label("password"), lang.tool_label("no_password"));
            (Sharing::Open, qr_of(&wifi_qr_text(&w.ssid, None, false)))
        }
        Some(Key::Protected) => (Sharing::NeedsAdmin, None),
        Some(Key::Enterprise) => (Sharing::Enterprise, None),
        None => (Sharing::Unknown, None),
    };
    let line = lang.share_line(&w.ssid, sharing);
    (Report { title, text, line }, qr)
}

// ---------------------------------------------------------------- calls home

pub fn calls_report(calls: &crate::pet::Calls, today: u32, lang: Lang) -> Report {
    // Who, how many calls in all, and to which names how often.
    type Tally<'a> = Vec<(String, u32, Vec<(&'a String, u32)>)>;
    let title = lang.tool_name(Tool::CallsHome).to_string();
    let mut tally: Tally = Vec::new();
    if calls.day == today {
        for (process, names) in &calls.by_process {
            let mut names: Vec<(&String, u32)> = names.iter().map(|(n, c)| (n, *c)).collect();
            names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            tally.push((lang.who(process), names.iter().map(|(_, c)| c).sum(), names));
        }
    }
    tally.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut text = format!("{}\n\n", lang.tool_label("calls_today"));
    if tally.is_empty() {
        text.push_str(&format!("{}\n", lang.tool_label("nobody")));
    }
    for (who, count, names) in &tally {
        row(&mut text, who, &format!("{count}×"));
        for (name, n) in names.iter().take(TOP) {
            text.push_str(&format!("  {name}  {n}×\n"));
        }
        if names.len() > TOP {
            text.push_str(&format!("  {} {}\n", lang.tool_label("and_more"), names.len() - TOP));
        }
    }
    let line = lang.calls_line(tally.first().map(|(who, n, _)| (who.as_str(), *n)), tally.len());
    Report { title, text, line }
}

// ---------------------------------------------------------------- neighbours

// Never more than the /24 it sits in.
fn subnet_hosts(addr: Ipv4Addr, prefix: u8) -> Vec<Ipv4Addr> {
    let prefix = u32::from(prefix.clamp(24, 30));
    let size = 1u32 << (32 - prefix);
    let base = u32::from(addr) & !(size - 1);
    (1..size - 1).map(|i| Ipv4Addr::from(base + i)).filter(|&ip| ip != addr).collect()
}

fn neighbours_report(lang: Lang) -> Report {
    let title = lang.tool_name(Tool::Neighbours).to_string();
    let adapters = platform::tools::adapters();
    let Some(adapter) = adapters.iter().find(|a| a.gateway4().is_some() && a.ipv4().is_some()) else {
        return Report { title, text: format!("{}\n", lang.tool_label("no_local_network")), line: lang.neighbours_line(None) };
    };
    let (me, prefix) = adapter.ipv4().unwrap();
    let gateway = adapter.gateway4();
    let hosts = subnet_hosts(me, prefix);

    // An ARP request to every address at once: an absent one takes seconds
    // to time out, so one small thread each keeps the whole knock short.
    let found: Vec<(Ipv4Addr, [u8; 6])> = std::thread::scope(|s| {
        let jobs: Vec<_> = hosts
            .iter()
            .map(|&ip| std::thread::Builder::new().stack_size(64 * 1024).spawn_scoped(s, move || platform::tools::arp(ip, me).map(|mac| (ip, mac))))
            .collect();
        jobs.into_iter().filter_map(|job| job.ok()?.join().ok().flatten()).collect()
    });
    let names = names_of(found.iter().map(|&(ip, _)| Some(ip)));
    let mut seen: Vec<(Ipv4Addr, [u8; 6], Option<String>)> =
        found.iter().zip(names).map(|(&(ip, mac), name)| (ip, mac, name)).collect();
    if let Some(mac) = adapter.mac {
        seen.push((me, mac, platform::host::computer_name()));
    }
    seen.sort_by_key(|(ip, _, _)| u32::from(*ip));

    let network = u32::from(me) & !((1u32 << (32 - u32::from(prefix.clamp(24, 30)))) - 1);
    let mut text = format!(
        "{} {}/{}, {} {}\n",
        lang.tool_label("segment"),
        Ipv4Addr::from(network),
        prefix.max(24),
        hosts.len(),
        lang.tool_label("knocked"),
    );
    if prefix < 24 {
        text.push_str(&format!("{}\n", lang.tool_label("bigger_than_24")));
    }
    text.push('\n');
    for (ip, mac, name) in &seen {
        let mut marks = Vec::new();
        if Some(*ip) == gateway {
            marks.push(lang.tool_label("gateway"));
        }
        if *ip == me {
            marks.push(lang.tool_label("this_computer"));
        }
        if is_random(*mac) {
            marks.push(lang.tool_label("random_mac"));
        }
        let name = name.as_deref().unwrap_or("");
        let marks = if marks.is_empty() { String::new() } else { format!("  ({})", marks.join(", ")) };
        text.push_str(&format!("{:<16}{}  {name}{marks}\n", ip.to_string(), mac_text(*mac)));
    }
    let others: Vec<_> = seen.iter().filter(|(ip, _, _)| *ip != me).collect();
    let random = others.iter().filter(|(_, mac, _)| is_random(*mac)).count();
    Report { title, text, line: lang.neighbours_line(Some((others.len(), random))) }
}

// ---------------------------------------------------------------- listening

fn port_name(lang: Lang, port: u16) -> &'static str {
    match port {
        21 => "ftp",
        22 => "ssh",
        23 => "telnet",
        25 => "smtp",
        53 => "dns",
        80 => "http",
        135 => "rpc",
        139 | 445 => "smb",
        443 => "https",
        1433 => "mssql",
        2375 | 2376 => "docker",
        3306 => "mysql",
        3389 => "rdp",
        5357 => "wsd",
        5432 => "postgres",
        5800 => "vnc http",
        5900..=5903 => "vnc",
        5985 | 5986 => "winrm",
        6379 => "redis",
        7680 => "delivery optimization",
        27017 => "mongodb",
        3000 | 5173 | 8000 | 8080 => "http dev",
        49152.. => lang.tool_label("dynamic"),
        _ => "",
    }
}

fn listening_report(lang: Lang) -> Report {
    let mut listeners = net::listeners_now();
    let exposed: HashSet<(String, u16)> =
        listeners.iter().filter(|l| l.exposed).map(|l| (l.process.clone(), l.port)).collect();
    listeners.retain(|l| l.exposed || !exposed.contains(&(l.process.clone(), l.port)));
    listeners.sort_by(|a, b| (!a.exposed, a.port, &a.process).cmp(&(!b.exposed, b.port, &b.process)));
    listeners.dedup_by(|a, b| a.port == b.port && a.process == b.process && a.exposed == b.exposed);

    let mut text = String::new();
    let open = listeners.iter().filter(|l| l.exposed).count();
    for (exposed, heading) in [
        (true, lang.tool_label("to_the_network")),
        (false, lang.tool_label("at_home")),
    ] {
        text.push_str(&format!("{heading}\n"));
        for l in listeners.iter().filter(|l| l.exposed == exposed) {
            text.push_str(&format!("  TCP {:<7}{:<24}{}\n", l.port, port_name(lang, l.port), lang.who(&l.process)));
        }
        text.push('\n');
    }
    Report { title: lang.tool_name(Tool::Listening).into(), text, line: lang.listening_line(listeners.len(), open) }
}

// ---------------------------------------------------------------- usage

struct Second {
    rx: u64,
    tx: u64,
    flows: Vec<(String, IpAddr, u64, u64)>,
    connections: Vec<String>,
}

#[derive(Default)]
pub struct Usage {
    seconds: VecDeque<Second>,
}

impl Usage {
    pub fn record(&mut self, tick: &Tick) {
        self.seconds.push_back(Second {
            rx: tick.rx,
            tx: tick.tx,
            flows: tick.flows.iter().map(|f| (f.conn.process.clone(), f.conn.remote, f.rx, f.tx)).collect(),
            connections: tick.active.iter().map(|c| c.process.clone()).collect(),
        });
        while self.seconds.len() > USAGE_SECS {
            self.seconds.pop_front();
        }
    }

    fn totals<K: Eq + std::hash::Hash>(&self, key: impl Fn(&(String, IpAddr, u64, u64)) -> K) -> Vec<(K, u64, u64)> {
        let mut by: HashMap<K, (u64, u64)> = HashMap::new();
        for flow in self.seconds.iter().flat_map(|s| &s.flows) {
            let e = by.entry(key(flow)).or_default();
            (e.0, e.1) = (e.0 + flow.2, e.1 + flow.3);
        }
        by.into_iter().map(|(k, (rx, tx))| (k, rx, tx)).collect()
    }

    fn processes(&self) -> Vec<(String, u64, u64)> {
        let mut out = self.totals(|f| f.0.clone());
        out.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)).then_with(|| a.0.cmp(&b.0)));
        out
    }

    fn destinations(&self) -> Vec<(IpAddr, u64, u64)> {
        let mut out = self.totals(|f| f.1);
        out.sort_by_key(|&(_, rx, tx)| std::cmp::Reverse(rx + tx));
        out
    }

    pub fn report(&self, names: &Names, sized: bool, lang: Lang) -> Report {
        let secs = self.seconds.len().max(1) as u64;
        let per_sec = |bytes: u64| talk::rate(bytes / secs);
        let (rx, tx) = self.seconds.iter().fold((0, 0), |(r, t), s| (r + s.rx, t + s.tx));
        let mut text = String::new();
        row(&mut text, lang.tool_label("total"), &format!("↓{} ↑{}", per_sec(rx), per_sec(tx)));
        text.push('\n');
        let line = if sized {
            let processes = self.processes();
            text.push_str(&format!("{}\n", lang.tool_label("programs")));
            for (process, rx, tx) in processes.iter().take(TOP) {
                text.push_str(&format!("  {:<28}↓{:<12}↑{}\n", lang.who(process), per_sec(*rx), per_sec(*tx)));
            }
            text.push_str(&format!("\n{}\n", lang.tool_label("where_it_goes")));
            for (ip, rx, tx) in self.destinations().iter().take(TOP) {
                let dest = match names.get(*ip) {
                    _ if net::is_own(*ip) => lang.this_pc().to_string(),
                    Some(name) if services::by_domain(&name).is_some_and(services::Service::discreet) => lang.hidden().to_string(),
                    Some(name) => net::short_domain(&name),
                    None => ip.to_string(),
                };
                text.push_str(&format!("  {dest:<28}↓{:<12}↑{}\n", per_sec(*rx), per_sec(*tx)));
            }
            match processes.first() {
                Some((process, rx, tx)) if rx + tx >= secs * 1024 => {
                    lang.usage_line(Some((process.as_str(), per_sec(rx + tx).as_str())))
                }
                _ => lang.usage_line(None),
            }
        } else {
            text.push_str(&format!("{}\n\n", lang.tool_label(if cfg!(windows) { "no_sizes" } else { "no_sizes_here" })));
            let mut counts: HashMap<&str, usize> = HashMap::new();
            for process in self.seconds.back().map(|s| s.connections.as_slice()).unwrap_or_default() {
                *counts.entry(process.as_str()).or_default() += 1;
            }
            let mut counts: Vec<_> = counts.into_iter().collect();
            counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            for (process, n) in counts.iter().take(TOP) {
                text.push_str(&format!("  {:<28}{n}\n", lang.who(process)));
            }
            match counts.first() {
                Some((process, n)) => {
                    lang.usage_line(Some((*process, format!("{n} {}", lang.tool_label("connections")).as_str())))
                }
                None => lang.usage_line(None),
            }
        };
        Report { title: lang.tool_name(Tool::Usage).into(), text, line }
    }
}

// ---------------------------------------------------------------- ping


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Echo {
    Reply,
    // A hop on the way answered that the time to live ran out.
    Expired,
    Failed,
}

fn trace(icmp: &platform::tools::Pinger, target: Ipv4Addr) -> Vec<(u8, Option<(Ipv4Addr, u32)>)> {
    let mut hops = Vec::new();
    let mut silent = 0;
    for ttl in 1..=MAX_HOPS {
        match icmp.echo(target, ttl, PING_TIMEOUT_MS) {
            Some((Echo::Reply, from, rtt)) => {
                hops.push((ttl, Some((from, rtt))));
                break;
            }
            Some((Echo::Expired, from, rtt)) => {
                silent = 0;
                hops.push((ttl, Some((from, rtt))));
            }
            _ => {
                silent += 1;
                hops.push((ttl, None));
                if silent >= SILENT_HOPS {
                    break;
                }
            }
        }
    }
    hops
}

fn verdict(network: bool, gateway: Option<&[Option<u32>]>, internet: &[Option<u32>], names: bool) -> Verdict {
    let fastest = |series: &[Option<u32>]| series.iter().flatten().min().copied();
    if !network {
        return Verdict::NoNetwork;
    }
    match (gateway.map(fastest), fastest(internet)) {
        (_, Some(_)) if !names => Verdict::NoDns,
        (gateway, Some(internet)) => Verdict::Fine { gateway: gateway.flatten(), internet },
        (Some(None), None) => Verdict::NoGateway,
        (_, None) => Verdict::NoInternet,
    }
}

fn ping_report(lang: Lang) -> Report {
    let title = lang.tool_name(Tool::Ping).to_string();
    let adapters = platform::tools::adapters();
    let primary = adapters.iter().find(|a| a.gateway4().is_some());
    let Some(icmp) = platform::tools::Pinger::open() else {
        return Report { title, text: "ICMP\n".into(), line: lang.ping_line(&Verdict::NoNetwork) };
    };
    let series = |ip: Ipv4Addr| -> Vec<Option<u32>> {
        (0..PINGS).map(|_| icmp.echo(ip, 64, PING_TIMEOUT_MS).filter(|r| r.0 == Echo::Reply).map(|r| r.2)).collect()
    };
    let gateway = primary.and_then(Adapter::gateway4);
    let dns = primary.and_then(Adapter::dns4);
    let gateway_pings = gateway.map(&series);
    let dns_pings = dns.filter(|&ip| Some(ip) != gateway).map(&series);
    let internet_pings = series(INTERNET);
    let started = Instant::now();
    let names = (NAME_CHECK, 443).to_socket_addrs().is_ok_and(|mut a| a.next().is_some());
    let name_ms = started.elapsed().as_millis();
    let hops = trace(&icmp, INTERNET);
    let hop_names = names_of(hops.iter().map(|(_, hop)| hop.map(|(ip, _)| ip)));

    let summary = |series: &[Option<u32>]| {
        let answered = series.iter().flatten().count();
        match series.iter().flatten().min() {
            Some(min) => format!("{answered}/{}  min {min} ms", series.len()),
            None => format!("0/{}  {}", series.len(), lang.tool_label("silent")),
        }
    };
    let mut text = String::new();
    if let (Some(ip), Some(pings)) = (gateway, &gateway_pings) {
        text.push_str(&format!("{} {ip}  {}\n", lang.tool_label("gateway"), summary(pings)));
    }
    if let (Some(ip), Some(pings)) = (dns, &dns_pings) {
        text.push_str(&format!("DNS {ip}  {}\n", summary(pings)));
    }
    text.push_str(&format!("internet {INTERNET}  {}\n", summary(&internet_pings)));
    let name_result = if names { format!("ok, {name_ms} ms") } else { lang.tool_label("failing").to_string() };
    text.push_str(&format!("{} ({NAME_CHECK})  {name_result}\n", lang.tool_label("name_lookup")));
    text.push_str(&format!("\n{} {INTERNET}\n", lang.tool_label("route_to")));
    for ((ttl, hop), name) in hops.iter().zip(&hop_names) {
        match hop {
            Some((ip, rtt)) => {
                let name = name.as_deref().unwrap_or("");
                text.push_str(&format!("  {ttl:>2}  {:<16}{:>5} ms  {name}\n", ip.to_string(), rtt));
            }
            None => text.push_str(&format!("  {ttl:>2}  *\n")),
        }
    }
    let verdict = verdict(primary.is_some(), gateway_pings.as_deref(), &internet_pings, names);
    Report { title, text, line: lang.ping_line(&verdict) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_segment_is_knocked_on_up_to_a_slash_24() {
        let me = Ipv4Addr::new(192, 168, 1, 23);
        let hosts = subnet_hosts(me, 24);
        assert_eq!(hosts.len(), 253);
        assert!(!hosts.contains(&me) && hosts.contains(&Ipv4Addr::new(192, 168, 1, 1)));
        assert!(!hosts.contains(&Ipv4Addr::new(192, 168, 1, 0)) && !hosts.contains(&Ipv4Addr::new(192, 168, 1, 255)));
        assert_eq!(subnet_hosts(Ipv4Addr::new(10, 0, 7, 9), 16).len(), 253, "a big network is knocked on only around us");
        assert_eq!(subnet_hosts(Ipv4Addr::new(10, 0, 0, 5), 30), vec![Ipv4Addr::new(10, 0, 0, 6)]);
    }

    #[test]
    fn phones_hide_behind_random_macs() {
        assert!(is_random([0xda, 0xa1, 0x19, 0x00, 0x00, 0x01]));
        assert!(!is_random([0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]));
        assert_eq!(mac_text([0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]), "00:1A:2B:3C:4D:5E");
    }

    #[test]
    fn weak_wifi_security_is_called_out() {
        assert!(security(Lang::En, 1, 0, false).1, "open");
        assert!(security(Lang::En, 1, 257, true).1, "WEP");
        assert!(security(Lang::En, 4, 2, true).1, "WPA");
        assert!(security(Lang::En, 7, 2, true).1, "WPA2 with TKIP");
        assert_eq!(security(Lang::En, 7, 4, true), ("WPA2".to_string(), false));
        assert_eq!(security(Lang::En, 6, 4, true), ("WPA2 Enterprise".to_string(), false));
        assert_eq!(security(Lang::En, 9, 4, true), ("WPA3".to_string(), false));
    }

    #[test]
    fn bands_follow_the_frequency() {
        assert_eq!(band(2437), "2.4 GHz");
        assert_eq!(band(5180), "5 GHz");
        assert_eq!(band(5955), "6 GHz");
    }

    #[test]
    fn the_verdict_says_where_it_breaks() {
        let ok: &[Option<u32>] = &[Some(3), Some(2), None];
        let silent: &[Option<u32>] = &[None, None, None];
        assert_eq!(verdict(false, None, silent, false), Verdict::NoNetwork);
        assert_eq!(verdict(true, Some(silent), silent, false), Verdict::NoGateway);
        assert_eq!(verdict(true, Some(ok), silent, false), Verdict::NoInternet);
        assert_eq!(verdict(true, Some(ok), ok, false), Verdict::NoDns);
        assert_eq!(verdict(true, Some(ok), ok, true), Verdict::Fine { gateway: Some(2), internet: 2 });
        assert_eq!(verdict(true, Some(silent), ok, true), Verdict::Fine { gateway: None, internet: 2 }, "a router may ignore pings");
    }

    #[test]
    fn usage_ranks_the_hungriest() {
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        let mut usage = Usage::default();
        for _ in 0..3 {
            usage.seconds.push_back(Second {
                rx: 0,
                tx: 0,
                flows: vec![("msedge".into(), ip, 100, 20), ("steam".into(), ip, 9000, 10)],
                connections: vec![],
            });
        }
        let processes = usage.processes();
        assert_eq!(processes[0], ("steam".to_string(), 27_000, 30));
        assert_eq!(processes[1].0, "msedge");
    }

    #[test]
    #[ignore]
    fn run_every_tool_live() {
        for tool in Tool::ALL.into_iter().filter(|&t| t != Tool::Usage) {
            let started = Instant::now();
            let report = run(tool, Lang::En);
            let mask = |s: &str| s.chars().take(18).map(|c| if c.is_ascii_digit() { '#' } else { c }).collect::<String>();
            let labels: Vec<String> = report.text.lines().map(mask).collect();
            println!("{tool:?} in {:.1?}: {} lines, said {} chars", started.elapsed(), labels.len(), report.line.len());
            for label in labels.iter().take(14) {
                println!("    |{label}");
            }
        }
    }

    #[test]
    fn no_em_dashes_or_cyrillic_in_tool_copy() {
        let src = include_str!("mod.rs");
        let body = &src[..src.find("#[cfg(test)]").unwrap()];
        assert!(!body.contains('\u{2014}'));
        assert!(!body.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)));
    }
}

#[cfg(test)]
mod sharing_tests {
    use super::*;

    #[test]
    fn a_wifi_code_escapes_what_readers_split_on() {
        assert_eq!(wifi_qr_text("Kafe;Bar", Some(r"a\b:c"), false), r"WIFI:T:WPA;S:Kafe\;Bar;P:a\\b\:c;;");
        assert_eq!(wifi_qr_text("volno", None, false), "WIFI:T:nopass;S:volno;;");
        assert!(qr_of("WIFI:T:WPA;S:home;P:secret;;").is_some_and(|qr| qr.modules.len() == qr.size * qr.size));
    }

    #[test]
    fn a_profile_key_is_read_only_in_the_clear() {
        let plain = "<WLANProfile><MSM><security><authEncryption><authentication>WPA2PSK</authentication></authEncryption><sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>tom &amp; jerry</keyMaterial></sharedKey></security></MSM></WLANProfile>";
        assert_eq!(key_of(plain), Key::Plain("tom & jerry".into()));
        assert_eq!(key_of(&plain.replace("<protected>false", "<protected>true")), Key::Protected);
        assert_eq!(key_of("<authentication>open</authentication>"), Key::Open);
        assert_eq!(key_of("<authentication>WPA2</authentication>"), Key::Enterprise);
    }
}
