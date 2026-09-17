use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;

use crate::lang::Lang;
use crate::tuning::tuning;
use crate::net::{self, Conn, Kind, Names, Place, Tick};
use crate::pet::{Activity, Pet};
use crate::net::services::{self, Service};
use crate::watch::{Finding, Remote, Severity};

const STALE: u32 = 20;
const TOLD_KEPT: usize = 500;

#[derive(Debug, PartialEq)]
pub struct Said {
    pub text: String,
    pub severity: Option<Severity>,
    pub kind: Option<Kind>,
}

impl Said {
    fn plain(text: String) -> Said {
        Said { text, severity: None, kind: None }
    }

    fn about(text: String, kind: Option<Kind>) -> Said {
        Said { text, severity: None, kind }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Context {
    pub idle_secs: u64,
    pub hushed: bool,
    pub hour: u8,
    pub day: u32,
    // Unix time.
    pub now: u64,
}

struct Pending {
    conn: Conn,
    age: u32,
    visited: bool,
}

#[derive(Clone)]
struct Share {
    conn: Conn,
    total: u64,
}

// Kernel byte counts and DNS names both trail the traffic by a second or so.
struct Burst {
    incoming: bool,
    peak: u64,
    waited: u32,
    flows: HashMap<(u32, IpAddr, u16), Share>,
}

#[derive(Default)]
pub struct Talk {
    clock: u64,
    last_line: u64,
    last_warned: bool,
    line_until: u64,
    last_spike: Option<u64>,
    burst: Option<Burst>,
    last_moan: u64,
    last_summary: u64,
    last_lore: u64,
    last_chatter: u64,
    // Bytes a second, both ways.
    appetite: f64,
    feasting: u64,
    last_feast: Option<u64>,
    last_activity: Option<Activity>,
    last_opened: Option<(Conn, u64)>,
    pending: VecDeque<Pending>,
    mentioned: HashMap<(String, Kind), u64>,
    services_said: HashMap<Service, u64>,
    findings: Vec<(Finding, u64)>,
    // By key, with the unix time.
    pub told: HashMap<String, u64>,
    standing: HashMap<String, (Finding, u64)>,
    now: u64,
    subjects: HashMap<(String, IpAddr), u64>,
    pub visits: Vec<String>,
    pub calls: Vec<(String, String)>,
    // Warning, text, and whether it was said aloud.
    pub journal: Vec<(bool, String, bool)>,
    greeted: HashSet<String>,
    greeted_day: u32,
    night_owl_day: Option<u32>,
}

impl Talk {
    pub fn remembering(told: HashMap<String, u64>) -> Talk {
        Talk { told, ..Talk::default() }
    }

    pub fn exposed(&self) -> Vec<(crate::watch::Exposure, u16)> {
        let mut open: Vec<_> = self
            .standing
            .values()
            .filter_map(|(f, _)| match f {
                Finding::Exposed { what, port, .. } => Some((*what, *port)),
                _ => None,
            })
            .collect();
        open.sort_by_key(|&(_, port)| port);
        open
    }

    pub fn hold(&mut self) {
        self.last_line = self.clock.max(1);
        self.last_warned = true;
    }

    pub fn showing(&mut self, secs: u64) {
        self.line_until = self.clock + secs;
    }

    fn chatter_every(&self) -> u64 {
        match self.appetite as u64 {
            a if a >= tuning().talk.big_feast_bps => tuning().talk.chatter_big_feast_secs,
            a if a >= tuning().talk.feast_bps => tuning().talk.chatter_feast_secs,
            a if a >= tuning().talk.trickle_bps => tuning().talk.chatter_trickle_secs,
            _ => tuning().talk.chatter_quiet_secs,
        }
    }

    pub fn tick(
        &mut self,
        tick: &Tick,
        names: &Names,
        pet: &Pet,
        found: Vec<Finding>,
        cx: Context,
        lang: Lang,
    ) -> Option<Said> {
        self.clock += 1;
        self.now = cx.now;
        // Asleep, notes wait for him to wake rather than going stale unsaid.
        if pet.activity() == Activity::Sleeping {
            for (f, at) in &mut self.findings {
                if f.severity() == Severity::Note {
                    *at = (*at + 1).min(self.clock);
                }
            }
        }
        self.appetite += ((tick.rx + tick.tx) as f64 - self.appetite) / tuning().talk.appetite_secs;
        self.feasting = if self.appetite >= tuning().talk.feast_bps as f64 { self.feasting + 1 } else { 0 };
        for conn in &tick.opened {
            if conn.place == Place::Internet {
                self.last_opened = Some((conn.clone(), self.clock));
            }
            let dup = self.pending.iter().any(|p| p.conn.process == conn.process && p.conn.remote == conn.remote);
            if !dup {
                names.get(conn.remote);
                self.pending.push_back(Pending { conn: conn.clone(), age: 0, visited: false });
            }
        }
        for p in &mut self.pending {
            p.age += 1;
        }
        self.pending.retain(|p| p.age <= STALE);
        for p in &mut self.pending {
            if p.visited || p.conn.place != Place::Internet {
                continue;
            }
            if let Some(name) = names.get(p.conn.remote) {
                p.visited = true;
                if !services::by_domain(&name).is_some_and(Service::discreet) {
                    self.visits.push(net::short_domain(&name));
                    if services::calls_home(&name) {
                        self.calls.push((p.conn.process.clone(), name.trim_end_matches('.').to_lowercase()));
                    }
                }
            }
        }
        if self.greeted_day != cx.day {
            self.greeted_day = cx.day;
            self.greeted.clear();
        }
        self.stand(&found, tick);
        self.queue(found, names, lang);

        let activity = pet.activity();
        let woke = self.last_activity == Some(Activity::Sleeping) && activity != Activity::Sleeping;
        let dozed = self.last_activity.is_some_and(|a| a != Activity::Sleeping) && activity == Activity::Sleeping;
        self.last_activity = Some(activity);

        if cx.hushed {
            self.burst = None;
            return None;
        }
        let present = cx.idle_secs < tuning().talk.hold_idle_secs;
        let since = self.clock.saturating_sub(self.last_line);
        let said = self.choose(tick, names, pet, activity, woke, dozed, present, since, cx, lang)?;
        self.last_line = self.clock;
        self.last_warned = said.severity == Some(Severity::Warn);
        Some(said)
    }

    #[allow(clippy::too_many_arguments)]
    fn choose(
        &mut self,
        tick: &Tick,
        names: &Names,
        pet: &Pet,
        activity: Activity,
        woke: bool,
        dozed: bool,
        present: bool,
        since: u64,
        cx: Context,
        lang: Lang,
    ) -> Option<Said> {
        let fresh = self.last_line == 0;
        let gap = if self.last_warned { u64::from(tuning().talk.warn_secs) } else { tuning().talk.warn_gap_secs };
        if present
            && since >= gap
            && let Some(said) = self.next_finding(Severity::Warn, names, lang)
        {
            return Some(said);
        }
        if activity == Activity::Sleeping {
            self.burst = None;
        } else if let Some((text, kind)) = self.spike(tick, names, activity, lang) {
            return Some(Said::about(text, kind));
        }
        let cooldown = if self.last_warned { u64::from(tuning().talk.warn_secs) } else { tuning().talk.cooldown_secs };
        if !fresh && (since < cooldown || self.clock < self.line_until) {
            return None;
        }
        let salt = self.clock;
        if woke {
            return Some(Said::plain(lang.woke(salt)));
        }
        // A nap was already announced; the quiet-wire line would be wrong.
        if dozed && !pet.napping() {
            return Some(Said::plain(lang.dozed(salt)));
        }
        if activity == Activity::Sleeping {
            return None;
        }
        if present
            && let Some(said) = self.next_finding(Severity::Note, names, lang)
        {
            return Some(said);
        }
        let chatter_due = fresh || self.clock.saturating_sub(self.last_chatter) >= self.chatter_every();
        if chatter_due && let Some((text, kind)) = self.next_connection(names, pet, lang) {
            self.last_chatter = self.clock;
            return Some(Said::about(text, Some(kind)));
        }
        if chatter_due && self.feasting >= tuning().talk.feast_hold_secs && self.last_feast.is_none_or(|t| self.clock - t >= tuning().talk.feast_every_secs) {
            (self.last_feast, self.last_chatter) = (Some(self.clock), self.clock);
            let rate = rate(self.appetite as u64);
            return Some(match meal(tick) {
                Some((process, kind)) => Said::about(lang.feast(&rate, Some(&process), salt), Some(kind)),
                None => Said::plain(lang.feast(&rate, None, salt)),
            });
        }
        if self.clock - self.last_moan >= tuning().talk.moan_every_secs {
            let moan = match activity {
                Activity::Starving => Some(lang.starving(salt)),
                Activity::Hungry => Some(lang.hungry(salt)),
                Activity::Bored => Some(lang.bored(salt)),
                _ => None,
            };
            if let Some(moan) = moan {
                self.last_moan = self.clock;
                return Some(Said::plain(moan));
            }
        }
        if present && (1..=4).contains(&cx.hour) && self.night_owl_day != Some(cx.day) {
            self.night_owl_day = Some(cx.day);
            return Some(Said::plain(lang.night_owl(cx.hour, salt)));
        }
        if self.clock - self.last_summary >= tuning().talk.summary_every_secs && !tick.active.is_empty() {
            self.last_summary = self.clock;
            let lan = tick.active.iter().filter(|c| c.place == Place::Lan).count();
            let top = busiest(&tick.active);
            let text = lang.summary(tick.active.len(), top.as_ref().map(|(p, n)| (p.as_str(), *n)), lan);
            return Some(Said::plain(text));
        }
        let calm = matches!(activity, Activity::Content | Activity::Eating) && self.appetite < tuning().talk.feast_bps as f64;
        if calm && since >= tuning().talk.lore_quiet_secs && self.clock - self.last_lore >= tuning().talk.lore_every_secs {
            self.last_lore = self.clock;
            return Some(Said::plain(lang.lore(salt, cx.hour)));
        }
        None
    }

    fn queue(&mut self, found: Vec<Finding>, names: &Names, lang: Lang) {
        let (clock, now) = (self.clock, self.now);
        if self.told.len() > TOLD_KEPT {
            self.told.retain(|_, t| now.saturating_sub(*t) < 7 * 86_400);
        }
        if self.told.len() > TOLD_KEPT {
            let mut times: Vec<u64> = self.told.values().copied().collect();
            times.sort_unstable_by(|a, b| b.cmp(a));
            let cutoff = times[TOLD_KEPT - 1];
            self.told.retain(|_, t| *t >= cutoff);
        }
        // Both are only ever read through a window on the clock, so anything
        // past its window is already the same as absent.
        self.subjects.retain(|_, t| clock - *t < tuning().talk.subject_quiet_secs);
        self.mentioned.retain(|_, t| clock - *t < tuning().talk.repeat_every_secs);
        for f in found {
            let key = f.key();
            let told = self.told.get(&key).copied();
            let repeat = match (&f, f.severity()) {
                (Finding::Exposed { .. }, Severity::Warn) => tuning().talk.exposed_remind_secs,
                (Finding::Exposed { .. }, Severity::Note) => tuning().talk.exposed_note_remind_secs,
                (_, Severity::Warn) => tuning().talk.warn_repeat_secs,
                (_, Severity::Note) => tuning().talk.note_repeat_secs,
            };
            let recent = told.is_some_and(|t| now.saturating_sub(t) < repeat);
            let waiting = self.findings.iter().any(|(q, _)| q.key() == key);
            if recent {
                continue;
            }
            if waiting {
                if let (Finding::Upload { .. }, Some(queued)) = (&f, self.findings.iter_mut().find(|(q, _)| q.key() == key)) {
                    queued.0 = f;
                }
                continue;
            }
            if let Finding::Session { process, remote, what, started, lasted: Some(_), .. } = &f {
                self.findings.retain(|(q, _)| {
                    !matches!(q, Finding::Session { process: p, remote: r, what: w, started: s, lasted: None, .. }
                        if p == process && r == remote && w == what && s == started)
                });
            }
            let f = match f {
                Finding::Exposed { process, port, what, .. } if told.is_some() => {
                    Finding::Exposed { process, port, what, again: true }
                }
                f => f,
            };
            if let Some(ip) = f.remote() {
                names.get(ip);
            }
            self.findings.push((f, clock));
        }
        let (keep, stale): (Vec<_>, Vec<_>) = std::mem::take(&mut self.findings)
            .into_iter()
            .partition(|(f, at)| (f.severity() == Severity::Warn && clock - at < tuning().talk.warn_stale_secs) || clock - at < tuning().talk.note_stale_secs);
        self.findings = keep;
        for (f, _) in stale {
            self.told.insert(f.key(), now);
            if by_design(&f, names) {
                continue;
            }
            let dest = |ip: IpAddr| display(names, ip, lang);
            self.journal.push((f.severity() == Severity::Warn, lang.finding(&f, &dest, clock), false));
        }
    }

    fn stand(&mut self, found: &[Finding], tick: &Tick) {
        let clock = self.clock;
        for f in found.iter().filter(|f| matches!(f, Finding::Exposed { .. })) {
            self.standing.insert(f.key(), (f.clone(), clock));
        }
        if tick.listeners.is_none() {
            return;
        }
        let open = |key: &str, standing: &HashMap<String, (Finding, u64)>| {
            standing.get(key).is_some_and(|(_, seen)| *seen == clock)
        };
        let gone: Vec<String> =
            self.told.keys().filter(|k| k.starts_with("exposed:") && !open(k, &self.standing)).cloned().collect();
        for key in gone {
            self.told.remove(&key);
            if let Some((Finding::Exposed { process, port, what, .. }, _)) = self.standing.get(&key).cloned() {
                self.findings.push((Finding::Unexposed { process, port, what }, clock));
            }
        }
        let standing = std::mem::take(&mut self.standing);
        self.findings.retain(|(f, _)| !matches!(f, Finding::Exposed { .. }) || open(&f.key(), &standing));
        self.standing = standing.into_iter().filter(|(_, (_, seen))| *seen == clock).collect();
    }

    fn next_finding(&mut self, severity: Severity, names: &Names, lang: Lang) -> Option<Said> {
        let clock = self.clock;
        loop {
            let named = |f: &Finding, at: u64| {
                clock - at >= u64::from(tuning().talk.name_wait_secs) || f.remote().is_none_or(|ip| names.get(ip).is_some())
            };
            let pos = self.findings.iter().position(|(f, at)| f.severity() == severity && named(f, *at))?;
            let (finding, _) = self.findings.remove(pos);
            self.told.insert(finding.key(), self.now);
            if by_design(&finding, names) {
                continue;
            }
            if let (Some(process), Some(remote)) = (finding.process(), finding.remote()) {
                self.subjects.insert((process.to_string(), remote), self.clock);
            }
            let dest = |ip: IpAddr| display(names, ip, lang);
            let text = lang.finding(&finding, &dest, self.clock);
            self.journal.push((severity == Severity::Warn, text.clone(), true));
            return Some(Said { text, severity: Some(severity), kind: None });
        }
    }

    fn spike(&mut self, tick: &Tick, names: &Names, activity: Activity, lang: Lang) -> Option<(String, Option<Kind>)> {
        let recent = self
            .last_opened
            .as_ref()
            .filter(|(_, at)| self.clock - at <= tuning().talk.spike_cause_secs)
            .map(|(c, _)| c.clone());
        if self.burst.is_none() {
            let big = tick.rx.max(tick.tx);
            if big <= tuning().talk.spike_bps || self.last_spike.is_some_and(|t| self.clock - t < tuning().talk.spike_every_secs) {
                return None;
            }
            let incoming = tick.rx >= tick.tx;
            self.burst = Some(Burst { incoming, peak: 0, waited: 0, flows: HashMap::new() });
        }

        let burst = self.burst.as_mut()?;
        burst.peak = burst.peak.max(if burst.incoming { tick.rx } else { tick.tx });
        for f in &tick.flows {
            let bytes = if burst.incoming { f.rx } else { f.tx };
            let key = (f.conn.pid, f.conn.remote, f.conn.port);
            burst.flows.entry(key).or_insert(Share { conn: f.conn.clone(), total: 0 }).total += bytes;
        }
        let measured: u64 = burst.flows.values().map(|s| s.total).sum();
        let top = burst.flows.values().max_by_key(|s| s.total).filter(|s| s.total > tuning().talk.spike_bps / 4).cloned();

        let subject = top.as_ref().map(|s| &s.conn).or(recent.as_ref());
        let named = subject.is_none_or(|c| names.get(c.remote).is_some());
        let counted = top.is_some() || !tick.sized;
        crate::trace::log(|| {
            let top = top.as_ref().map(|s| (s.conn.remote, s.total));
            format!("burst waited={} peak={} measured={measured} top={top:?} named={named} counted={counted}", burst.waited, burst.peak)
        });
        if burst.waited < tuning().talk.name_wait_secs && !(named && counted) {
            burst.waited += 1;
            return None;
        }
        let burst = self.burst.take()?;
        self.last_spike = Some(self.clock);
        let salt = self.clock;

        let mut line = match &top {
            Some(share) => {
                // Kernel counts arrive split across seconds, so a single
                // drained second undercounts. When one destination owns the
                // burst, the adapter's peak is its rate.
                let owns = share.total as f64 >= tuning().talk.owner_share * measured as f64;
                let bytes = if owns { burst.peak } else { share.total / u64::from(burst.waited.max(1)) };
                let dest = display(names, share.conn.remote, lang);
                lang.flow(&rate(bytes), burst.incoming, &share.conn.process, &dest, salt)
            }
            None => {
                let mut line = lang.spike(&rate(burst.peak), burst.incoming, salt);
                match &recent {
                    Some(c) => line += &lang.right_after(&c.process, &display(names, c.remote, lang)),
                    None => {
                        if let Some((top, n)) = busiest(&tick.active) {
                            line += &lang.busiest(&top, n);
                        }
                    }
                }
                line
            }
        };
        if activity == Activity::Stuffed {
            line += lang.stuffed();
        }
        Some((line, subject.map(|c| c.kind)))
    }

    fn next_connection(&mut self, names: &Names, pet: &Pet, lang: Lang) -> Option<(String, Kind)> {
        while let Some(pos) = self.pending.iter().position(|p| p.age >= tuning().talk.name_wait_secs || names.get(p.conn.remote).is_some()) {
            let p = self.pending.remove(pos)?;
            let c = &p.conn;
            // Remote sessions get their own lines from the watch.
            if c.incoming() || Remote::from_port(c.port).is_some() {
                continue;
            }
            if self.subjects.get(&(c.process.clone(), c.remote)).is_some_and(|&t| self.clock - t < tuning().talk.subject_quiet_secs) {
                continue;
            }
            if self.findings.iter().any(|(f, _)| f.process() == Some(c.process.as_str()) && f.remote() == Some(c.remote)) {
                continue;
            }
            let name = names.get(c.remote);
            let service = if c.place == Place::Internet { services::identify(&c.process, name.as_deref()) } else { None };
            if service == Some(Service::Private) {
                if self.services_said.get(&Service::Private).is_some_and(|&t| self.clock - t < tuning().talk.service_every_secs) {
                    continue;
                }
                self.services_said.insert(Service::Private, self.clock);
                return Some((lang.private(&c.process, self.clock), c.kind));
            }
            if let Some(host) = name.as_deref().filter(|_| c.place == Place::Internet).map(net::short_domain)
                && pet.is_regular(&host)
                && self.greeted.insert(host.clone())
            {
                let days = pet.regulars.get(&host).map_or(0, |r| r.days);
                self.mentioned.insert((c.process.clone(), c.kind), self.clock);
                return Some((lang.regular(&c.process, &host, days, self.clock), c.kind));
            }
            if let Some(s) = service
                && self.services_said.get(&s).is_none_or(|&t| self.clock - t >= tuning().talk.service_every_secs)
            {
                self.services_said.insert(s, self.clock);
                self.mentioned.insert((c.process.clone(), c.kind), self.clock);
                return Some((lang.service(s, &c.process, &display(names, c.remote, lang), self.clock), c.kind));
            }
            if routine(c, name.as_deref()) {
                continue;
            }
            let key = (c.process.clone(), c.kind);
            if self.mentioned.get(&key).is_some_and(|&t| self.clock - t < tuning().talk.repeat_every_secs) {
                continue;
            }
            self.mentioned.insert(key, self.clock);
            let dest = display(names, c.remote, lang);
            return Some((lang.opened(&c.process, c.kind, &dest, c.place, self.clock), c.kind));
        }
        None
    }
}

fn routine(c: &Conn, name: Option<&str>) -> bool {
    match (c.place, c.kind) {
        _ if net::is_own(c.remote) => true,
        (_, Kind::Dns) => true,
        (Place::Lan, Kind::Ssh | Kind::RemoteDesktop | Kind::WinRm | Kind::Database | Kind::Git) => false,
        (Place::Lan, _) => true,
        (Place::Internet, Kind::Web) => name.is_none(),
        (Place::Internet, Kind::PlainWeb) => name.is_some_and(services::plain_http_by_design),
        _ => false,
    }
}

// This computer's own addresses are this computer, not whatever a hosts file calls them.
fn display(names: &Names, ip: IpAddr, lang: Lang) -> String {
    if net::is_own(ip) {
        return lang.this_pc().into();
    }
    let place = net::place_of(ip).unwrap_or(Place::Internet);
    match names.get(ip) {
        Some(name) if services::by_domain(&name).is_some_and(Service::discreet) => lang.hidden().into(),
        Some(name) => lang.dest(&net::short_domain(&name), place),
        None => lang.dest(&ip.to_string(), place),
    }
}

fn meal(tick: &Tick) -> Option<(String, Kind)> {
    if let Some(f) = tick.flows.iter().filter(|f| f.rx + f.tx > 0).max_by_key(|f| f.rx + f.tx) {
        return Some((f.conn.process.clone(), f.conn.kind));
    }
    let (process, _) = busiest(&tick.active)?;
    let kind = tick.active.iter().find(|c| c.process == process)?.kind;
    Some((process, kind))
}

fn busiest(active: &[Conn]) -> Option<(String, usize)> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for c in active {
        *counts.entry(c.process.as_str()).or_default() += 1;
    }
    counts.into_iter().max_by_key(|&(name, n)| (n, std::cmp::Reverse(name))).map(|(name, n)| (name.to_string(), n))
}

pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    const KB: f64 = 1024.0;
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.0} KB", b / KB)
    }
}

pub fn rate(bytes_per_sec: u64) -> String {
    let b = bytes_per_sec as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", b / (1024.0 * 1024.0))
    } else if b >= 1024.0 {
        format!("{:.0} KB/s", b / 1024.0)
    } else {
        format!("{bytes_per_sec} B/s")
    }
}

fn by_design(f: &Finding, names: &Names) -> bool {
    matches!(f, Finding::Cleartext { what: crate::watch::Cleartext::Http, .. })
        && f.remote().and_then(|ip| names.get(ip)).is_some_and(|name| services::plain_http_by_design(&name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Flow;
    use crate::watch::Exposure;

    const MB: u64 = 1024 * 1024;

    fn conn() -> Conn {
        Conn {
            pid: 7,
            process: "curl".into(),
            from_temp: false,
            local_port: 50000,
            incoming: false,
            remote: "141.95.207.211".parse().unwrap(),
            port: 443,
            kind: Kind::Web,
            place: Place::Internet,
        }
    }

    fn tick(rx: u64, flow_rx: Option<u64>) -> Tick {
        Tick {
            rx,
            tx: 10_000,
            opened: vec![],
            active: vec![conn()],
            flows: flow_rx.map(|b| vec![Flow { conn: conn(), rx: b, tx: 0 }]).unwrap_or_default(),
            sized: true,
            attempts: vec![],
            listeners: None,
        }
    }

    fn vnc() -> Finding {
        Finding::Exposed { process: "tvnserver".into(), port: 5900, what: Exposure::Vnc, again: false }
    }

    #[test]
    fn rates() {
        assert_eq!(rate(512), "512 B/s");
        assert_eq!(rate(4096), "4 KB/s");
        assert_eq!(rate(12 * 1024 * 1024 + 400 * 1024), "12.4 MB/s");
        assert_eq!(size(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    #[test]
    fn measured_burst_names_its_owner_at_the_adapter_rate() {
        let names = Names::known(&[(conn().remote, "proof.ovh.net")]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        // Kernel counts trail the adapter by a second and arrive split.
        let ticks = [tick(80 * MB, None), tick(80 * MB, Some(21 * MB)), tick(80 * MB, Some(40 * MB))];
        let said: Vec<Said> =
            ticks.iter().filter_map(|t| talk.tick(t, &names, &pet, vec![], Context::default(), Lang::En)).collect();
        assert_eq!(said, vec![Said::about(Lang::En.flow("80.0 MB/s", true, "curl", "ovh.net", 2), Some(conn().kind))]);
    }

    #[test]
    fn unmeasured_burst_is_never_pinned_on_a_process() {
        let names = Names::known(&[]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let mut t = tick(80 * MB, None);
        t.sized = false;
        let said = talk.tick(&t, &names, &pet, vec![], Context::default(), Lang::En).expect("burst line");
        assert_eq!(said.text, Lang::En.spike("80.0 MB/s", true, 1) + &Lang::En.busiest("curl", 1));
    }

    #[test]
    fn a_long_feast_gets_a_word_and_the_pace_follows_his_appetite() {
        let names = Names::known(&[(conn().remote, "proof.ovh.net")]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let meal = tick(400 * 1024, Some(400 * 1024));
        let said: Vec<Said> =
            (0..tuning().talk.feast_every_secs + 120).filter_map(|_| talk.tick(&meal, &names, &pet, vec![], Context::default(), Lang::En)).collect();
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said.iter().all(|s| s.text.contains("curl") && s.kind == Some(Kind::Web)), "{said:?}");
        talk.appetite = 2.0 * MB as f64;
        assert_eq!(talk.chatter_every(), 45);
        talk.appetite = 0.0;
        assert_eq!(talk.chatter_every(), 150);
    }

    #[test]
    fn plain_chatter_waits_for_the_line_up_to_be_read() {
        let names = Names::known(&[(conn().remote, "proof.ovh.net")]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let meal = tick(400 * 1024, Some(400 * 1024));
        let first = (0..120).find_map(|_| talk.tick(&meal, &names, &pet, vec![], Context::default(), Lang::En));
        assert!(first.is_some(), "the first word about the meal");
        talk.showing(30);
        (talk.last_chatter, talk.last_feast) = (0, None);
        let quiet: Vec<Said> = (0..29).filter_map(|_| talk.tick(&meal, &names, &pet, vec![], Context::default(), Lang::En)).collect();
        assert!(quiet.is_empty(), "{quiet:?}");
        let again = talk.tick(&meal, &names, &pet, vec![], Context::default(), Lang::En);
        assert!(again.is_some(), "said once the line is read");
        talk.showing(30);
        for _ in 0..tuning().talk.warn_gap_secs {
            assert!(talk.tick(&meal, &names, &pet, vec![], Context::default(), Lang::En).is_none());
        }
        let warned = talk.tick(&meal, &names, &pet, vec![vnc()], Context::default(), Lang::En);
        assert_eq!(warned.and_then(|s| s.severity), Some(Severity::Warn), "a warning cuts in");
    }

    // Docker writes host.docker.internal into the hosts file for this computer's own address.
    #[test]
    fn this_computer_is_never_named_after_a_hosts_file_entry() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        let mut known = vec![(loopback, "kubernetes.docker.internal")];
        let own = crate::tools::own_addresses().into_iter().find(|ip| ip.is_ipv4());
        known.extend(own.map(|ip| (ip, "host.docker.internal")));
        let names = Names::known(&known);
        for (ip, _) in &known {
            assert_eq!(display(&names, *ip, Lang::En), "this pc");
            assert_eq!(display(&names, *ip, Lang::Cs), "tenhle počítač");
            assert!(routine(&Conn { remote: *ip, place: Place::Lan, ..conn() }, Some("host.docker.internal")));
        }
    }

    #[test]
    fn a_note_found_while_he_sleeps_waits_for_him() {
        let names = Names::known(&[]);
        let mut pet = Pet::new(0);
        pet.nap(3600);
        let mut talk = Talk::default();
        let quiet = tick(0, None);
        let share = Finding::Exposed { process: "System".into(), port: 445, what: Exposure::FileShare, again: false };
        assert_eq!(share.severity(), Severity::Note);
        let mut found = vec![share];
        for _ in 0..2 * tuning().talk.note_stale_secs {
            assert!(talk.tick(&quiet, &names, &pet, std::mem::take(&mut found), Context::default(), Lang::En).is_none_or(|s| s.severity.is_none()));
        }
        pet.wake_up();
        let said = (0..30).find_map(|_| talk.tick(&quiet, &names, &pet, vec![], Context::default(), Lang::En).filter(|s| s.severity.is_some()));
        assert!(said.is_some_and(|s| s.text.contains("445")), "the note is said on waking");
    }

    #[test]
    fn a_warning_is_said_during_a_nap() {
        let names = Names::known(&[]);
        let mut pet = Pet::new(0);
        pet.nap(600);
        assert_eq!(pet.activity(), Activity::Sleeping);
        let mut talk = Talk::default();
        let quiet = tick(0, None);
        let mut found = vec![vnc()];
        let said = (0..=tuning().talk.warn_gap_secs)
            .find_map(|_| talk.tick(&quiet, &names, &pet, std::mem::take(&mut found), Context::default(), Lang::En))
            .expect("the warning");
        assert_eq!(said.severity, Some(Severity::Warn));
        pet.wake_up();
        assert!(!pet.napping() && pet.activity() != Activity::Sleeping);
    }

    #[test]
    fn warnings_wait_for_someone_at_the_keyboard_and_are_not_repeated() {
        let names = Names::known(&[]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let quiet = tick(0, None);
        let away = Context { idle_secs: 900, ..Context::default() };
        assert_eq!(talk.tick(&quiet, &names, &pet, vec![vnc()], away, Lang::En), None);
        let back = (0..tuning().talk.warn_gap_secs)
            .find_map(|_| talk.tick(&quiet, &names, &pet, vec![], Context::default(), Lang::En))
            .expect("delivered on return");
        assert_eq!(back.severity, Some(Severity::Warn));
        assert!(back.text.contains("5900"), "{}", back.text);
        for _ in 0..20 {
            let again = talk.tick(&quiet, &names, &pet, vec![vnc()], Context::default(), Lang::En);
            assert!(again.is_none_or(|s| s.severity.is_none()), "same exposure is not repeated");
        }
    }

    #[test]
    fn a_warning_is_never_cut_short_by_another() {
        let names = Names::known(&[]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let quiet = tick(0, None);
        let rdp = Finding::Exposed { process: "svchost".into(), port: 3389, what: Exposure::RemoteDesktop, again: false };
        let mut warned_at = vec![];
        for second in 0..30u64 {
            let found = if second == 0 { vec![vnc(), rdp.clone()] } else { vec![] };
            if let Some(s) = talk.tick(&quiet, &names, &pet, found, Context::default(), Lang::En)
                && s.severity == Some(Severity::Warn)
            {
                warned_at.push(second);
            }
        }
        assert_eq!(warned_at.len(), 2, "{warned_at:?}");
        assert!(warned_at[1] - warned_at[0] >= u64::from(tuning().talk.warn_secs), "{warned_at:?}");
    }

    #[test]
    fn remote_sessions_are_left_to_the_watch() {
        let names = Names::known(&[]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let mut t = tick(0, None);
        let mut c = conn();
        c.port = 22;
        c.kind = Kind::Ssh;
        t.opened = vec![c];
        let mut said = vec![];
        for _ in 0..=tuning().talk.name_wait_secs {
            said.extend(talk.tick(&t, &names, &pet, vec![], Context::default(), Lang::En));
            t.opened.clear();
        }
        assert!(said.is_empty(), "{said:?}");
    }

    #[test]
    fn a_finding_silences_plain_chatter_about_the_same_connection() {
        let ip: IpAddr = "93.184.216.34".parse().unwrap();
        let names = Names::known(&[(ip, "example.com")]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let mut t = tick(0, None);
        let mut c = conn();
        c.process = "powershell".into();
        c.remote = ip;
        t.opened = vec![c];
        let shell = Finding::Lolbin { process: "powershell".into(), remote: ip };
        let mut said = vec![];
        for i in 0..20 {
            let found = if i == 0 { vec![shell.clone()] } else { vec![] };
            said.extend(talk.tick(&t, &names, &pet, found, Context::default(), Lang::En));
            t.opened.clear();
        }
        let about: Vec<_> = said.iter().filter(|s| s.text.contains("example.com")).collect();
        assert_eq!(about.len(), 1, "{said:?}");
    }

    #[test]
    fn regulars_get_a_greeting_once_a_day() {
        let ip: IpAddr = "140.82.121.4".parse().unwrap();
        let names = Names::known(&[(ip, "github.com")]);
        let mut pet = Pet::new(0);
        for day in 1..=3 {
            pet.visit("github.com", day);
        }
        let mut talk = Talk::default();
        let cx = Context { day: 4, ..Context::default() };
        let mut said = vec![];
        for i in 0..40 {
            let mut t = tick(0, None);
            if i % 10 == 0 {
                let mut c = conn();
                c.remote = ip;
                c.process = "git".into();
                c.port = 9418;
                c.kind = Kind::Git;
                t.opened = vec![c];
            }
            said.extend(talk.tick(&t, &names, &pet, vec![], cx, Lang::En));
        }
        let greetings =
            said.iter().map(|s| s.text.to_lowercase()).filter(|text| text.contains("old friend") || text.contains("we're regulars")).count();
        assert_eq!(greetings, 1, "{said:?}");
        assert!(talk.visits.contains(&"github.com".to_string()));
    }

    #[test]
    fn an_answer_to_the_user_is_not_cut_short() {
        let names = Names::known(&[]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let quiet = tick(0, None);
        for _ in 0..5 {
            talk.tick(&quiet, &names, &pet, vec![], Context::default(), Lang::En);
        }
        talk.hold();
        let mut warned_at = None;
        for second in 1..=20u64 {
            let found = if second == 1 { vec![vnc()] } else { vec![] };
            let said = talk.tick(&quiet, &names, &pet, found, Context::default(), Lang::En);
            if said.is_some_and(|s| s.severity == Some(Severity::Warn)) {
                warned_at.get_or_insert(second);
            }
        }
        assert!(warned_at.is_some_and(|s| s >= u64::from(tuning().talk.warn_secs)), "{warned_at:?}");
    }

    #[test]
    fn unheard_notes_still_reach_the_journal() {
        let names = Names::known(&[]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let quiet = tick(0, None);
        let away = Context { idle_secs: 900, ..Context::default() };
        talk.tick(&quiet, &names, &pet, vec![Finding::Tor { process: "tor".into() }], away, Lang::En);
        for _ in 0..=tuning().talk.note_stale_secs {
            talk.tick(&quiet, &names, &pet, vec![], away, Lang::En);
        }
        assert!(
            matches!(talk.journal.as_slice(), [(false, text, false)] if text.contains("tor")),
            "{:?}",
            talk.journal
        );
    }

    #[test]
    fn a_listening_service_is_told_once_reminded_later_and_its_closing_noted() {
        let names = Names::known(&[]);
        let pet = Pet::new(0);
        let mut listening = tick(0, None);
        listening.listeners = Some(vec![]);
        let run = |talk: &mut Talk, found: Vec<Finding>, now: u64| -> Vec<Said> {
            let cx = Context { now, ..Context::default() };
            (0..15).filter_map(|_| talk.tick(&listening, &names, &pet, found.clone(), cx, Lang::En)).collect()
        };
        let mut talk = Talk::default();
        let said = run(&mut talk, vec![vnc()], 1000);
        assert_eq!(said.iter().filter(|s| s.text.contains("5900")).count(), 1, "{said:?}");

        let mut restarted = Talk::remembering(talk.told.clone());
        let said = run(&mut restarted, vec![vnc()], 1000 + 3600);
        assert!(said.iter().all(|s| !s.text.contains("5900")), "not again after a restart: {said:?}");

        let mut next_day = Talk::remembering(talk.told.clone());
        let said = run(&mut next_day, vec![vnc()], 1000 + 25 * 3600);
        let reminders: Vec<_> = said.iter().filter(|s| s.text.contains("5900")).collect();
        assert!(reminders.len() == 1 && reminders[0].text.contains("still"), "{said:?}");

        let said = run(&mut restarted, vec![], 1000 + 3700);
        assert_eq!(said.iter().filter(|s| s.text.contains("stopped listening") || s.text.contains("off the network")).count(), 1, "{said:?}");
        assert!(!restarted.told.contains_key(&vnc().key()), "opening it again is news");
    }

    #[test]
    fn revocation_checks_over_plain_http_are_not_findings() {
        let ip: IpAddr = "192.0.2.10".parse().unwrap();
        let names = Names::known(&[(ip, "ocsp.digicert.com")]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let plain = Finding::Cleartext { process: "msedge".into(), remote: ip, what: crate::watch::Cleartext::Http };
        let said: Vec<Said> = (0..20)
            .filter_map(|i| {
                let found = if i == 0 { vec![plain.clone()] } else { vec![] };
                talk.tick(&tick(0, None), &names, &pet, found, Context::default(), Lang::En)
            })
            .collect();
        assert!(said.iter().all(|s| !s.text.contains("digicert")), "{said:?}");
        assert!(talk.journal.is_empty(), "{:?}", talk.journal);
        let check = Conn { remote: ip, port: 80, kind: Kind::PlainWeb, ..conn() };
        assert!(routine(&check, Some("x1.c.lencr.org")));
        assert!(!routine(&check, Some("example.com")), "other plain http is still news");
    }

    #[test]
    fn fullscreen_hushes_everything() {
        let names = Names::known(&[]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let hushed = Context { hushed: true, ..Context::default() };
        assert_eq!(talk.tick(&tick(80 * MB, None), &names, &pet, vec![vnc()], hushed, Lang::En), None);
    }

    #[test]
    fn private_destinations_are_never_named() {
        let bank: IpAddr = "198.51.100.77".parse().unwrap();
        let names = Names::known(&[(bank, "ib.fio.cz")]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let mut t = tick(0, None);
        let mut c = conn();
        c.remote = bank;
        c.process = "msedge".into();
        t.opened = vec![c];
        let said = talk.tick(&t, &names, &pet, vec![], Context::default(), Lang::Cs).expect("a line");
        assert!(!said.text.contains("fio"), "{}", said.text);
        assert_eq!(display(&names, bank, Lang::En), "somewhere private");
    }

    #[test]
    fn known_services_get_a_take() {
        let signal: IpAddr = "198.51.100.88".parse().unwrap();
        let names = Names::known(&[]);
        let pet = Pet::new(0);
        let mut talk = Talk::default();
        let mut t = tick(0, None);
        let mut c = conn();
        c.remote = signal;
        c.process = "signal".into();
        t.opened = vec![c];
        let mut said = None;
        for _ in 0..=tuning().talk.name_wait_secs {
            said = said.or(talk.tick(&t, &names, &pet, vec![], Context::default(), Lang::En));
            t.opened.clear();
        }
        let said = said.expect("a take");
        assert_eq!(said.kind, Some(conn().kind));
        let text = said.text;
        let said = text.to_lowercase();
        assert!(said.contains("signal"), "{text}");
        assert!(!said.contains("opened"), "a take, not a plain report: {text}");
    }
}
