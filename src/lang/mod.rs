pub mod book;

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::tools::inspect::{Broken, Dropped, Origin, Signature, ZONE_INTERNET};
use crate::net::{Kind, Place};
use crate::net::services::Service;
use crate::render::Sheet;
use crate::render::sprite::Idle;
use crate::tools::{Sharing, Tool, Verdict};
use book::Book;
use crate::watch::{Cleartext, Exposure, Finding, Oddity};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    Cs,
    En,
}

const MISSING: &str = "(missing line)";

fn pick<T>(set: &[T], salt: u64) -> &T {
    &set[(salt.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 33) as usize % set.len()]
}

pub fn polished(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 1);
    let mut starting = true;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (starting, c.is_alphabetic()) {
            (true, true) => out.extend(c.to_uppercase()),
            _ => out.push(c),
        }
        if starting && !c.is_whitespace() {
            starting = false;
        }
        if (matches!(c, '.' | '!' | '?') && chars.peek() == Some(&' ')) || c == '\n' {
            starting = true;
        }
    }
    if out.trim_end().chars().last().is_some_and(|c| !ENDS_A_LINE.contains(c)) {
        out.push('.');
    }
    out
}

// The last one is the arm of a shrug, which ends a line as a full stop does.
const ENDS_A_LINE: &str = ".!?…:)\"'¯";

fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in vars {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

pub struct Details {
    pub satiety: f32,
    pub mood: f32,
    pub neglect: f32,
    pub stage: u8,
    pub days_to_grow: Option<u32>,
    pub age_days: u64,
    pub today: (String, String),
    pub total: (String, String),
    pub regulars: usize,
    pub known: usize,
    pub open: Vec<(Exposure, u16)>,
    pub muted_until: Option<String>,
}

fn duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

impl Lang {
    pub fn detect() -> Lang {
        let name = crate::platform::host::locale_name();
        if name.starts_with("cs") || name.starts_with("sk") { Lang::Cs } else { Lang::En }
    }

    pub fn other(self) -> Lang {
        match self {
            Lang::Cs => Lang::En,
            Lang::En => Lang::Cs,
        }
    }

    fn book(self) -> &'static Book {
        match self {
            Lang::Cs => book::cs(),
            Lang::En => book::en(),
        }
    }

    fn variants(self, key: &str) -> &'static [String] {
        match self.book().get(key) {
            Some(lines) if !lines.is_empty() => lines,
            _ => {
                debug_assert!(false, "{self:?} voice file has no line {key}");
                &[]
            }
        }
    }

    fn line(self, key: &str, salt: u64) -> &'static str {
        let set = self.variants(key);
        if set.is_empty() { MISSING } else { pick(set, salt).as_str() }
    }

    fn text(self, key: &str) -> &'static str {
        self.line(key, 0)
    }

    fn say(self, key: &str, salt: u64, vars: &[(&str, &str)]) -> String {
        fill(self.line(key, salt), vars)
    }

    fn count(self, key: &str, n: u64) -> String {
        let form = match (self, n) {
            (_, 1) => "one",
            (Lang::Cs, 2..=4) => "few",
            _ => "many",
        };
        fill(self.text(&format!("{key}.{form}")), &[("n", &n.to_string())])
    }

    fn varied(self, key: &str, salt: u64, process: &str, vars: &[(&str, &str)]) -> String {
        let set = self.variants(key);
        if set.is_empty() {
            return MISSING.into();
        }
        let start = (salt.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 33) as usize % set.len();
        let lines: Vec<String> = (0..set.len()).map(|i| fill(&set[(start + i) % set.len()], vars)).collect();
        let needle = process.to_lowercase();
        let repeats = |line: &String| needle.len() >= 3 && line.to_lowercase().matches(needle.as_str()).count() >= 2;
        lines.iter().find(|line| !repeats(line)).unwrap_or(&lines[0]).clone()
    }

    pub fn greeting(self, salt: u64, hour: u8) -> String {
        let key = match hour {
            5..=10 => "greeting.morning",
            18..=21 => "greeting.evening",
            22..=23 | 0..=4 => "greeting.night",
            _ => "greeting.day",
        };
        self.line(key, salt).into()
    }

    pub fn intro(self) -> &'static str {
        self.text("words.intro")
    }

    pub fn hidden(self) -> &'static str {
        self.text("words.hidden")
    }

    pub fn this_pc(self) -> &'static str {
        self.text("words.this_pc")
    }

    pub fn who(self, process: &str) -> String {
        match process {
            "" => self.text("words.system_process").into(),
            p => p.into(),
        }
    }

    pub fn kind(self, kind: Kind) -> String {
        let key = match kind {
            Kind::Web => "web",
            Kind::PlainWeb => "plain_web",
            Kind::Ssh => "ssh",
            Kind::RemoteDesktop => "remote_desktop",
            Kind::WinRm => "winrm",
            Kind::Dns => "dns",
            Kind::Mail => "mail",
            Kind::FileShare => "file_share",
            Kind::Database => "database",
            Kind::Git => "git",
            Kind::Chat => "chat",
            Kind::Call => "call",
            Kind::Game => "game",
            Kind::Other(p) => return fill(self.text("kind.other"), &[("port", &p.to_string())]),
        };
        self.text(&format!("kind.{key}")).into()
    }

    fn taste(self, kind: Kind, place: Place) -> &'static str {
        let key = match kind {
            Kind::Ssh => "ssh",
            Kind::Git => "git",
            Kind::PlainWeb if place == Place::Internet => "plain_web",
            Kind::RemoteDesktop | Kind::WinRm => "remote",
            Kind::Database => "database",
            Kind::Other(_) => "other",
            _ => return "",
        };
        self.text(&format!("taste.{key}"))
    }

    pub fn dest(self, name: &str, place: Place) -> String {
        match place {
            Place::Internet => name.into(),
            Place::Lan => fill(self.text("words.on_lan"), &[("name", name)]),
        }
    }

    pub fn opened(self, process: &str, kind: Kind, dest: &str, place: Place, salt: u64) -> String {
        let home = process.len() >= 3 && dest.to_lowercase().contains(&process.to_lowercase());
        let key = if home { "connection.calls_home" } else { "connection.opened" };
        self.say(key, salt, &[("who", &self.who(process)), ("what", &self.kind(kind)), ("dest", dest), ("taste", self.taste(kind, place))])
    }

    pub fn private(self, process: &str, salt: u64) -> String {
        self.say("connection.private", salt, &[("who", &self.who(process))])
    }

    pub fn service(self, service: Service, process: &str, dest: &str, salt: u64) -> String {
        if service == Service::Private {
            return self.private(process, salt);
        }
        self.varied(&format!("service.{service:?}"), salt, process, &[("who", &self.who(process)), ("dest", dest)])
    }

    pub fn spike(self, rate: &str, incoming: bool, salt: u64) -> String {
        let key = if incoming { "traffic.spike_in" } else { "traffic.spike_out" };
        self.say(key, salt, &[("rate", rate)])
    }

    pub fn right_after(self, process: &str, dest: &str) -> String {
        fill(self.text("traffic.right_after"), &[("who", &self.who(process)), ("dest", dest)])
    }

    pub fn busiest(self, process: &str, n: usize) -> String {
        fill(self.text("traffic.busiest"), &[("who", &self.who(process)), ("n", &n.to_string())])
    }

    pub fn flow(self, rate: &str, incoming: bool, process: &str, dest: &str, salt: u64) -> String {
        let key = if incoming { "traffic.flow_in" } else { "traffic.flow_out" };
        self.say(key, salt, &[("rate", rate), ("who", &self.who(process)), ("dest", dest)])
    }

    pub fn feast(self, rate: &str, process: Option<&str>, salt: u64) -> String {
        match process {
            Some(process) => self.say("traffic.feast", salt, &[("rate", rate), ("who", &self.who(process))]),
            None => self.say("traffic.feast_plain", salt, &[("rate", rate)]),
        }
    }

    pub fn stuffed(self) -> &'static str {
        self.text("traffic.stuffed")
    }

    pub fn woke(self, salt: u64) -> String {
        self.line("mood.woke", salt).into()
    }

    pub fn hobby(self, idle: Idle, salt: u64) -> String {
        let key = match idle {
            Idle::Tinker => "hobby.tinker",
            Idle::Read => "hobby.read",
            Idle::Game => "hobby.game",
            _ => "hobby.snack",
        };
        self.line(key, salt).into()
    }

    pub fn nap(self, salt: u64) -> String {
        self.line("mood.nap", salt).into()
    }

    pub fn dozed(self, salt: u64) -> String {
        self.line("mood.dozed", salt).into()
    }

    pub fn starving(self, salt: u64) -> String {
        self.line("mood.starving", salt).into()
    }

    pub fn hungry(self, salt: u64) -> String {
        self.line("mood.hungry", salt).into()
    }

    pub fn bored(self, salt: u64) -> String {
        self.line("mood.bored", salt).into()
    }

    pub fn lore(self, salt: u64, hour: u8) -> String {
        let key = if (5..22).contains(&hour) { "lore.day" } else { "lore.night" };
        self.line(key, salt).into()
    }

    pub fn summary(self, open: usize, top: Option<(&str, usize)>, lan: usize) -> String {
        let mut line = fill(self.text("summary.open"), &[("open", &open.to_string())]);
        if let Some((process, count)) = top {
            line += &fill(self.text("summary.top"), &[("count", &count.to_string()), ("who", &self.who(process))]);
        }
        if lan > 0 {
            line += &fill(self.text("summary.lan"), &[("lan", &lan.to_string())]);
        }
        line
    }

    fn exposure_thing(self, what: Exposure, port: u16) -> String {
        fill(self.text(&format!("thing.{what:?}")), &[("port", &port.to_string())])
    }

    pub fn finding(self, finding: &Finding, dest: &dyn Fn(IpAddr) -> String, salt: u64) -> String {
        match finding {
            Finding::Exposed { process, port, what, again } => {
                let thing = self.exposure_thing(*what, *port);
                let key = if *again { "finding.exposed.again".to_string() } else { format!("finding.exposed.{what:?}") };
                self.say(&key, salt, &[("who", &self.who(process)), ("port", &port.to_string()), ("thing", &thing)])
            }
            Finding::Unexposed { port, what, .. } => self.say("finding.unexposed", salt, &[("thing", &self.exposure_thing(*what, *port))]),
            Finding::Cleartext { process, remote, what } => {
                let proto = match what {
                    Cleartext::Http => "http",
                    Cleartext::Ftp => "ftp",
                    Cleartext::Pop3 => "pop3",
                    Cleartext::Imap => "imap",
                    Cleartext::Ldap => "ldap",
                    Cleartext::Mqtt => "mqtt",
                };
                self.say("finding.cleartext", salt, &[("who", &self.who(process)), ("proto", proto), ("dest", &dest(*remote))])
            }
            Finding::Lolbin { process, remote } => {
                let shell = finding.severity() == crate::watch::Severity::Note;
                let key = if shell { "finding.shell" } else { "finding.lolbin" };
                self.say(key, salt, &[("who", &self.who(process)), ("dest", &dest(*remote))])
            }
            Finding::FromTemp { process, remote } => self.say("finding.from_temp", salt, &[("who", &self.who(process)), ("dest", &dest(*remote))]),
            Finding::Beacon { process, remote, every } => {
                self.say("finding.beacon", salt, &[("who", &self.who(process)), ("dest", &dest(*remote)), ("every", &every.to_string())])
            }
            Finding::Sweep { process, hosts, ports } => {
                let key = if *hosts >= *ports { "finding.sweep_hosts" } else { "finding.sweep_ports" };
                self.say(key, salt, &[("who", &self.who(process)), ("hosts", &hosts.to_string()), ("ports", &ports.to_string())])
            }
            Finding::Upload { rate, secs, owner, away } => {
                let owner = match owner {
                    Some((process, remote)) => fill(self.text("finding.upload_owner"), &[("who", &self.who(process)), ("dest", &dest(*remote))]),
                    None => String::new(),
                };
                let key = if *away { "finding.upload_away" } else { "finding.upload" };
                self.say(key, salt, &[("rate", &crate::talk::rate(*rate)), ("secs", &secs.to_string()), ("owner", &owner)])
            }
            Finding::Newcomer { process, remote } => self.say("finding.newcomer", salt, &[("who", &self.who(process)), ("dest", &dest(*remote))]),
            Finding::Tor { process } => self.say("finding.tor", salt, &[("who", &self.who(process))]),
            Finding::Mining { process, remote } => self.say("finding.mining", salt, &[("who", &self.who(process)), ("dest", &dest(*remote))]),
            Finding::OddName { process, name, why } => {
                let key = match why {
                    Oddity::Punycode => "finding.punycode",
                    Oddity::CheapTld => "finding.cheap_tld",
                    Oddity::RandomLabel => "finding.random_label",
                };
                self.say(key, salt, &[("who", &self.who(process)), ("name", name)])
            }
            Finding::Session { process, remote, what, incoming, lasted, .. } => {
                let vars = [
                    ("who", self.who(process)),
                    ("dest", dest(*remote)),
                    ("what", self.text(&format!("session.what.{what:?}")).to_string()),
                    ("over", self.text(&format!("session.over.{what:?}")).to_string()),
                    ("dur", lasted.map(duration).unwrap_or_default()),
                ];
                let vars: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
                let key = match (*incoming, lasted.is_some()) {
                    (true, false) => "session.in_open".to_string(),
                    (true, true) => "session.in_closed".to_string(),
                    (false, true) => "session.out_closed".to_string(),
                    (false, false) => format!("session.out.{what:?}"),
                };
                self.say(&key, salt, &vars)
            }
            Finding::DnsBypass { process, remote } => self.say("finding.dns_bypass", salt, &[("who", &self.who(process)), ("dest", &dest(*remote))]),
            Finding::NewDevice { ip, mac, name, random } => {
                let ip = ip.to_string();
                let who = match name {
                    Some(name) => fill(self.text("words.named"), &[("name", name), ("ip", &ip)]),
                    None => fill(self.text("words.unnamed"), &[("ip", &ip), ("mac", mac)]),
                };
                let random = if *random { self.text("finding.random_mac") } else { "" };
                self.say("finding.new_device", salt, &[("who", &who), ("random", random)])
            }
            Finding::GatewaySpoof { gateway, mac, posing, name } => {
                let ip = posing.to_string();
                let posing = match name {
                    Some(name) => fill(self.text("words.named"), &[("name", name), ("ip", &ip)]),
                    None => ip,
                };
                self.say("finding.gateway_spoof", salt, &[("posing", &posing), ("mac", mac), ("gateway", &gateway.to_string())])
            }
            Finding::GatewayChanged { gateway, mac } => self.say("finding.gateway_changed", salt, &[("gateway", &gateway.to_string()), ("mac", mac)]),
            Finding::GatewayFlapped { gateway, mac } => self.say("finding.gateway_flapped", salt, &[("gateway", &gateway.to_string()), ("mac", mac)]),
            Finding::Offline { local: true, .. } => self.say("finding.offline_local", salt, &[]),
            Finding::Offline { local: false, .. } => self.say("finding.offline_none", salt, &[]),
            Finding::Online { secs, .. } => self.say("finding.online", salt, &[("dur", &duration(*secs))]),
            Finding::NewAutorun { place, name } => {
                use crate::watch::autoruns::Place;
                let key = match place {
                    Place::Run => "finding.autorun_run",
                    Place::Startup => "finding.autorun_startup",
                    Place::Service => "finding.autorun_service",
                    Place::Task => "finding.autorun_task",
                };
                self.say(key, salt, &[("name", name)])
            }
            Finding::HostsAdded { name, ip, more: 0 } => self.say("finding.hosts_one", salt, &[("name", name), ("ip", ip)]),
            Finding::HostsAdded { name, ip, more } => {
                self.say("finding.hosts_more", salt, &[("name", name), ("ip", ip), ("n", &(more + 1).to_string())])
            }
            Finding::ProxySet { via } => self.say("finding.proxy_set", salt, &[("host", via)]),
            Finding::DnsChanged { adapter, servers } => self.say("finding.dns_changed", salt, &[("name", adapter), ("ip", servers)]),
        }
    }

    pub fn stage_name(self, stage: u8) -> &'static str {
        self.text(&format!("sheet.stage.{}", stage.min(3)))
    }

    pub fn sheet(self, d: &Details) -> Sheet {
        let growth = match d.days_to_grow {
            None => self.text("sheet.grown").to_string(),
            Some(k) => self.count("sheet.to_grow", k.into()),
        };
        let age = match d.age_days {
            0 => self.text("sheet.born_today").to_string(),
            a => self.count("sheet.age", a),
        };
        let condition = match d.neglect {
            n if n >= 0.8 => self.text("sheet.condition.torn_dirty"),
            n if n >= 0.5 => self.text("sheet.condition.torn"),
            _ => self.text("sheet.condition.fine"),
        };
        let ((tin, tout), (ain, aout)) = (&d.today, &d.total);
        let about = vec![fill(self.text("sheet.stage_of"), &[("stage", &(d.stage + 1).to_string())]), growth, age];
        let meters = vec![(self.text("sheet.meter.full").to_string(), d.satiety), (self.text("sheet.meter.mood").to_string(), d.mood)];
        let mut rows = vec![
            fill(self.text("sheet.row.condition"), &[("condition", condition)]),
            String::new(),
            fill(self.text("sheet.row.today"), &[("in", tin), ("out", tout)]),
            fill(self.text("sheet.row.total"), &[("in", ain), ("out", aout)]),
            fill(self.text("sheet.row.counts"), &[("regulars", &d.regulars.to_string()), ("known", &d.known.to_string())]),
        ];
        if let Some(until) = &d.muted_until {
            rows.push(fill(self.text("sheet.muted"), &[("until", until)]));
        }
        let mut warnings = Vec::new();
        if !d.open.is_empty() {
            let things: Vec<String> = d.open.iter().map(|&(what, port)| self.exposure_thing(what, port)).collect();
            warnings.push(fill(self.text("sheet.open"), &[("things", &things.join(", "))]));
        }
        Sheet { name: self.text("sheet.name").into(), stage: self.stage_name(d.stage).into(), about, meters, rows, warnings }
    }

    pub fn regular(self, process: &str, host: &str, days: u32, salt: u64) -> String {
        self.say("connection.regular", salt, &[("who", &self.who(process)), ("dest", host), ("days", &days.to_string())])
    }

    pub fn night_owl(self, hour: u8, salt: u64) -> String {
        self.say("mood.night_owl", salt, &[("hour", &hour.to_string())])
    }

    pub fn petted(self, strokes: usize, salt: u64) -> String {
        let key = if strokes <= 3 { "mood.petted" } else { "mood.petted_too_much" };
        self.line(key, salt).into()
    }

    pub fn level_up(self, stage: u8) -> &'static str {
        self.text(&format!("level_up.{}", stage.clamp(1, 3)))
    }

    pub fn patrol(self, salt: u64) -> &'static str {
        self.line("roam.patrol", salt)
    }

    pub fn sits(self, salt: u64) -> &'static str {
        self.line("roam.sits", salt)
    }

    pub fn sits_on(self, process: &str, salt: u64) -> String {
        match crate::detect::seat_kind(process) {
            Some(kind) => self.say(&format!("sits_on.{kind:?}"), salt, &[("app", process)]),
            None => self.sits(salt).to_string(),
        }
    }

    pub fn back(self, salt: u64) -> &'static str {
        self.line("roam.back", salt)
    }

    pub fn fell(self, salt: u64) -> &'static str {
        self.line("roam.fell", salt)
    }

    pub fn covered(self, salt: u64) -> &'static str {
        self.line("roam.covered", salt)
    }

    pub fn escaping(self, app: &str, salt: u64) -> String {
        let app = if app.is_empty() { self.text("words.something") } else { app };
        self.say("roam.escaping", salt, &[("app", app)])
    }

    pub fn travel(self, salt: u64) -> &'static str {
        self.line("roam.travel", salt)
    }

    pub fn arrived(self, salt: u64) -> &'static str {
        self.line("roam.arrived", salt)
    }

    pub fn hiding(self, app: &str, salt: u64) -> String {
        let app = if app.is_empty() { self.text("words.something") } else { app };
        self.say("roam.hiding", salt, &[("app", app)])
    }

    pub fn menu_tools(self) -> &'static str {
        self.text("menu.tools")
    }

    pub fn tool_name(self, tool: Tool) -> &'static str {
        self.text(&format!("tool.name.{tool:?}"))
    }

    pub fn tool_label(self, key: &str) -> &'static str {
        self.text(&format!("tool.label.{key}"))
    }

    pub fn calls_line(self, top: Option<(&str, u32)>, programs: usize) -> String {
        match top {
            None => self.text("calls.none").into(),
            Some((who, n)) => {
                let programs = self.count("calls.programs", programs as u64);
                fill(self.text("calls.top"), &[("who", who), ("n", &n.to_string()), ("programs", &programs)])
            }
        }
    }

    pub fn tool_busy(self) -> &'static str {
        self.text("tool.busy")
    }

    pub fn tool_still_busy(self) -> &'static str {
        self.text("tool.still_busy")
    }

    pub fn network_line(self, public: Option<&str>, salt: u64) -> String {
        match public {
            Some(ip) => self.say("network.public", salt, &[("ip", ip)]),
            None => self.text("network.none").into(),
        }
    }

    pub fn wifi_line(self, ssid: &str, signal: u32, security: &str, weak: bool) -> String {
        let key = if weak { "wifi.weak" } else { "wifi.fine" };
        fill(self.text(key), &[("ssid", ssid), ("signal", &signal.to_string()), ("security", security)])
    }

    pub fn share_line(self, ssid: &str, sharing: Sharing) -> String {
        fill(self.text(&format!("share.{sharing:?}")), &[("ssid", ssid)])
    }

    pub fn wifi_denied(self) -> &'static str {
        self.text("wifi.denied")
    }

    pub fn wifi_off(self) -> &'static str {
        self.text("wifi.off")
    }

    pub fn wifi_none(self) -> &'static str {
        self.text("wifi.none")
    }

    pub fn neighbours_line(self, found: Option<(usize, usize)>) -> String {
        match found {
            None => self.text("neighbours.none").into(),
            Some((0, _)) => self.text("neighbours.alone").into(),
            Some((n, random)) => {
                let machines = self.count("neighbours.machines", n as u64);
                match random {
                    0 => fill(self.text("neighbours.list"), &[("machines", &machines)]),
                    r => fill(self.text("neighbours.random"), &[("machines", &machines), ("random", &r.to_string())]),
                }
            }
        }
    }

    pub fn listening_line(self, total: usize, exposed: usize) -> String {
        let key = if exposed == 0 { "listening.local" } else { "listening.exposed" };
        fill(self.text(key), &[("total", &total.to_string()), ("exposed", &exposed.to_string())])
    }

    pub fn usage_line(self, top: Option<(&str, &str)>) -> String {
        match top {
            Some((who, rate)) => fill(self.text("usage.top"), &[("who", &self.who(who)), ("rate", rate)]),
            None => self.text("usage.none").into(),
        }
    }

    pub fn ping_line(self, verdict: &Verdict) -> String {
        match verdict {
            Verdict::NoNetwork => self.text("ping.no_network").into(),
            Verdict::NoGateway => self.text("ping.no_gateway").into(),
            Verdict::NoInternet => self.text("ping.no_internet").into(),
            Verdict::NoDns => self.text("ping.no_dns").into(),
            Verdict::Fine { gateway: Some(g), internet } => fill(self.text("ping.fine"), &[("gateway", &g.to_string()), ("internet", &internet.to_string())]),
            Verdict::Fine { gateway: None, internet } => fill(self.text("ping.fine_no_gateway"), &[("internet", &internet.to_string())]),
        }
    }

    pub fn card_hint(self) -> &'static str {
        self.text("tool.card_hint")
    }

    pub fn panel_hint(self) -> &'static str {
        self.text("tool.panel_hint")
    }

    pub fn inspect_title(self) -> &'static str {
        self.text("inspect.title")
    }

    pub fn inspect_busy(self) -> &'static str {
        self.text("inspect.busy")
    }

    pub fn inspect_label(self, i: usize) -> &'static str {
        let key = ["file", "size", "signature", "origin"][i];
        self.text(&format!("inspect.label.{key}"))
    }

    pub fn inspect_more(self, more: usize) -> String {
        fill(self.text("inspect.more"), &[("more", &more.to_string())])
    }

    fn broken_why(self, why: Broken) -> &'static str {
        self.text(&format!("inspect.broken.{why:?}"))
    }

    pub fn signature_text(self, s: &Signature) -> String {
        match s {
            Signature::Valid { signer, catalog } => {
                // What vouches for a file: its own signature, Windows'
                // catalog of them, or, where programs are not signed at
                // all, the package that put the file there.
                let key = match (*catalog, cfg!(windows)) {
                    (true, true) => "inspect.signature.valid_catalog",
                    (true, false) => "inspect.signature.valid_package",
                    (false, _) => "inspect.signature.valid",
                };
                fill(self.text(key), &[("signer", signer)])
            }
            Signature::Broken { signer: Some(signer), why } => fill(self.text("inspect.signature.broken_by"), &[("why", self.broken_why(*why)), ("signer", signer)]),
            Signature::Broken { signer: None, why } => fill(self.text("inspect.signature.broken"), &[("why", self.broken_why(*why))]),
            Signature::Unsigned => self.text("inspect.signature.unsigned").into(),
            Signature::Unchecked => self.text("inspect.signature.unchecked").into(),
            Signature::NotSignable => self.text("inspect.signature.not_signable").into(),
        }
    }

    pub fn origin_text(self, origin: Option<&Origin>) -> String {
        let Some(o) = origin else {
            return self.text("inspect.origin.none").into();
        };
        let zone = match o.zone {
            z @ 0..=2 => self.text(&format!("inspect.zone.{z}")),
            _ => self.text("inspect.zone.internet"),
        };
        match &o.host {
            Some(host) => fill(self.text("inspect.origin.host"), &[("zone", zone), ("host", host)]),
            None => zone.into(),
        }
    }

    pub fn dropped_line(self, dropped: &Dropped, salt: u64) -> String {
        let Dropped::File(l) = dropped else {
            return match dropped {
                Dropped::InCloud(_) => self.text("dropped.in_cloud"),
                Dropped::Folder(_) => self.text("dropped.folder"),
                _ => self.text("dropped.unreadable"),
            }
            .into();
        };
        let internet = self.text("dropped.the_internet");
        let from = l.origin.as_ref().filter(|o| o.zone >= ZONE_INTERNET).map(|o| o.host.as_deref().unwrap_or(internet));
        match (&l.signature, l.executable, from) {
            (Signature::Valid { signer, catalog }, _, _) => {
                let key = if *catalog && !cfg!(windows) { "dropped.valid_package" } else { "dropped.valid" };
                self.say(key, salt, &[("signer", signer)])
            }
            (Signature::Broken { why: Broken::Tampered | Broken::Distrusted, .. }, _, _) => self.text("dropped.fake").into(),
            (Signature::Broken { why, .. }, _, _) => fill(self.text("dropped.broken"), &[("why", self.broken_why(*why))]),
            (Signature::Unchecked, _, _) => self.text("dropped.unchecked").into(),
            (_, true, Some(host)) => fill(self.text("dropped.runnable_from"), &[("host", host)]),
            (_, true, None) => self.text("dropped.runnable").into(),
            (Signature::Unsigned, false, _) => self.text("dropped.unsigned").into(),
            (_, false, Some(host)) => fill(self.text("dropped.downloaded_from"), &[("host", host)]),
            _ => self.text("dropped.not_signable").into(),
        }
    }

    // What the uninstall asks before it takes him away.
    pub fn forget_question(self) -> &'static str {
        self.text("autostart.forget")
    }

    pub fn autostart_line(self, on: bool, done: bool) -> &'static str {
        match (done, on, cfg!(windows)) {
            (true, true, true) => self.text("autostart.on"),
            (true, false, true) => self.text("autostart.off"),
            (false, _, true) => self.text("autostart.refused"),
            (true, true, false) => self.text("autostart.session_on"),
            (true, false, false) => self.text("autostart.session_off"),
            (false, _, false) => self.text("autostart.session_refused"),
        }
    }

    pub fn tool_failed(self) -> &'static str {
        self.text("tool.failed")
    }

    pub fn crashed(self, place: &str) -> String {
        fill(self.text("fault.crashed"), &[("where", place)])
    }

    pub fn wire_lost(self) -> &'static str {
        self.text("fault.wire_lost")
    }

    pub fn wire_back(self) -> &'static str {
        self.text("fault.wire_back")
    }

    pub fn copy_failed(self) -> &'static str {
        self.text("tool.copy_failed")
    }

    pub fn copied(self) -> &'static str {
        self.text("tool.copied")
    }

    pub fn menu_scanlines(self) -> &'static str {
        self.text("menu.scanlines")
    }

    pub fn menu_journal(self) -> &'static str {
        self.text("menu.journal")
    }

    pub fn journal_title(self) -> &'static str {
        self.text("journal.title")
    }

    pub fn journal_empty(self) -> &'static str {
        self.text("journal.empty")
    }

    pub fn journal_unsaid(self) -> &'static str {
        self.text("journal.unsaid")
    }

    pub fn menu_mute(self) -> &'static str {
        self.text("menu.mute")
    }

    pub fn menu_mute_hour(self) -> &'static str {
        self.text("menu.mute_hour")
    }

    pub fn menu_mute_morning(self) -> &'static str {
        self.text("menu.mute_morning")
    }

    pub fn menu_unmute(self, until: &str) -> String {
        fill(self.text("menu.unmute"), &[("until", until)])
    }

    pub fn muted(self, until: &str) -> String {
        fill(self.text("mute.muted"), &[("until", until)])
    }

    pub fn unmuted(self) -> &'static str {
        self.text("mute.unmuted")
    }

    pub fn menu_character(self) -> &'static str {
        self.text("menu.character")
    }

    pub fn menu_switch(self) -> &'static str {
        self.text("menu.switch")
    }

    pub fn menu_autostart(self) -> &'static str {
        self.text("menu.autostart")
    }

    pub fn menu_admin(self) -> &'static str {
        self.text("menu.admin")
    }

    pub fn menu_update(self, version: &str) -> String {
        fill(self.text("menu.update"), &[("version", version)])
    }

    pub fn update_found(self, version: &str, salt: u64) -> String {
        self.say("update.found", salt, &[("version", version)])
    }

    pub fn update_failed(self) -> &'static str {
        self.text("update.failed")
    }

    pub fn menu_quit(self) -> &'static str {
        self.text("menu.quit")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::Remote;
    use std::collections::BTreeSet;

    fn salt_for(set: &[&str], wanted: &str) -> u64 {
        (0..1000).find(|&s| *pick(set, s) == wanted).expect("variant reachable")
    }

    #[test]
    fn every_variant_is_reachable() {
        let set = ["a", "b", "c"];
        for v in set {
            salt_for(&set, v);
        }
        for lang in [Lang::Cs, Lang::En] {
            for (key, lines) in lang.book() {
                let reached: BTreeSet<&String> = (0..2000).map(|s| pick(lines, s)).collect();
                assert_eq!(reached.len(), lines.iter().collect::<BTreeSet<_>>().len(), "{lang:?} {key}");
            }
        }
    }

    const LEGEND: &[&str] = &[
        "who", "dest", "what", "taste", "port", "rate", "n", "app", "ssid", "name", "ip", "mac", "open", "count", "lan", "days",
        "hour", "thing", "proto", "every", "hosts", "ports", "secs", "owner", "random", "gateway", "posing", "over", "dur",
        "stage", "until", "things", "condition", "in", "out", "regulars", "known", "security", "signal", "machines", "total",
        "exposed", "programs", "internet", "more", "signer", "why", "zone", "host", "where", "version",
    ];

    fn placeholders(line: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut rest = line;
        while let Some(open) = rest.find('{') {
            let Some(close) = rest[open..].find('}') else { break };
            found.insert(rest[open + 1..open + close].to_string());
            rest = &rest[open + close + 1..];
        }
        found
    }

    #[test]
    fn both_languages_have_every_line() {
        let keys = |lang: Lang| -> BTreeSet<&String> { lang.book().keys().filter(|k| !k.ends_with(".few")).collect() };
        let (cs, en) = (keys(Lang::Cs), keys(Lang::En));
        assert!(cs.len() > 200, "the Czech voice file did not read");
        assert_eq!(cs.difference(&en).collect::<Vec<_>>(), Vec::<&&String>::new(), "only in cs.toml");
        assert_eq!(en.difference(&cs).collect::<Vec<_>>(), Vec::<&&String>::new(), "only in en.toml");
    }

    #[test]
    fn placeholders_are_known_and_match_across_languages() {
        let used = |lang: Lang, key: &str| -> BTreeSet<String> {
            lang.book().get(key).into_iter().flatten().flat_map(|l| placeholders(l)).collect()
        };
        for (key, lines) in book::cs() {
            for line in lines.iter().chain(book::en().get(key).into_iter().flatten()) {
                for name in placeholders(line) {
                    assert!(LEGEND.contains(&name.as_str()), "unknown {{{name}}} in {key}: {line}");
                }
            }
            // Plural forms spell the count out in one and not in others.
            if !key.contains(".one") && !key.ends_with(".few") {
                assert_eq!(used(Lang::Cs, key), used(Lang::En, key), "{key} fills different things in cs and en");
            }
        }
    }

    #[test]
    fn nothing_is_left_unfilled() {
        use crate::tools::inspect::Look;
        let dest = |ip: IpAddr| ip.to_string();
        let ip: IpAddr = "192.0.2.7".parse().unwrap();
        let v4 = std::net::Ipv4Addr::new(192, 168, 1, 23);
        let exposures = [
            Exposure::RemoteDesktop,
            Exposure::Vnc,
            Exposure::FileShare,
            Exposure::Telnet,
            Exposure::Ftp,
            Exposure::Database,
            Exposure::DockerApi,
            Exposure::WinRm,
            Exposure::DevServer,
        ];
        let mut findings = vec![
            Finding::Cleartext { process: "curl".into(), remote: ip, what: Cleartext::Http },
            Finding::Lolbin { process: "certutil".into(), remote: ip },
            Finding::Lolbin { process: "powershell".into(), remote: ip },
            Finding::FromTemp { process: "x".into(), remote: ip },
            Finding::Beacon { process: "x".into(), remote: ip, every: 60 },
            Finding::Sweep { process: "x".into(), hosts: 40, ports: 1 },
            Finding::Sweep { process: "x".into(), hosts: 1, ports: 40 },
            Finding::Upload { rate: 1 << 20, secs: 20, owner: Some(("x".into(), ip)), away: true },
            Finding::Upload { rate: 1 << 20, secs: 20, owner: None, away: false },
            Finding::Newcomer { process: "".into(), remote: ip },
            Finding::Tor { process: "tor".into() },
            Finding::Mining { process: "x".into(), remote: ip },
            Finding::DnsBypass { process: "x".into(), remote: ip },
            Finding::NewDevice { ip: v4, mac: "aa".into(), name: Some("tiskarna".into()), random: true },
            Finding::NewDevice { ip: v4, mac: "aa".into(), name: None, random: false },
            Finding::GatewaySpoof { gateway: v4, mac: "aa".into(), posing: v4, name: Some("pc".into()) },
            Finding::GatewayChanged { gateway: v4, mac: "aa".into() },
            Finding::Offline { started: 0, local: true },
            Finding::Offline { started: 0, local: false },
            Finding::Online { started: 0, secs: 125 },
            Finding::NewAutorun { place: crate::watch::autoruns::Place::Run, name: "Updater".into() },
            Finding::NewAutorun { place: crate::watch::autoruns::Place::Startup, name: "helper.lnk".into() },
            Finding::NewAutorun { place: crate::watch::autoruns::Place::Service, name: "Helper".into() },
            Finding::NewAutorun { place: crate::watch::autoruns::Place::Task, name: "\\Helper".into() },
            Finding::HostsAdded { name: "bank.example".into(), ip: "10.0.0.1".into(), more: 0 },
            Finding::HostsAdded { name: "bank.example".into(), ip: "10.0.0.1".into(), more: 2 },
            Finding::ProxySet { via: "127.0.0.1:8080".into() },
            Finding::DnsChanged { adapter: "Wi-Fi".into(), servers: "10.0.0.53".into() },
            Finding::GatewayFlapped { gateway: v4, mac: "aa".into() },
        ];
        for why in [Oddity::Punycode, Oddity::CheapTld, Oddity::RandomLabel] {
            findings.push(Finding::OddName { process: "x".into(), name: "xn--e1a.example".into(), why });
        }
        for what in exposures {
            findings.push(Finding::Exposed { process: "x".into(), port: 5900, what, again: false });
            findings.push(Finding::Exposed { process: "x".into(), port: 5900, what, again: true });
            findings.push(Finding::Unexposed { process: "x".into(), port: 5900, what });
        }
        for what in [Remote::Ssh, Remote::Rdp, Remote::Telnet, Remote::Vnc, Remote::WinRm] {
            for (incoming, lasted) in [(true, None), (true, Some(90)), (false, Some(4000)), (false, None)] {
                findings.push(Finding::Session { process: "x".into(), remote: ip, what, incoming, started: 1, lasted });
            }
        }
        let file = |signature, executable, origin| Dropped::File(Look { name: "a.exe".into(), size: 1, sha256: String::new(), executable, signature, origin });
        let web = || Some(Origin { zone: 3, host: Some("example.net".into()) });
        let signatures = || {
            let mut all = vec![Signature::Valid { signer: "S".into(), catalog: false }, Signature::Valid { signer: "S".into(), catalog: true }];
            for why in [Broken::Tampered, Broken::Distrusted, Broken::Expired, Broken::UntrustedRoot, Broken::Other] {
                all.push(Signature::Broken { signer: Some("S".into()), why });
                all.push(Signature::Broken { signer: None, why });
            }
            all.extend([Signature::Unsigned, Signature::Unchecked, Signature::NotSignable]);
            all
        };
        let kinds = [
            Kind::Web,
            Kind::PlainWeb,
            Kind::Ssh,
            Kind::RemoteDesktop,
            Kind::WinRm,
            Kind::Dns,
            Kind::Mail,
            Kind::FileShare,
            Kind::Database,
            Kind::Git,
            Kind::Chat,
            Kind::Call,
            Kind::Game,
            Kind::Other(8443),
        ];

        for lang in [Lang::Cs, Lang::En] {
            let mut lines: Vec<String> = Vec::new();
            for salt in 0..64 {
                for hour in [3, 7, 12, 19, 23] {
                    lines.extend([lang.greeting(salt, hour), lang.lore(salt, hour)]);
                }
                for kind in kinds {
                    lines.push(lang.opened("curl", kind, &lang.dest("ovh.net", Place::Lan), Place::Internet, salt));
                    lines.push(lang.opened("github", kind, "github.com", Place::Internet, salt));
                }
                for service in crate::net::services::known() {
                    lines.push(lang.service(service, "x", "example.net", salt));
                }
                for f in &findings {
                    lines.push(lang.finding(f, &dest, salt));
                }
                for idle in [Idle::Tinker, Idle::Read, Idle::Game, Idle::Snack] {
                    lines.push(lang.hobby(idle, salt));
                }
                lines.extend([
                    lang.spike("1 MB/s", true, salt),
                    lang.spike("1 MB/s", false, salt),
                    lang.flow("1 MB/s", true, "x", "y", salt),
                    lang.flow("1 MB/s", false, "x", "y", salt),
                    lang.feast("1 MB/s", Some("x"), salt),
                    lang.feast("1 MB/s", None, salt),
                    lang.woke(salt),
                    lang.nap(salt),
                    lang.dozed(salt),
                    lang.starving(salt),
                    lang.hungry(salt),
                    lang.bored(salt),
                    lang.regular("x", "y", 4, salt),
                    lang.night_owl(3, salt),
                    lang.petted(1, salt),
                    lang.petted(9, salt),
                    lang.patrol(salt).into(),
                    lang.sits(salt).into(),
                    lang.back(salt).into(),
                    lang.fell(salt).into(),
                    lang.covered(salt).into(),
                    lang.travel(salt).into(),
                    lang.arrived(salt).into(),
                    lang.escaping("", salt),
                    lang.hiding("vlc", salt),
                    lang.network_line(Some("203.0.113.5"), salt),
                ]);
                for signature in signatures() {
                    lines.push(lang.dropped_line(&file(signature, true, web()), salt));
                }
            }
            for (i, signature) in signatures().into_iter().enumerate() {
                lines.push(lang.signature_text(&signature));
                for (exe, origin) in [(true, None), (false, None), (false, web())] {
                    lines.push(lang.dropped_line(&file(signatures().swap_remove(i), exe, origin), 0));
                }
            }
            for dropped in [Dropped::InCloud("a".into()), Dropped::Folder("a".into()), Dropped::Unreadable("a".into())] {
                lines.push(lang.dropped_line(&dropped, 0));
            }
            for zone in 0..5 {
                lines.push(lang.origin_text(Some(&Origin { zone, host: Some("h".into()) })));
                lines.push(lang.origin_text(Some(&Origin { zone, host: None })));
            }
            lines.push(lang.origin_text(None));
            for n in [0, 1, 2, 5, 22] {
                let details = Details {
                    satiety: 50.0,
                    mood: 50.0,
                    neglect: n as f32 / 22.0,
                    stage: n as u8 % 5,
                    days_to_grow: (n > 0).then_some(n),
                    age_days: n.into(),
                    today: ("1".into(), "2".into()),
                    total: ("3".into(), "4".into()),
                    regulars: 1,
                    known: 2,
                    open: vec![(Exposure::Vnc, 5900), (Exposure::WinRm, 5985)],
                    muted_until: Some("07:00".into()),
                };
                let sheet = lang.sheet(&details);
                lines.extend([sheet.name, sheet.stage]);
                lines.extend(sheet.about);
                lines.extend(sheet.rows);
                lines.extend(sheet.warnings);
                lines.extend(sheet.meters.into_iter().map(|m| m.0));
                lines.push(lang.level_up(n as u8).into());
                lines.push(lang.calls_line(Some(("x", 3)), n as usize));
                lines.push(lang.neighbours_line(Some((n as usize, 0))));
                lines.push(lang.neighbours_line(Some((n as usize, 2))));
                lines.push(lang.listening_line(4, n as usize));
            }
            for tool in crate::tools::Tool::ALL {
                lines.push(lang.tool_name(tool).into());
            }
            for sharing in [Sharing::Shared, Sharing::Open, Sharing::NeedsAdmin, Sharing::Enterprise, Sharing::Unknown] {
                lines.push(lang.share_line("home", sharing));
            }
            for verdict in [
                Verdict::NoNetwork,
                Verdict::NoGateway,
                Verdict::NoInternet,
                Verdict::NoDns,
                Verdict::Fine { gateway: Some(1), internet: 20 },
                Verdict::Fine { gateway: None, internet: 20 },
            ] {
                lines.push(lang.ping_line(&verdict));
            }
            for i in 0..4 {
                lines.push(lang.inspect_label(i).into());
            }
            lines.extend([
                lang.intro().into(),
                lang.hidden().into(),
                lang.who(""),
                lang.private("x", 0),
                lang.right_after("x", "y"),
                lang.busiest("x", 3),
                lang.stuffed().into(),
                lang.summary(9, Some(("x", 3)), 2),
                lang.menu_tools().into(),
                lang.calls_line(None, 0),
                lang.tool_busy().into(),
                lang.tool_still_busy().into(),
                lang.network_line(None, 0),
                lang.wifi_line("home", 70, "WPA2", false),
                lang.wifi_line("home", 70, "WEP", true),
                lang.wifi_denied().into(),
                lang.wifi_off().into(),
                lang.wifi_none().into(),
                lang.neighbours_line(None),
                lang.usage_line(Some(("x", "1 MB/s"))),
                lang.usage_line(None),
                lang.panel_hint().into(),
                lang.inspect_title().into(),
                lang.inspect_busy().into(),
                lang.inspect_more(2),
            ]);
            lines.extend([lang.autostart_line(true, true), lang.autostart_line(false, true), lang.autostart_line(true, false)].map(String::from));
            lines.extend(
                [
                    lang.tool_failed(),
                    lang.copy_failed(),
                    lang.copied(),
                    lang.menu_scanlines(),
                    lang.menu_journal(),
                    lang.journal_title(),
                    lang.journal_empty(),
                    lang.journal_unsaid(),
                    lang.menu_mute(),
                    lang.menu_mute_hour(),
                    lang.menu_mute_morning(),
                    lang.unmuted(),
                    lang.menu_character(),
                    lang.menu_switch(),
                    lang.menu_autostart(),
                    lang.menu_admin(),
                    lang.menu_quit(),
                ]
                .map(String::from),
            );
            lines.extend([lang.menu_unmute("07:00"), lang.muted("07:00"), lang.crashed("net.rs:123")]);
            lines.extend([lang.wire_lost(), lang.wire_back()].map(String::from));
            for line in &lines {
                assert!(!line.contains('{') && !line.contains('}') && !line.contains(MISSING), "{lang:?}: {line}");
            }
        }
    }

    #[test]
    fn every_kind_of_seat_has_lines() {
        for kind in crate::detect::SeatKind::ALL {
            for lang in [Lang::Cs, Lang::En] {
                assert!(lang.book().contains_key(&format!("sits_on.{kind:?}")), "{kind:?} {lang:?}");
            }
        }
        for salt in 0..10 {
            let line = Lang::Cs.sits_on("code", salt);
            assert!(!line.contains('{') && !line.is_empty(), "{line}");
            assert!(!Lang::Cs.sits_on("notanapp", salt).is_empty());
        }
    }

    #[test]
    fn never_names_the_app_twice() {
        for salt in 0..20 {
            for lang in [Lang::Cs, Lang::En] {
                let line = lang.service(Service::Discord, "discord", "discord.com", salt);
                assert_eq!(line.to_lowercase().matches("discord").count(), 1, "{line}");
                let line = lang.opened("discord", Kind::Web, "discord.com", Place::Internet, salt);
                assert_eq!(line.matches("discord").count(), 1, "{line}");
                let line = lang.opened("curl", Kind::Web, "ovh.net", Place::Internet, salt);
                assert!(line.contains("curl") && line.contains("ovh.net"), "{line}");
            }
        }
    }

    #[test]
    fn every_known_service_has_a_voice_of_its_own() {
        for service in crate::net::services::known().into_iter().filter(|&s| s != Service::Private) {
            for lang in [Lang::Cs, Lang::En] {
                let lines: std::collections::HashSet<String> = (0..300).map(|salt| lang.service(service, "x", "example.net", salt)).collect();
                assert!(lines.len() >= 3, "{service:?} {lang:?} has {} lines", lines.len());
            }
        }
    }

    #[test]
    fn fills_placeholders() {
        assert_eq!(fill("{who} to {dest}{taste}", &[("who", "curl"), ("dest", "ovh.net"), ("taste", "")]), "curl to ovh.net");
    }

    #[test]
    fn a_line_is_finished_as_it_is_said() {
        assert_eq!(polished("msedge opened https to seznam.cz. Crunchy"), "Msedge opened https to seznam.cz. Crunchy.");
        assert_eq!(polished("Kdo mi to tam leze?"), "Kdo mi to tam leze?");
        assert_eq!(polished("A dost, kurva! Přehryžu ti kabely!"), "A dost, kurva! Přehryžu ti kabely!");
        assert_eq!(polished("čau, sem Raccy\nžeru pakety"), "Čau, sem Raccy\nŽeru pakety.");
        assert_eq!(polished("nešlo to ověřit ¯\\_(ツ)_/¯"), "Nešlo to ověřit ¯\\_(ツ)_/¯", "the shrug ends a line");
        let once = polished("2.1 MB/s se valí dovnitř");
        assert_eq!(once, "2.1 MB/s se valí dovnitř.");
        assert_eq!(polished(&once), once, "saying it again changes nothing");
    }

    #[test]
    fn counts_take_the_right_form() {
        assert_eq!(Lang::Cs.calls_line(Some(("x", 2)), 1), "Nejvíc donáší x, dneska 2×. Domů volaly 1 program");
        assert_eq!(polished(&Lang::Cs.neighbours_line(Some((3, 0)))), "Jsou tu 3 další mašiny. Seznam je v okně.");
        assert_eq!(polished(&Lang::Cs.neighbours_line(Some((12, 0)))), "Je tu 12 dalších mašin. Seznam je v okně.");
        assert_eq!(polished(&Lang::En.calls_line(Some(("x", 2)), 4)), "X tells on you the most, 2 times today. 4 programs called home.");
    }

    #[test]
    fn czech_lines_read_right() {
        let dest = Lang::Cs.dest("192.168.1.30", Place::Lan);
        let line = Lang::Cs.opened("pwsh", Kind::WinRm, &dest, Place::Lan, 0);
        assert!(line.contains("pwsh") && line.contains("192.168.1.30 v lanu") && line.ends_with("šťourá do jiný mašiny"), "{line}");
        let flow = Lang::Cs.flow("45.1 MB/s", true, "curl", "ovh.net", 0);
        assert!(flow.contains("45.1 MB/s") && flow.contains("curl") && flow.contains("ovh.net"), "{flow}");
        assert_eq!(Lang::Cs.who(""), "systémovej proces");
        let details = Details {
            satiety: 72.4,
            mood: 99.7,
            neglect: 0.6,
            stage: 1,
            days_to_grow: Some(9),
            age_days: 3,
            today: ("1.2 GB".into(), "30 MB".into()),
            total: ("5.3 GB".into(), "182.1 MB".into()),
            regulars: 12,
            known: 40,
            open: vec![(Exposure::Vnc, 5800)],
            muted_until: None,
        };
        let sheet = Lang::Cs.sheet(&details);
        assert_eq!(sheet.stage, "vekslák");
        assert_eq!(sheet.about, vec!["stadium 2 ze 4", "do dalšího stadia 9 nasycenejch dní", "starej 3 dny"]);
        assert_eq!(sheet.meters[0], ("sytost".to_string(), 72.4));
        assert!(sheet.rows.iter().any(|r| r.ends_with("natržený ucho")), "{:?}", sheet.rows);
        assert_eq!(sheet.rows[2], "dnes      ↓1.2 GB ↑30 MB");
        assert_eq!(sheet.warnings, vec!["otevřený do sítě: vnc na portu 5800".to_string()]);
    }

    #[test]
    fn no_cyrillic_lookalikes_in_czech_copy() {
        assert!(!book::CS.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)));
    }

    #[test]
    fn the_introduction_fits_the_bubble() {
        for lang in [Lang::Cs, Lang::En] {
            let intro = lang.intro();
            assert_eq!(intro.lines().count(), 5, "{intro}");
            assert!(intro.lines().all(|l| l.chars().count() <= 30), "{intro}");
        }
    }

    #[test]
    fn private_services_are_never_named() {
        let line = Lang::En.service(Service::Private, "msedge", "ib.fio.cz", 0);
        assert!(!line.contains("fio"), "{line}");
    }

    #[test]
    fn findings_carry_their_evidence() {
        let dest = |ip: IpAddr| ip.to_string();
        for salt in 0..6 {
            let exposed = Finding::Exposed { process: "tvnserver".into(), port: 5900, what: Exposure::Vnc, again: false };
            let line = Lang::Cs.finding(&exposed, &dest, salt);
            assert!(line.contains("tvnserver") && line.contains("5900"), "{line}");
            let again = Finding::Exposed { process: "tvnserver".into(), port: 5900, what: Exposure::Vnc, again: true };
            assert!(Lang::Cs.finding(&again, &dest, salt).contains("5900"));
            let closed = Finding::Unexposed { process: "tvnserver".into(), port: 5900, what: Exposure::Vnc };
            assert!(Lang::Cs.finding(&closed, &dest, salt).contains("vnc na portu 5900"));
        }
        let upload = Finding::Upload { rate: 5 * 1024 * 1024, secs: 20, owner: Some(("rclone".into(), "198.51.100.20".parse().unwrap())), away: true };
        let line = Lang::En.finding(&upload, &dest, 0);
        assert_eq!(line, "You're away and 5.0 MB/s has been leaving for 20 s. rclone to 198.51.100.20");
    }

    #[test]
    fn a_dropped_file_is_judged_by_its_papers() {
        use crate::tools::inspect::{Broken, Dropped, Look, Origin, Signature};
        let file = |signature, executable, origin| {
            Dropped::File(Look { name: "setup.exe".into(), size: 1, sha256: String::new(), executable, signature, origin })
        };
        let web = || Some(Origin { zone: 3, host: Some("example.net".into()) });
        let signed = file(Signature::Valid { signer: "Contoso".into(), catalog: false }, true, web());
        assert!(Lang::Cs.dropped_line(&signed, 0).contains("Contoso"));
        let fake = file(Signature::Broken { signer: None, why: Broken::Tampered }, true, None);
        assert!(Lang::En.dropped_line(&fake, 0).contains("Don't run it"));
        let loose = file(Signature::Unsigned, true, web());
        assert!(Lang::Cs.dropped_line(&loose, 0).contains("example.net"));
        assert_eq!(Lang::Cs.origin_text(web().as_ref()), "internet, example.net");
        let broken = Signature::Broken { signer: Some("Contoso".into()), why: Broken::Expired };
        assert_eq!(Lang::En.signature_text(&broken), "invalid: the certificate has expired, Contoso");
    }

    #[test]
    fn the_lan_watch_names_the_machine() {
        let dest = |ip: IpAddr| ip.to_string();
        let ip = std::net::Ipv4Addr::new(192, 168, 1, 40);
        for salt in 0..3 {
            let named = Finding::NewDevice { ip, mac: "aa:bb:cc:dd:ee:ff".into(), name: Some("tiskarna".into()), random: false };
            let line = Lang::Cs.finding(&named, &dest, salt);
            assert!(line.contains("tiskarna (192.168.1.40)"), "{line}");
            let bare = Finding::NewDevice { ip, mac: "aa:bb:cc:dd:ee:ff".into(), name: None, random: true };
            let line = Lang::En.finding(&bare, &dest, salt);
            assert!(line.contains("192.168.1.40, mac aa:bb:cc:dd:ee:ff with a random mac"), "{line}");
        }
        let spoof = Finding::GatewaySpoof { gateway: std::net::Ipv4Addr::new(192, 168, 1, 1), mac: "aa:bb:cc:dd:ee:ff".into(), posing: ip, name: None };
        let line = Lang::Cs.finding(&spoof, &dest, 0);
        assert!(line.contains("192.168.1.1") && line.contains("192.168.1.40") && line.contains("aa:bb:cc:dd:ee:ff"), "{line}");
    }

    #[test]
    fn sessions_read_with_destination_and_duration() {
        let dest = |_: IpAddr| "forge v lanu".to_string();
        let remote: IpAddr = "192.168.1.20".parse().unwrap();
        let closed = Finding::Session { process: "ssh".into(), remote, what: Remote::Ssh, incoming: false, started: 1, lasted: Some(2520) };
        let line = Lang::Cs.finding(&closed, &dest, 0);
        assert!(line.contains("forge v lanu") && line.contains("42 min"), "{line}");
        let rdp_in = Finding::Session { process: "svchost".into(), remote, what: Remote::Rdp, incoming: true, started: 1, lasted: None };
        assert!(Lang::Cs.finding(&rdp_in, &dest, 0).contains("přes vzdálenou plochu"));
        assert_eq!(duration(3900), "1 h 5 min");
    }

    #[test]
    fn no_em_dashes_or_emoji_in_any_line() {
        for src in [book::CS, book::EN, include_str!("mod.rs")] {
            assert!(!src.contains('\u{2014}'), "em dash in copy");
            assert!(!src.chars().any(|c| ('\u{1F300}'..='\u{1FAFF}').contains(&c)), "emoji in copy");
        }
    }
}
