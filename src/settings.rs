//! `settings.txt`: `key = value` lines, shared by every feature.
//!
//! Two views of the same file:
//! - `TOGGLES`: the on/off options of the "Race" menu section (`is_on`, `set`).
//! - free key/values for anything else, e.g. the leaderboard's size
//!   (`get_bool`, `get_f32`, `set_value`).
//!
//! Every save writes all keys, so one feature never drops another's settings.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::{api, tracer};

const FILE_NAME: &str = "settings.txt";

pub struct Toggle {
    /// Key in settings.txt.
    key: &'static str,
    /// Checkbox text.
    pub label: &'static str,
    default: bool,
    on: AtomicBool,
}

pub const CUTINS: usize = 0;
pub const POPUPS: usize = 1;
pub const CLOCK: usize = 2;
pub const HIDE_UI: usize = 3;
pub const SEEK_BAR: usize = 4;

pub static TOGGLES: [Toggle; 5] = [
    Toggle { key: "disable_skill_cutins", label: "Disable skill cut-ins (all horses)", default: false, on: AtomicBool::new(false) },
    Toggle { key: "hide_skill_popups", label: "Hide skill popups (right side)", default: false, on: AtomicBool::new(false) },
    Toggle { key: "race_clock", label: "Race clock", default: true, on: AtomicBool::new(true) },
    Toggle {
        key: "hide_race_ui",
        label: "Hide race UI while running (buttons, icon, marker, commentary)",
        default: false,
        on: AtomicBool::new(false),
    },
    Toggle { key: "seek_bar", label: "Seek bar (drag to jump in the replay)", default: true, on: AtomicBool::new(true) },
];

/// All keys from the file, toggles included. None until first use.
static VALUES: Mutex<Option<BTreeMap<String, String>>> = Mutex::new(None);

fn path() -> PathBuf {
    tracer::plugin_dir().join(FILE_NAME)
}

pub fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .filter(|(k, _)| !k.is_empty() && !k.starts_with('#'))
        .collect()
}

fn with<R>(f: impl FnOnce(&mut BTreeMap<String, String>) -> R) -> R {
    let mut guard = VALUES.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(|| parse(&fs::read_to_string(path()).unwrap_or_default()));
    f(map)
}

/// Toggle states from the file's values (defaults for missing keys).
fn toggle_values(map: &BTreeMap<String, String>) -> Vec<bool> {
    TOGGLES.iter().map(|t| map.get(t.key).map(|v| v == "true").unwrap_or(t.default)).collect()
}

fn apply_toggles(map: &BTreeMap<String, String>) {
    for (t, on) in TOGGLES.iter().zip(toggle_values(map)) {
        t.on.store(on, Ordering::Relaxed);
    }
}

/// (Re)reads settings.txt and applies the toggles.
pub fn load() {
    let map = parse(&fs::read_to_string(path()).unwrap_or_default());
    apply_toggles(&map);
    *VALUES.lock().unwrap_or_else(|e| e.into_inner()) = Some(map);
}

// ---------------------------------------------------------------- toggles

pub fn is_on(toggle: usize) -> bool {
    TOGGLES[toggle].on.load(Ordering::Relaxed)
}

pub fn set(toggle: usize, on: bool) {
    TOGGLES[toggle].on.store(on, Ordering::Relaxed);
    set_value(TOGGLES[toggle].key, on);
}

#[cfg(test)]
pub fn set_for_test(toggle: usize, on: bool) {
    TOGGLES[toggle].on.store(on, Ordering::Relaxed);
}

// ---------------------------------------------------------------- key/values

pub fn get_bool(key: &str, default: bool) -> bool {
    with(|m| m.get(key).map(|v| v == "true").unwrap_or(default))
}

pub fn get_f32(key: &str, default: f32) -> f32 {
    with(|m| m.get(key).and_then(|v| v.parse().ok()).filter(|v: &f32| v.is_finite()).unwrap_or(default))
}

/// Sets one key and saves the whole file.
pub fn set_value(key: &str, value: impl ToString) {
    let text = with(|m| {
        m.insert(key.to_owned(), value.to_string());
        m.iter().map(|(k, v)| format!("{k} = {v}\n")).collect::<String>()
    });
    let path = path();
    let result = path.parent().map_or(Ok(()), fs::create_dir_all).and_then(|_| fs::write(&path, text));
    if let Err(e) = result {
        api::get().warn(&format!("cannot save {}: {e}", path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_values() {
        let m = parse("a = true\n# c = 1\nb=0.5\nbad line\n");
        assert_eq!(m.get("a").map(String::as_str), Some("true"));
        assert_eq!(m.get("b").map(String::as_str), Some("0.5"));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn toggles_use_defaults_for_missing_keys() {
        // Pure: the global toggles are shared with other tests running in parallel.
        let map = parse("hide_race_ui = true\nleaderboard_scale = 1.20\n");
        let on = toggle_values(&map);
        assert!(on[HIDE_UI]);
        assert!(on[CLOCK], "race_clock defaults to on");
        assert!(!on[CUTINS] && !on[POPUPS]);
        // Other features' keys are kept alongside the toggles.
        assert_eq!(map.get("leaderboard_scale").map(String::as_str), Some("1.20"));
    }
}
