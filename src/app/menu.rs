use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::clock::{self, next_morning};
use crate::platform::{autostart, system};
use crate::render::MenuItem;
use crate::{tools, trace};

use super::{App, with_app};

// Creating or removing the autostart task waits on a UAC prompt.
static AUTOSTART_BUSY: AtomicBool = AtomicBool::new(false);
// 0 not looked at yet, 1 off, 2 on; looking asks schtasks, too slow to do as the menu opens.
static AUTOSTART_ON: AtomicU8 = AtomicU8::new(0);

pub(super) fn refresh_autostart() {
    std::thread::spawn(|| AUTOSTART_ON.store(if autostart::enabled() { 2 } else { 1 }, Ordering::SeqCst));
}

const MENU_CHARACTER: usize = 1;
const MENU_LANGUAGE: usize = 2;
const MENU_AUTOSTART: usize = 3;
const MENU_ADMIN: usize = 4;
const MENU_QUIT: usize = 5;
const MENU_JOURNAL: usize = 6;
const MENU_MUTE_HOUR: usize = 7;
const MENU_MUTE_MORNING: usize = 8;
const MENU_UNMUTE: usize = 9;
const MENU_SCANLINES: usize = 10;
const MENU_UPDATE: usize = 11;
const MENU_TOOL_FIRST: usize = 20;
const MENU_TOOL_LAST: usize = MENU_TOOL_FIRST + tools::Tool::ALL.len() - 1;

pub(super) fn open() {
    let (mut shell, mut items, mut scale) = (None, Vec::new(), 4);
    with_app(|app| (shell, items, scale) = (Some(app.shell), app.menu_items(), app.scale));
    // The menu runs its own message loop, so no borrow may be held here.
    if let Some(chosen) = shell.and_then(|shell| shell.menu(&items, scale)) {
        command(chosen);
    }
}

pub(super) fn command(chosen: usize) {
    trace::record(|| format!("menu chose {chosen}"));
    match chosen {
        MENU_CHARACTER => with_app(App::open_card),
        MENU_JOURNAL => with_app(App::open_journal),
        id @ MENU_TOOL_FIRST..=MENU_TOOL_LAST => {
            with_app(|app| app.use_tool(tools::Tool::ALL[id - MENU_TOOL_FIRST]));
        }
        MENU_MUTE_HOUR => with_app(|app| app.mute(clock::unix_now() + 3600)),
        MENU_MUTE_MORNING => with_app(|app| app.mute(next_morning())),
        MENU_UNMUTE => with_app(App::unmute),
        MENU_LANGUAGE => with_app(App::switch_language),
        MENU_SCANLINES => with_app(App::toggle_scanlines),
        MENU_AUTOSTART => {
            if AUTOSTART_BUSY.swap(true, Ordering::SeqCst) {
                return;
            }
            let mut shell = None;
            with_app(|app| shell = Some(app.shell));
            std::thread::spawn(move || {
                let done = autostart::set(!autostart::enabled());
                AUTOSTART_BUSY.store(false, Ordering::SeqCst);
                let on = autostart::enabled();
                AUTOSTART_ON.store(if on { 2 } else { 1 }, Ordering::SeqCst);
                if let Some(shell) = shell {
                    shell.post_autostart_done(on, done);
                }
            });
        }
        // The package asks for administrator itself, and stops him on its way
        // in, so nothing of his may be held while it runs. How it went is
        // picked up by the frame after: a thread of his own cannot reach the
        // app, whose state belongs to the thread that draws him.
        MENU_UPDATE => {
            if let Some(release) = crate::update::waiting() {
                crate::update::take_in_the_background(release);
            }
        }
        MENU_ADMIN => {
            let mut shell = None;
            with_app(|app| shell = Some(app.shell));
            // The consent prompt blocks until it is answered, and an event that
            // arrives while the app is borrowed is dropped, the one that saves included.
            if let Some(shell) = shell
                && system::restart_elevated(shell.id())
            {
                with_app(|app| {
                    app.pet.save();
                    app.shell.quit();
                });
            }
        }
        MENU_QUIT => with_app(|app| app.shell.quit()),
        _ => {}
    }
}

impl App {
    fn menu_items(&self) -> Vec<MenuItem> {
        let lang = self.lang;
        let item = |id: usize, label: &str| MenuItem::Item { id, label: label.to_string(), checked: false, grayed: false };
        let mut items = vec![item(MENU_CHARACTER, lang.menu_character()), item(MENU_JOURNAL, lang.menu_journal())];
        let tools = tools::Tool::ALL.into_iter().enumerate().map(|(i, tool)| item(MENU_TOOL_FIRST + i, lang.tool_name(tool))).collect();
        items.push(MenuItem::Submenu { label: lang.menu_tools().into(), items: tools });
        match self.muted_until() {
            Some(until) => items.push(item(MENU_UNMUTE, &lang.menu_unmute(&until))),
            None => items.push(MenuItem::Submenu {
                label: lang.menu_mute().into(),
                items: vec![item(MENU_MUTE_HOUR, lang.menu_mute_hour()), item(MENU_MUTE_MORNING, lang.menu_mute_morning())],
            }),
        }
        items.push(MenuItem::Separator);
        items.push(item(MENU_LANGUAGE, lang.menu_switch()));
        items.push(MenuItem::Item { id: MENU_SCANLINES, label: lang.menu_scanlines().into(), checked: !self.pet.scanlines_off, grayed: false });
        let on = AUTOSTART_ON.load(Ordering::SeqCst) == 2;
        items.push(MenuItem::Item { id: MENU_AUTOSTART, label: lang.menu_autostart().into(), checked: on, grayed: AUTOSTART_BUSY.load(Ordering::SeqCst) });
        // Without per-destination byte sizes, an administrator would see them.
        if self.sized == Some(false) && system::may_elevate() {
            items.push(item(MENU_ADMIN, lang.menu_admin()));
        }
        // Only when there is one waiting.
        if let Some(release) = crate::update::waiting() {
            items.push(item(MENU_UPDATE, &lang.menu_update(&release.version)));
        }
        items.push(MenuItem::Separator);
        items.push(item(MENU_QUIT, lang.menu_quit()));
        items
    }
}

#[cfg(feature = "trace")]
pub(super) fn test_command(name: &str) -> Option<usize> {
    if let Some(tool) = name.strip_prefix("tool:") {
        return tools::Tool::ALL.iter().position(|t| format!("{t:?}").eq_ignore_ascii_case(tool)).map(|i| MENU_TOOL_FIRST + i);
    }
    Some(match name {
        "character" => MENU_CHARACTER,
        "journal" => MENU_JOURNAL,
        "mute_hour" => MENU_MUTE_HOUR,
        "mute_morning" => MENU_MUTE_MORNING,
        "unmute" => MENU_UNMUTE,
        "language" => MENU_LANGUAGE,
        "scanlines" => MENU_SCANLINES,
        "quit" => MENU_QUIT,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINGLES: [usize; 11] = [
        MENU_CHARACTER,
        MENU_LANGUAGE,
        MENU_AUTOSTART,
        MENU_ADMIN,
        MENU_QUIT,
        MENU_JOURNAL,
        MENU_MUTE_HOUR,
        MENU_MUTE_MORNING,
        MENU_UNMUTE,
        MENU_SCANLINES,
        MENU_UPDATE,
    ];

    #[test]
    fn no_two_menu_items_answer_to_the_same_number() {
        let mut seen = SINGLES.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), SINGLES.len(), "two items share a number: {SINGLES:?}");
        assert!(SINGLES.iter().all(|id| *id < MENU_TOOL_FIRST), "an item is in the toolkit's range: {SINGLES:?}");
        assert_eq!(MENU_TOOL_LAST + 1 - MENU_TOOL_FIRST, tools::Tool::ALL.len(), "the range is the toolkit");
    }

    #[cfg(feature = "trace")]
    #[test]
    fn a_scripted_check_asks_for_the_item_it_names() {
        for name in ["character", "journal", "mute_hour", "mute_morning", "unmute", "language", "scanlines", "quit"] {
            let id = test_command(name).unwrap_or_else(|| panic!("{name} is not an item"));
            assert!(SINGLES.contains(&id), "{name} gave {id}");
        }
        assert_eq!(test_command("nothing of the sort"), None);
        for (i, tool) in tools::Tool::ALL.iter().enumerate() {
            let asked = format!("tool:{}", format!("{tool:?}").to_lowercase());
            assert_eq!(test_command(&asked), Some(MENU_TOOL_FIRST + i), "{asked}");
        }
        assert_eq!(test_command("tool:no such tool"), None);
    }
}
