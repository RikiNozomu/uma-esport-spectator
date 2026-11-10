//! Menu toggles, persisted in `settings.txt` as `key = true/false` lines.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{api, tracer};

const FILE_NAME: &str = "settings.txt";

pub struct Toggle {
    /// Key in settings.txt.
    key: &'static str,
    /// Checkbox text.
    pub label: &'static str,
    on: AtomicBool,
}

pub const CUTINS: usize = 0;
pub const POPUPS: usize = 1;
pub const CLOCK: usize = 2;
pub const HIDE_UI: usize = 3;

/// Values here are the defaults when settings.txt has no line for a key.
pub static TOGGLES: [Toggle; 4] = [
    Toggle { key: "disable_skill_cutins", label: "Disable skill cut-ins (all horses)", on: AtomicBool::new(false) },
    Toggle { key: "hide_skill_popups", label: "Hide skill popups (right side)", on: AtomicBool::new(false) },
    Toggle { key: "race_clock", label: "Race clock", on: AtomicBool::new(true) },
    Toggle {
        key: "hide_race_ui",
        label: "Hide race UI while running (buttons, icon, marker, commentary)",
        on: AtomicBool::new(false),
    },
];

pub fn is_on(toggle: usize) -> bool {
    TOGGLES[toggle].on.load(Ordering::Relaxed)
}

pub fn set(toggle: usize, on: bool) {
    TOGGLES[toggle].on.store(on, Ordering::Relaxed);
    save();
}

#[cfg(test)]
pub fn set_for_test(toggle: usize, on: bool) {
    TOGGLES[toggle].on.store(on, Ordering::Relaxed);
}

fn path() -> PathBuf {
    tracer::plugin_dir().join(FILE_NAME)
}

pub fn load() {
    let Ok(text) = fs::read_to_string(path()) else { return };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        if let Some(t) = TOGGLES.iter().find(|t| t.key == k.trim()) {
            t.on.store(v.trim() == "true", Ordering::Relaxed);
        }
    }
}

fn save() {
    let path = path();
    let text: String = TOGGLES
        .iter()
        .map(|t| format!("{} = {}\n", t.key, t.on.load(Ordering::Relaxed)))
        .collect();
    let result = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|_| fs::write(&path, text));
    if let Err(e) = result {
        api::get().warn(&format!("cannot save {}: {e}", path.display()));
    }
}
