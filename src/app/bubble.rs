use std::collections::VecDeque;

use crate::clock::{self, stamp};
use crate::lang::{Details, Lang};
use crate::platform::shell::PanelWindow;
use crate::tools::inspect;
use crate::{pet, render, roam, talk, tools, trace};

use super::App;

pub(super) struct ToolPanel {
    window: PanelWindow,
    panel: render::Panel,
    at: (i32, i32),
    tail: Option<render::Tail>,
    showing: Showing,
    busy_text: &'static str,
    wheel: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Showing {
    Report,
    Journal,
    Card,
}

impl App {
    pub(super) fn use_tool(&mut self, tool: tools::Tool) {
        trace::record(|| format!("tool {tool:?} busy={}", self.tool_job.is_some()));
        let lang = self.lang;
        if tool == tools::Tool::Usage {
            let report = self.usage.report(&self.names, self.sized == Some(true), lang);
            self.show_report(report, None);
            return;
        }
        if tool == tools::Tool::CallsHome {
            let report = tools::calls_report(&self.pet.calls, crate::clock::local_clock().1, lang);
            self.show_report(report, None);
            return;
        }
        if self.tool_job.is_some() {
            self.say(self.lang.tool_still_busy().into(), false);
            return;
        }
        self.open_panel(lang.tool_name(tool), None);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(match tool {
                tools::Tool::ShareWifi => tools::share_wifi_report(lang),
                tool => (tools::run(tool, lang), None),
            });
        });
        self.tool_job = Some(rx);
    }

    pub(super) fn inspect(&mut self, path: std::path::PathBuf, more: usize) {
        trace::record(|| format!("inspect busy={}", self.tool_job.is_some()));
        if self.tool_job.is_some() {
            self.say(self.lang.tool_still_busy().into(), false);
            return;
        }
        let lang = self.lang;
        self.open_panel(lang.inspect_title(), None);
        if let Some(p) = &mut self.panel {
            p.busy_text = lang.inspect_busy();
        }
        self.paint_panel();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send((inspect::report(&path, more, lang), None));
        });
        self.tool_job = Some(rx);
    }

    pub(super) fn show_report(&mut self, report: tools::Report, qr: Option<render::Qr>) {
        self.tool_job = None;
        self.open_panel(&report.title, Some(&report.text));
        if let Some(p) = &mut self.panel {
            p.panel.qr = qr;
        }
        if !report.line.is_empty() {
            self.say(report.line, false);
        }
        self.talk.hold();
    }

    fn open_panel(&mut self, title: &str, body: Option<&str>) {
        if !self.ensure_panel() {
            return;
        }
        let busy = self.lang.tool_busy();
        if let Some(p) = &mut self.panel {
            p.panel.set(title, body);
            p.showing = Showing::Report;
            p.busy_text = busy;
        }
        self.paint_panel();
    }

    fn ensure_panel(&mut self) -> bool {
        if self.panel.as_ref().is_none_or(|p| p.panel.scale() != self.scale) {
            self.close_panel();
            let panel = render::Panel::new(self.scale);
            let (w, h) = panel.size();
            trace::log(|| format!("panel window {w}x{h}"));
            let Some(window) = PanelWindow::create(w as i32, h as i32) else { return false };
            let busy_text = self.lang.tool_busy();
            self.panel = Some(ToolPanel { window, panel, at: (i32::MIN, i32::MIN), showing: Showing::Report, tail: None, busy_text, wheel: 0 });
        }
        true
    }

    fn place_panel(&mut self) -> Option<render::Tail> {
        let p = self.panel.as_mut()?;
        let (x, y) = self.shell.pos();
        let (sw, sh) = self.scene.size();
        let r = (x, y, x + sw as i32, y + sh as i32);
        let at_home = if self.grabbed { (x, y) } else { self.pet.pos.unwrap_or((x, y)) };
        let work = roam::home_work_area(&self.desk, at_home, self.scale as i32).map_or(r, |work| (work.left, work.top, work.right, work.bottom));
        let (w, h) = p.panel.size();
        let (at, tail) = render::panel_place(r, self.scene.sprite_y() as i32, (w as i32, h as i32), work, self.scale as i32);
        if at != p.at {
            p.window.place(at.0, at.1);
            p.at = at;
        }
        Some(tail)
    }

    pub(super) fn paint_panel(&mut self) {
        let Some(tail) = self.place_panel() else { return };
        let Some(p) = &mut self.panel else { return };
        p.tail = Some(tail);
        p.panel.scanlines = !self.pet.scanlines_off;
        let voice = self.line.as_ref().map(|l| (l.text.as_str(), l.alert));
        let hint = if p.showing == Showing::Card { self.lang.card_hint() } else { self.lang.panel_hint() };
        if let Some(canvas) = p.panel.frame(tail, voice, hint, p.busy_text) {
            p.window.present(canvas);
        }
    }

    pub(super) fn follow_panel(&mut self) {
        let tail = self.place_panel();
        if self.panel.as_ref().is_some_and(|p| p.tail != tail) {
            self.paint_panel();
        }
    }

    pub(super) fn raise_panel(&self) {
        if let Some(p) = self.panel.as_ref() {
            p.window.raise();
        }
    }

    fn scroll_panel(&mut self, rows: i32) {
        if let Some(p) = &mut self.panel {
            p.panel.scroll(rows);
        }
        self.paint_panel();
    }

    pub(super) fn wheel_panel(&mut self, delta: i32) {
        let Some(p) = &mut self.panel else { return };
        let rows = wheel_rows(&mut p.wheel, delta);
        if rows != 0 {
            self.scroll_panel(-rows);
        }
    }

    pub(super) fn copy_panel(&mut self) {
        let Some((text, private)) = self.panel.as_ref().map(|p| (p.panel.plain(), p.panel.qr.is_some())) else { return };
        let line = match self.shell.copy(&text, private) {
            true => self.lang.copied(),
            false => self.lang.copy_failed(),
        };
        self.say(line.into(), false);
        self.talk.hold();
    }

    pub(super) fn relabel_panel(&mut self) {
        match self.panel.as_ref().map(|p| p.showing) {
            Some(Showing::Journal) => {
                self.refresh_journal();
                self.paint_panel();
            }
            Some(Showing::Card) => {
                self.refresh_card();
                self.paint_panel();
            }
            Some(Showing::Report) => self.close_panel(),
            None => {}
        }
    }

    pub(super) fn rescale_panel(&mut self) {
        match self.panel.as_ref().map(|p| p.showing) {
            Some(Showing::Journal) => {
                self.close_panel();
                self.open_journal();
            }
            Some(Showing::Card) => {
                self.close_panel();
                self.open_card();
            }
            Some(Showing::Report) => self.close_panel(),
            None => {}
        }
    }

    pub(super) fn open_card(&mut self) {
        self.tool_job = None;
        if !self.ensure_panel() {
            return;
        }
        if let Some(p) = &mut self.panel {
            p.showing = Showing::Card;
        }
        self.refresh_card();
        self.paint_panel();
    }

    pub(super) fn refresh_card(&mut self) {
        if !self.panel.as_ref().is_some_and(|p| p.showing == Showing::Card) {
            return;
        }
        let details = Details {
            satiety: self.pet.satiety,
            mood: self.pet.mood,
            neglect: self.pet.neglect,
            stage: self.pet.stage(),
            days_to_grow: self.pet.days_to_grow(),
            age_days: clock::unix_now().saturating_sub(self.pet.born) / 86_400,
            today: (talk::size(self.pet.today.rx), talk::size(self.pet.today.tx)),
            total: (talk::size(self.pet.lifetime_rx), talk::size(self.pet.lifetime_tx)),
            regulars: self.pet.regular_count(),
            known: self.pet.known_processes.len(),
            open: self.talk.exposed(),
            muted_until: self.muted_until(),
        };
        let sheet = self.lang.sheet(&details);
        if let (Some(p), Some(pose)) = (&mut self.panel, self.portrait) {
            p.panel.set_card(pose, sheet);
        }
    }

    pub(super) fn close_panel(&mut self) {
        self.tool_job = None;
        if self.panel.take().is_some() {
            trace::log(|| "panel closed".to_string());
        }
    }

    pub(super) fn open_journal(&mut self) {
        self.tool_job = None;
        if !self.ensure_panel() {
            return;
        }
        let (title, rows) = (self.lang.journal_title(), journal_rows(&self.pet.journal, self.lang));
        if let Some(p) = &mut self.panel {
            p.panel.set_rows(title, rows, true);
            p.showing = Showing::Journal;
        }
        self.paint_panel();
    }

    pub(super) fn refresh_journal(&mut self) {
        if !self.panel.as_ref().is_some_and(|p| p.showing == Showing::Journal) {
            return;
        }
        let (title, rows) = (self.lang.journal_title(), journal_rows(&self.pet.journal, self.lang));
        if let Some(p) = &mut self.panel {
            p.panel.set_rows(title, rows, false);
        }
    }
}

// A wheel notch is 120; a touchpad sends smaller steps.
fn wheel_rows(carried: &mut i32, delta: i32) -> i32 {
    let step = 120 / render::PANEL_WHEEL_ROWS;
    *carried += delta;
    let rows = *carried / step;
    *carried -= rows * step;
    rows
}

fn journal_rows(entries: &VecDeque<pet::Entry>, lang: Lang) -> Vec<render::Row> {
    use render::{Row, Tone};
    if entries.is_empty() {
        return vec![Row { text: lang.journal_empty().into(), tone: Tone::Quiet, indent: 0 }];
    }
    entries
        .iter()
        .rev()
        .map(|e| {
            let stamp = stamp(e.at);
            let mark = if e.warn { "◆" } else { "○" };
            let unsaid = if e.said { "" } else { lang.journal_unsaid() };
            let tone = match (e.warn, e.said) {
                (true, _) => Tone::Warn,
                (false, true) => Tone::Plain,
                (false, false) => Tone::Quiet,
            };
            Row { indent: stamp.chars().count() + 3, text: format!("{stamp} {mark} {}{unsaid}", e.text), tone }
        })
        .collect()
}


#[cfg(test)]
mod tests {
    use super::*;

    fn entry(at: u64, text: &str, warn: bool, said: bool) -> pet::Entry {
        pet::Entry { at, warn, text: text.to_string(), said }
    }

    #[test]
    fn an_empty_journal_says_so_and_a_full_one_reads_newest_first() {
        let rows = journal_rows(&VecDeque::new(), Lang::Cs);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tone, render::Tone::Quiet);
        assert_eq!(rows[0].text, Lang::Cs.journal_empty());

        let entries: VecDeque<pet::Entry> =
            [entry(1_700_000_000, "první", false, true), entry(1_700_003_600, "druhá", false, true)].into_iter().collect();
        let rows = journal_rows(&entries, Lang::Cs);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].text.contains("druhá"), "{}", rows[0].text);
        assert!(rows[1].text.contains("první"), "{}", rows[1].text);
    }

    #[test]
    fn the_journal_shows_what_was_a_warning_and_what_was_never_said() {
        let entries: VecDeque<pet::Entry> = [
            entry(1_700_000_000, "klid", false, true),
            entry(1_700_000_001, "nikdo neslyšel", false, false),
            entry(1_700_000_002, "pozor", true, true),
        ]
        .into_iter()
        .collect();
        let rows = journal_rows(&entries, Lang::Cs);
        let row = |text: &str| rows.iter().find(|r| r.text.contains(text)).expect(text);
        assert_eq!(row("pozor").tone, render::Tone::Warn);
        assert!(row("pozor").text.contains('◆'));
        assert_eq!(row("klid").tone, render::Tone::Plain);
        assert!(row("klid").text.contains('○'));
        let unheard = row("nikdo neslyšel");
        assert_eq!(unheard.tone, render::Tone::Quiet);
        assert!(unheard.text.ends_with(Lang::Cs.journal_unsaid()), "{}", unheard.text);
        assert_eq!(unheard.indent, stamp(1_700_000_001).chars().count() + 3);
    }

    #[test]
    fn the_wheel_scrolls_by_notches_and_a_touchpad_by_what_adds_up() {
        let step = 120 / render::PANEL_WHEEL_ROWS;
        let mut carried = 0;
        assert_eq!(wheel_rows(&mut carried, 120), render::PANEL_WHEEL_ROWS);
        assert_eq!(carried, 0, "a whole notch leaves nothing over");
        assert_eq!(wheel_rows(&mut carried, step / 3), 0, "too small a step on its own");
        assert_eq!(wheel_rows(&mut carried, step / 3), 0);
        assert_eq!(wheel_rows(&mut carried, step), 1, "and now they have come to a row");
        let mut back = 0;
        assert_eq!(wheel_rows(&mut back, -120), -(render::PANEL_WHEEL_ROWS), "the other way too");
    }
}
