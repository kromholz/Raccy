use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::path::{Path, PathBuf};


use serde::{Deserialize, Serialize};

use crate::net::{Kind, Tick};
use crate::tuning::tuning;

const HOSTS_KEPT: usize = 300;
const CALL_NAMES_KEPT: usize = 50;
const SUSPEND_GAP_SECS: u64 = 120;
// The state file can still be locked at logon; read it again for ten seconds.
const READ_TRIES: u32 = 20;
const READ_RETRY_MS: u64 = 500;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Regular {
    pub days: u32,
    pub last_day: u32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub at: u64,
    pub warn: bool,
    pub text: String,
    pub said: bool,
}

const JOURNAL_KEPT: usize = 100;
const JOURNAL_REPEAT_SECS: u64 = 86_400;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Day {
    pub day: u32,
    pub rx: u64,
    pub tx: u64,
    #[serde(default)]
    pub fed_secs: u32,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Calls {
    pub day: u32,
    pub by_process: HashMap<String, HashMap<String, u32>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Sleeping,
    Stuffed,
    Eating,
    Starving,
    Hungry,
    Bored,
    Content,
}

#[derive(Serialize, Deserialize)]
pub struct Pet {
    pub satiety: f32,
    pub mood: f32,
    pub lifetime_rx: u64,
    pub lifetime_tx: u64,
    pub born: u64,
    saved: u64,
    #[serde(default)]
    pub pos: Option<(i32, i32)>,
    #[serde(default)]
    pub lang: Option<crate::lang::Lang>,
    #[serde(default)]
    pub known_processes: HashSet<String>,
    #[serde(default)]
    pub watching_since: Option<u64>,
    #[serde(default)]
    pub watched_secs: u64,
    #[serde(default)]
    pub regulars: HashMap<String, Regular>,
    #[serde(default)]
    pub today: Day,
    #[serde(default)]
    pub introduced: bool,
    #[serde(default)]
    pub journal: VecDeque<Entry>,
    #[serde(default)]
    pub muted_until: Option<u64>,
    #[serde(default)]
    pub stage_seen: Option<u8>,
    #[serde(default)]
    pub neglect: f32,
    #[serde(default)]
    pub fed_days: u32,
    #[serde(default)]
    pub told: HashMap<String, u64>,
    #[serde(default)]
    pub scanlines_off: bool,
    #[serde(default)]
    pub networks: HashMap<String, crate::watch::lan::Network>,
    #[serde(default)]
    pub calls: Calls,
    #[serde(default)]
    pub autoruns: HashMap<String, HashSet<String>>,
    #[serde(default)]
    pub redirects: crate::watch::redirects::Known,
    #[serde(skip)]
    clock: u64,
    #[serde(skip)]
    quiet: u32,
    #[serde(skip)]
    stuffed_for: u32,
    #[serde(skip)]
    nap_left: u32,
    #[serde(skip)]
    ate: bool,
    #[serde(skip)]
    kinds_seen: HashMap<Kind, u64>,
    #[serde(skip)]
    hosts_seen: HashMap<IpAddr, u64>,
    #[serde(skip)]
    last_fed: u64,
    #[serde(skip)]
    read_failed: bool,
}

impl Pet {
    pub fn new(now: u64) -> Pet {
        Pet {
            satiety: 70.0,
            mood: 70.0,
            lifetime_rx: 0,
            lifetime_tx: 0,
            born: now,
            saved: now,
            pos: None,
            lang: None,
            known_processes: HashSet::new(),
            autoruns: HashMap::new(),
            redirects: Default::default(),
            watching_since: Some(now),
            watched_secs: 0,
            regulars: HashMap::new(),
            today: Day::default(),
            introduced: false,
            journal: VecDeque::new(),
            muted_until: None,
            stage_seen: None,
            neglect: 0.0,
            fed_days: 0,
            told: HashMap::new(),
            scanlines_off: false,
            networks: HashMap::new(),
            calls: Calls::default(),
            clock: 0,
            quiet: 0,
            stuffed_for: 0,
            nap_left: 0,
            ate: false,
            kinds_seen: HashMap::new(),
            hosts_seen: HashMap::new(),
            last_fed: 0,
            read_failed: false,
        }
    }

    pub fn feed(&mut self, tick: &Tick) {
        let now = crate::clock::unix_now();
        // The computer slept with the app running.
        if self.last_fed > 0 && now.saturating_sub(self.last_fed) > SUSPEND_GAP_SECS {
            self.saved = self.last_fed;
            self.wake(now);
        }
        self.last_fed = now;
        self.clock += 1;
        self.watched_secs += 1;
        let bytes = tick.rx + tick.tx;
        self.lifetime_rx += tick.rx;
        self.lifetime_tx += tick.tx;

        let asleep = self.quiet >= tuning().pet.asleep_after_secs || self.nap_left > 0;
        self.nap_left = self.nap_left.saturating_sub(1);
        self.satiety -= if asleep { tuning().pet.hunger_asleep_per_sec() } else { tuning().pet.hunger_per_sec() };
        if bytes > tuning().pet.food_line_bps {
            self.satiety += tuning().pet.food_per_doubling * (bytes as f32 / tuning().pet.food_line_bps as f32).log2();
        }
        self.ate = bytes > tuning().pet.quiet_line_bps;
        if self.ate {
            self.quiet = 0;
        } else {
            self.quiet = self.quiet.saturating_add(1);
        }
        if bytes > tuning().pet.stuffed_bps && self.satiety > 90.0 {
            self.stuffed_for = 30;
        }
        self.stuffed_for = self.stuffed_for.saturating_sub(1);

        self.mood -= tuning().pet.boredom_per_sec();
        if self.satiety < 25.0 {
            self.mood -= tuning().pet.boredom_per_sec();
        }
        for conn in &tick.opened {
            let clock = self.clock;
            let fresh = |seen: Option<&u64>| seen.is_none_or(|&t| clock - t > tuning().pet.novelty_secs);
            if fresh(self.kinds_seen.get(&conn.kind)) {
                self.mood += 8.0;
            }
            if fresh(self.hosts_seen.get(&conn.remote)) {
                self.mood += 1.0;
            }
            self.kinds_seen.insert(conn.kind, clock);
            self.hosts_seen.insert(conn.remote, clock);
        }
        self.kinds_seen.retain(|_, &mut t| clock_gap(self.clock, t) <= tuning().pet.novelty_secs);
        self.hosts_seen.retain(|_, &mut t| clock_gap(self.clock, t) <= tuning().pet.novelty_secs);

        if self.satiety < 10.0 {
            self.neglect = (self.neglect + tuning().pet.neglect_per_starving_sec()).min(1.0);
        } else if self.satiety > 60.0 {
            self.neglect = (self.neglect - tuning().pet.mending_per_fed_sec()).max(0.0);
        }
        self.satiety = self.satiety.clamp(0.0, 100.0);
        self.mood = self.mood.clamp(0.0, 100.0);
    }

    pub fn activity(&self) -> Activity {
        if self.quiet >= tuning().pet.asleep_after_secs || self.nap_left > 0 {
            Activity::Sleeping
        } else if self.stuffed_for > 0 {
            Activity::Stuffed
        } else if self.satiety < 10.0 {
            Activity::Starving
        } else if self.ate {
            Activity::Eating
        } else if self.satiety < 30.0 {
            Activity::Hungry
        } else if self.mood < 25.0 {
            Activity::Bored
        } else {
            Activity::Content
        }
    }

    pub fn call_home(&mut self, day: u32, process: &str, name: &str) {
        if self.calls.day != day {
            self.calls = Calls { day, by_process: HashMap::new() };
        }
        let names = self.calls.by_process.entry(process.to_string()).or_default();
        if names.len() < CALL_NAMES_KEPT || names.contains_key(name) {
            *names.entry(name.to_string()).or_default() += 1;
        }
    }

    pub fn visit(&mut self, host: &str, day: u32) {
        let seen = self.regulars.entry(host.to_string()).or_default();
        if seen.last_day != day {
            seen.days += 1;
            seen.last_day = day;
        }
        if self.regulars.len() > HOSTS_KEPT {
            let stalest =
                self.regulars.iter().filter(|(h, _)| h.as_str() != host).min_by_key(|(_, r)| (r.last_day, r.days)).map(|(h, _)| h.clone());
            if let Some(host) = stalest {
                self.regulars.remove(&host);
            }
        }
    }

    pub fn is_regular(&self, host: &str) -> bool {
        self.regulars.get(host).is_some_and(|r| r.days >= tuning().pet.regular_days)
    }

    pub fn regular_count(&self) -> usize {
        self.regulars.values().filter(|r| r.days >= tuning().pet.regular_days).count()
    }

    pub fn count_day(&mut self, day: u32, rx: u64, tx: u64) {
        // A new day starts the counts; a clock moved back a little does not.
        if day > self.today.day || self.today.day > day + 1 {
            self.today = Day { day, ..Day::default() };
        }
        self.today.rx += rx;
        self.today.tx += tx;
        if self.quiet < tuning().pet.asleep_after_secs && self.satiety >= tuning().pet.fed_satiety {
            self.today.fed_secs += 1;
            if self.today.fed_secs == tuning().pet.fed_day_secs {
                self.fed_days += 1;
            }
        }
    }

    pub fn stage(&self) -> u8 {
        tuning().pet.stage_days.iter().filter(|&&days| self.fed_days >= days).count() as u8
    }

    pub fn days_to_grow(&self) -> Option<u32> {
        tuning().pet.stage_days.iter().find(|&&days| self.fed_days < days).map(|days| days - self.fed_days)
    }

    pub fn level_up(&mut self) -> Option<u8> {
        let stage = self.stage();
        match self.stage_seen {
            Some(seen) if stage > seen => {
                self.stage_seen = Some(stage);
                Some(stage)
            }
            // Growth used to follow bytes and ran far ahead; step back quietly.
            Some(seen) if stage < seen => {
                self.stage_seen = Some(stage);
                None
            }
            Some(_) => None,
            None => {
                self.stage_seen = Some(stage);
                None
            }
        }
    }

    pub fn note(&mut self, at: u64, warn: bool, text: String, said: bool) {
        self.journal.retain(|e| !(e.text == text && at.saturating_sub(e.at) < JOURNAL_REPEAT_SECS));
        self.journal.push_back(Entry { at, warn, text, said });
        while self.journal.len() > JOURNAL_KEPT {
            self.journal.pop_front();
        }
    }

    pub fn pet(&mut self) {
        self.mood = (self.mood + 2.0).min(100.0);
        self.nap_left = 0;
    }

    pub fn nap(&mut self, secs: u32) {
        self.nap_left = secs;
    }

    pub fn napping(&self) -> bool {
        self.nap_left > 0
    }

    pub fn wake_up(&mut self) {
        self.nap_left = 0;
    }

    #[cfg(feature = "trace")]
    pub fn stuff_for_a_test(&mut self) {
        self.stuffed_for = 30;
    }

    #[cfg(feature = "trace")]
    pub fn feed_for_a_test(&mut self) {
        (self.satiety, self.mood) = (70.0, 70.0);
    }

    pub fn learning(&self) -> bool {
        self.watched_secs < tuning().pet.learning_secs
    }

    fn wake(&mut self, now: u64) {
        let away = now.saturating_sub(self.saved) as f32;
        if self.satiety > tuning().pet.offline_satiety_floor {
            self.satiety = (self.satiety - away * tuning().pet.hunger_asleep_per_sec()).max(tuning().pet.offline_satiety_floor);
        }
        if self.mood > tuning().pet.offline_mood_floor {
            self.mood = (self.mood - away * tuning().pet.boredom_per_sec()).max(tuning().pet.offline_mood_floor);
        }
        self.saved = now;
    }

    pub fn load() -> Pet {
        let now = crate::clock::unix_now();
        let mut read = std::fs::read(state_path());
        for _ in 0..READ_TRIES {
            if !unreadable(&read) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(READ_RETRY_MS));
            read = std::fs::read(state_path());
        }
        let read_failed = unreadable(&read);
        let bytes = read.ok();
        let loaded = bytes.as_ref().and_then(|bytes| serde_json::from_slice::<Pet>(bytes).ok());
        if bytes.is_some() && loaded.is_none() {
            let _ = std::fs::rename(state_path(), state_path().with_extension("json.bad"));
        }
        match loaded {
            Some(mut pet) => {
                pet.watching_since.get_or_insert(now);
                // From before watching was counted: a day on the calendar counted.
                if pet.watched_secs == 0 && pet.watching_since.is_some_and(|t| now.saturating_sub(t) >= 24 * 3600) {
                    pet.watched_secs = tuning().pet.learning_secs;
                }
                crate::watch::lan::carry_over(&mut pet.networks, now);
                // A clock that ran ahead left times in the future.
                for t in pet.told.values_mut() {
                    *t = (*t).min(now);
                }
                // Journals from before repeated themselves: noting each line
                // again keeps only the latest of a day.
                for e in Vec::from(std::mem::take(&mut pet.journal)) {
                    pet.note(e.at, e.warn, e.text, e.said);
                }
                pet.wake(now);
                pet
            }
            None => Pet { read_failed, ..Pet::new(now) },
        }
    }

    // A state file that could not be read holds a pet this one is not: the
    // blank one that took his place must never be written over him. Only the
    // file being gone frees the save, which is why the file reading again is
    // no help at all: that is the pet, still in there, still unread.
    fn may_save(&mut self, path: &Path) -> bool {
        if self.read_failed {
            self.read_failed = !matches!(std::fs::read(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
        }
        !self.read_failed
    }

    pub fn save(&mut self) {
        let path = state_path();
        if !self.may_save(&path) {
            return;
        }
        self.saved = crate::clock::unix_now();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            // Write then rename, so a crash mid-write never eats the pet.
            let tmp = path.with_extension("json.tmp");
            let written = std::fs::File::create(&tmp).and_then(|mut file| {
                use std::io::Write;
                file.write_all(&json)?;
                // On disk before the rename, so a power cut leaves the old file whole.
                file.sync_all()
            });
            if written.is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        }
    }
}

fn clock_gap(now: u64, then: u64) -> u64 {
    now.saturating_sub(then)
}

// A file that is not there is not a file that could not be read.
fn unreadable(read: &std::io::Result<Vec<u8>>) -> bool {
    matches!(read, Err(e) if e.kind() != std::io::ErrorKind::NotFound)
}

fn state_path() -> PathBuf {
    crate::platform::host::app_dir().join("state.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{Conn, Place};

    #[test]
    fn a_full_list_of_hosts_still_takes_a_new_one() {
        let mut pet = Pet::new(0);
        for i in 0..HOSTS_KEPT {
            pet.visit(&format!("old{i}.example"), 1);
            pet.visit(&format!("old{i}.example"), 2);
        }
        pet.visit("new.example", 10);
        assert!(pet.regulars.contains_key("new.example"), "the host just visited stays");
        assert_eq!(pet.regulars.len(), HOSTS_KEPT);
    }

    fn tick(bytes: u64, opened: Vec<Conn>) -> Tick {
        Tick { rx: bytes, tx: 0, opened, active: Vec::new(), flows: Vec::new(), sized: false, attempts: Vec::new(), listeners: None }
    }

    fn conn(kind: Kind, last_octet: u8) -> Conn {
        Conn {
            pid: 1,
            process: "test".into(),
            from_temp: false,
            local_port: 50000,
            incoming: false,
            remote: IpAddr::from([1, 1, 1, last_octet]),
            port: 443,
            kind,
            place: Place::Internet,
        }
    }

    #[test]
    fn quiet_wire_starves_slowly_and_sleeps() {
        let mut pet = Pet::new(0);
        for _ in 0..3600 {
            pet.feed(&tick(0, vec![]));
        }
        // Three awake minutes, then hibernating hunger.
        let expected = 70.0 - 180.0 * tuning().pet.hunger_per_sec() - 3420.0 * tuning().pet.hunger_asleep_per_sec();
        assert!((pet.satiety - expected).abs() < 0.05, "{} vs {expected}", pet.satiety);
        assert_eq!(pet.activity(), Activity::Sleeping);
    }

    #[test]
    fn traffic_feeds_and_wakes() {
        let mut pet = Pet::new(0);
        pet.satiety = 50.0;
        for _ in 0..200 {
            pet.feed(&tick(0, vec![]));
        }
        assert_eq!(pet.activity(), Activity::Sleeping);
        for _ in 0..60 {
            pet.feed(&tick(1024 * 1024, vec![]));
        }
        assert!(pet.satiety > 50.0);
        assert_eq!(pet.activity(), Activity::Eating);
    }

    #[test]
    fn crumbs_are_not_food() {
        let mut pet = Pet::new(0);
        let before = pet.satiety;
        pet.feed(&tick(tuning().pet.food_line_bps, vec![]));
        assert!(pet.satiety < before);
    }

    #[test]
    fn variety_beats_repetition() {
        let mut pet = Pet::new(0);
        pet.mood = 50.0;
        pet.feed(&tick(0, vec![conn(Kind::Web, 1)]));
        let after_first = pet.mood;
        pet.feed(&tick(0, vec![conn(Kind::Web, 1)]));
        assert!(pet.mood < after_first, "same host and kind again is not a treat");
        pet.feed(&tick(0, vec![conn(Kind::Ssh, 2)]));
        assert!(pet.mood > after_first + 5.0);
    }

    #[test]
    fn calls_home_are_tallied_for_the_day() {
        let mut pet = Pet::new(0);
        pet.call_home(10, "msedge", "browser.events.data.msn.com");
        pet.call_home(10, "msedge", "browser.events.data.msn.com");
        pet.call_home(10, "", "v10.events.data.microsoft.com");
        assert_eq!(pet.calls.by_process["msedge"]["browser.events.data.msn.com"], 2);
        pet.call_home(11, "steam", "crash.steampowered.com");
        assert_eq!(pet.calls.by_process.len(), 1, "a new day starts over");
    }

    #[test]
    fn regulars_need_three_different_days() {
        let mut pet = Pet::new(0);
        pet.visit("github.com", 10);
        pet.visit("github.com", 10);
        pet.visit("github.com", 11);
        assert!(!pet.is_regular("github.com"));
        pet.visit("github.com", 12);
        assert!(pet.is_regular("github.com"));
        assert_eq!(pet.regular_count(), 1);
    }

    #[test]
    fn a_quiet_night_costs_about_five_percent() {
        let mut pet = Pet::new(0);
        for _ in 0..8 * 3600 {
            pet.feed(&tick(700, vec![]));
        }
        let lost = 70.0 - pet.satiety;
        assert!((4.0..7.0).contains(&lost), "lost {lost}");
    }

    const ACTIVE: [u64; 20] = [
        1700, 300, 2048, 380 * 1024, 900, 1700, 30_000, 1700, 500, 1024 * 1024,
        1700, 300, 2048, 700, 1700, 12_000, 1024 * 1024, 1700, 500, 1700,
    ];

    #[test]
    fn an_hour_of_active_use_feeds_him_a_little() {
        let mut pet = Pet::new(0);
        pet.satiety = 40.0;
        for second in 0..3600 {
            pet.feed(&tick(ACTIVE[second % ACTIVE.len()], vec![]));
        }
        let gained = pet.satiety - 40.0;
        assert!((2.0..10.0).contains(&gained), "gained {gained}");
    }

    #[test]
    fn the_background_trickle_is_not_food() {
        let mut pet = Pet::new(0);
        pet.satiety = 40.0;
        let trickle = ACTIVE.map(|b| b.min(30_000));
        for second in 0..3600 {
            pet.feed(&tick(trickle[second % trickle.len()], vec![]));
        }
        let lost = 40.0 - pet.satiety;
        assert!((12.0..18.0).contains(&lost), "lost {lost}");
    }

    #[test]
    fn the_same_note_within_a_day_is_one_entry() {
        let mut pet = Pet::new(0);
        pet.note(100, true, "vnc open".into(), true);
        pet.note(200, false, "other".into(), true);
        pet.note(300, true, "vnc open".into(), false);
        let entries: Vec<_> = pet.journal.iter().map(|e| (e.at, e.text.as_str())).collect();
        assert_eq!(entries, vec![(200, "other"), (300, "vnc open")]);
        pet.note(300 + JOURNAL_REPEAT_SECS, true, "vnc open".into(), true);
        assert_eq!(pet.journal.len(), 3, "a day later it is news again");
    }

    #[test]
    fn the_journal_keeps_the_last_hundred() {
        let mut pet = Pet::new(0);
        for i in 0..150 {
            pet.note(i, false, format!("line {i}"), true);
        }
        assert_eq!(pet.journal.len(), JOURNAL_KEPT);
        assert_eq!(pet.journal.front().map(|e| e.text.as_str()), Some("line 50"));
    }

    #[test]
    fn stages_come_with_well_fed_days_and_are_announced_once() {
        let mut pet = Pet::new(0);
        assert_eq!(pet.level_up(), None, "the first look only remembers");
        let mut day = 0;
        let mut live_a_day = |pet: &mut Pet, satiety: f32| {
            day += 1;
            for _ in 0..tuning().pet.fed_day_secs {
                pet.satiety = satiety;
                pet.count_day(day, 0, 0);
            }
        };
        live_a_day(&mut pet, 80.0);
        live_a_day(&mut pet, 80.0);
        live_a_day(&mut pet, 30.0);
        assert_eq!(pet.fed_days, 2, "a hungry day does not count");
        assert_eq!(pet.days_to_grow(), Some(1));
        live_a_day(&mut pet, 80.0);
        assert_eq!(pet.level_up(), Some(1));
        assert_eq!(pet.level_up(), None);
        pet.fed_days = 60;
        assert_eq!(pet.level_up(), Some(3));
        assert_eq!(pet.days_to_grow(), None);
        pet.fed_days = 0;
        assert_eq!(pet.level_up(), None, "stepping back is quiet");
        assert_eq!(pet.stage_seen, Some(0));
    }

    #[test]
    fn a_starving_day_tears_an_ear_and_good_days_mend_it() {
        let mut pet = Pet::new(0);
        pet.satiety = 5.0;
        for _ in 0..24 * 3600 {
            pet.feed(&tick(0, vec![]));
        }
        assert!(pet.neglect > 0.99, "{}", pet.neglect);
        pet.satiety = 90.0;
        for _ in 0..2 * 86_400 + 60 {
            pet.feed(&tick(5 * 1024 * 1024, vec![]));
        }
        assert!(pet.neglect < 0.01, "{}", pet.neglect);
    }

    #[test]
    fn a_pet_that_could_not_be_read_is_never_written_over() {
        let mut pet = Pet::new(0);
        pet.read_failed = true;
        // A directory stands in for a file that will not read. It has to be
        // one made here: a path ending in a separator is not found on Windows
        // rather than refused, which is a different answer.
        let locked = std::env::temp_dir().join(format!("raccy-unreadable-{}", std::process::id()));
        std::fs::create_dir_all(&locked).expect("a directory to stand in for a locked file");
        assert!(!pet.may_save(&locked), "a file that could not be read is not written over");
        assert!(pet.read_failed);

        // The one that matters: whatever held the file has let go, and it
        // reads again. That is the pet who was there all along, and this
        // blank one must not land on top of him.
        let readable = locked.join("state.json");
        std::fs::write(&readable, b"{}").expect("a state file that reads");
        assert!(!pet.may_save(&readable), "a state file that reads again is a pet, not a free save");
        assert!(pet.read_failed);

        // Gone is another matter: there is nobody left to write over.
        assert!(pet.may_save(&locked.join("no-such-state.json")), "gone is not unreadable");
        assert!(!pet.read_failed, "and the next save no longer asks");
        std::fs::remove_file(&readable).ok();
        std::fs::remove_dir(&locked).ok();
    }

    #[test]
    fn closing_the_app_never_kills() {
        let mut pet = Pet::new(0);
        pet.wake(30 * 24 * 3600);
        assert_eq!(pet.satiety, tuning().pet.offline_satiety_floor);
        assert_eq!(pet.mood, tuning().pet.offline_mood_floor);
    }
}
