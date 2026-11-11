//! `settings.txt`: `key = value` lines, shared by every feature.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::{api, tracer};

const FILE_NAME: &str = "settings.txt";

static VALUES: Mutex<Option<BTreeMap<String, String>>> = Mutex::new(None);

fn path() -> PathBuf {
    tracer::plugin_dir().join(FILE_NAME)
}

fn with<R>(f: impl FnOnce(&mut BTreeMap<String, String>) -> R) -> R {
    let mut guard = VALUES.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(|| parse(&fs::read_to_string(path()).unwrap_or_default()));
    f(map)
}

pub fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .filter(|(k, _)| !k.is_empty() && !k.starts_with('#'))
        .collect()
}

pub fn get_bool(key: &str, default: bool) -> bool {
    with(|m| m.get(key).map(|v| v == "true").unwrap_or(default))
}

pub fn get_f32(key: &str, default: f32) -> f32 {
    with(|m| m.get(key).and_then(|v| v.parse().ok()).filter(|v: &f32| v.is_finite()).unwrap_or(default))
}

pub fn set(key: &str, value: impl ToString) {
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
    #[test]
    fn parses_key_values() {
        let m = super::parse("a = true\n# c = 1\nb=0.5\nbad line\n");
        assert_eq!(m.get("a").map(String::as_str), Some("true"));
        assert_eq!(m.get("b").map(String::as_str), Some("0.5"));
        assert_eq!(m.len(), 2);
    }
}
