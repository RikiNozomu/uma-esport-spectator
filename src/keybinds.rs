#![cfg_attr(not(windows), allow(dead_code, unused_imports))] // polling is Windows-only
//! Rebindable keyboard shortcuts for the race player.
//!
//! One setting per action (`player_key_<name>` = Windows virtual-key code,
//! 0 = unbound). Polled every frame from the present callback with
//! GetAsyncKeyState, only while the game window has focus. Rebinding: the
//! player window calls `start_capture(action)`; the next key pressed becomes
//! the binding (Esc cancels, Backspace / Delete clears it).

use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use crate::player::{self, Command, STEP};
use crate::settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    TogglePause,
    Back,
    Forward,
    Slower,
    Faster,
    Rewind,
    ToggleWindow,
}

pub struct Binding {
    pub action: Action,
    key: &'static str,
    pub label: &'static str,
    default: i32,
}

pub const BINDINGS: [Binding; 7] = [
    Binding { action: Action::TogglePause, key: "player_key_play", label: "Play / pause", default: 0x20 },
    Binding { action: Action::Back, key: "player_key_back", label: "Back 5 s", default: 0x25 },
    Binding { action: Action::Forward, key: "player_key_forward", label: "Forward 5 s", default: 0x27 },
    Binding { action: Action::Slower, key: "player_key_slower", label: "Slower", default: 0x28 },
    Binding { action: Action::Faster, key: "player_key_faster", label: "Faster", default: 0x26 },
    Binding { action: Action::Rewind, key: "player_key_rewind", label: "Rewind", default: 0x52 },
    Binding { action: Action::ToggleWindow, key: "player_key_window", label: "Open / close player", default: 0x50 },
];

static KEYS: [AtomicI32; 7] = [const { AtomicI32::new(0) }; 7];
/// Index into BINDINGS being rebound, or -1.
static CAPTURING: AtomicI32 = AtomicI32::new(-1);

pub fn load_settings() {
    for (i, b) in BINDINGS.iter().enumerate() {
        let vk = settings::get_f32(b.key, b.default as f32) as i32;
        KEYS[i].store(vk.clamp(0, 0xFF), Ordering::Relaxed);
    }
}

pub fn key_of(i: usize) -> i32 {
    KEYS[i].load(Ordering::Relaxed)
}

/// Sets a binding; the same key is removed from any other action.
pub fn bind(i: usize, vk: i32) {
    if vk != 0 {
        for (j, b) in BINDINGS.iter().enumerate() {
            if j != i && key_of(j) == vk {
                KEYS[j].store(0, Ordering::Relaxed);
                settings::set_value(b.key, 0);
            }
        }
    }
    KEYS[i].store(vk, Ordering::Relaxed);
    settings::set_value(BINDINGS[i].key, vk);
}

pub fn reset_defaults() {
    for (i, b) in BINDINGS.iter().enumerate() {
        KEYS[i].store(b.default, Ordering::Relaxed);
        settings::set_value(b.key, b.default);
    }
}

pub fn start_capture(i: usize) {
    CAPTURING.store(i as i32, Ordering::Relaxed);
}

pub fn capturing() -> Option<usize> {
    usize::try_from(CAPTURING.load(Ordering::Relaxed)).ok()
}

pub fn command(action: Action) -> Option<Command> {
    Some(match action {
        Action::TogglePause => Command::TogglePause,
        Action::Back => Command::SeekBy(-STEP),
        Action::Forward => Command::SeekBy(STEP),
        Action::Slower => Command::Slower,
        Action::Faster => Command::Faster,
        Action::Rewind => Command::Rewind,
        Action::ToggleWindow => return None,
    })
}

/// Display name of a virtual-key code.
pub fn key_name(vk: i32) -> String {
    let fixed = match vk {
        0 => "none",
        0x08 => "Backspace",
        0x09 => "Tab",
        0x0D => "Enter",
        0x13 => "Pause",
        0x14 => "Caps Lock",
        0x1B => "Esc",
        0x20 => "Space",
        0x21 => "Page Up",
        0x22 => "Page Down",
        0x23 => "End",
        0x24 => "Home",
        0x25 => "Left",
        0x26 => "Up",
        0x27 => "Right",
        0x28 => "Down",
        0x2D => "Insert",
        0x2E => "Delete",
        0x6A => "Num *",
        0x6B => "Num +",
        0x6D => "Num -",
        0x6E => "Num .",
        0x6F => "Num /",
        0xBA => ";",
        0xBB => "=",
        0xBC => ",",
        0xBD => "-",
        0xBE => ".",
        0xBF => "/",
        0xC0 => "`",
        0xDB => "[",
        0xDC => "\\",
        0xDD => "]",
        0xDE => "'",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_owned();
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from(vk as u8).to_string(),
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => format!("Key 0x{vk:02X}"),
    }
}

/// Keys that cannot be bound: mouse buttons, modifiers, Windows keys.
pub fn bindable(vk: i32) -> bool {
    !matches!(vk, 0x01..=0x07 | 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5) && (0x08..=0xFE).contains(&vk)
}

/// What a key press during capture does.
#[derive(Debug, PartialEq)]
pub enum Captured {
    Cancel,
    Clear,
    Bind(i32),
}

pub fn captured(vk: i32) -> Captured {
    match vk {
        0x1B => Captured::Cancel,
        0x08 | 0x2E => Captured::Clear,
        _ => Captured::Bind(vk),
    }
}

// ---------------------------------------------------------------- polling (Windows)

/// Keys held in the previous frame (bit per BINDINGS index).
static HELD: AtomicI32 = AtomicI32::new(0);
/// Capture waits until every key is released (bit 0) before accepting one.
static CAPTURE_ARMED: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
pub fn poll() {
    use crate::sys;

    if let Some(i) = capturing() {
        poll_capture(i);
        return;
    }
    if !player::shortcuts_enabled() || !sys::game_has_focus() || sys::key_down(0x11) || sys::key_down(0x12) {
        HELD.store(0, Ordering::Relaxed);
        return;
    }
    let mut now = 0;
    for i in 0..BINDINGS.len() {
        let vk = key_of(i);
        if vk != 0 && sys::key_down(vk) {
            now |= 1 << i;
        }
    }
    let before = HELD.swap(now, Ordering::Relaxed);
    for (i, b) in BINDINGS.iter().enumerate() {
        if now & !before & (1 << i) == 0 {
            continue;
        }
        match command(b.action) {
            Some(cmd) => player::send(cmd),
            None => crate::player_gui::toggle_window(),
        }
    }
}

#[cfg(windows)]
fn poll_capture(i: usize) {
    use crate::sys;
    if !sys::game_has_focus() {
        return; // never take a key typed into another app
    }
    let pressed = (0x08..=0xFE).filter(|&vk| bindable(vk)).find(|&vk| sys::key_down(vk));
    // Arm once nothing is held, so the key that was down when capture
    // started (or Enter on the button) is not taken.
    if CAPTURE_ARMED.load(Ordering::Relaxed) == 0 {
        if pressed.is_none() {
            CAPTURE_ARMED.store(1, Ordering::Relaxed);
        }
        return;
    }
    let Some(vk) = pressed else { return };
    match captured(vk) {
        Captured::Cancel => {}
        Captured::Clear => bind(i, 0),
        Captured::Bind(vk) => bind(i, vk),
    }
    CAPTURE_ARMED.store(0, Ordering::Relaxed);
    CAPTURING.store(-1, Ordering::Relaxed);
    // The new key is down right now: do not fire its action this frame.
    HELD.store(i32::MAX, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names() {
        assert_eq!(key_name(0x20), "Space");
        assert_eq!(key_name(0x52), "R");
        assert_eq!(key_name(0x37), "7");
        assert_eq!(key_name(0x70), "F1");
        assert_eq!(key_name(0x7B), "F12");
        assert_eq!(key_name(0x65), "Num 5");
        assert_eq!(key_name(0), "none");
        assert_eq!(key_name(0xE2), "Key 0xE2");
    }

    #[test]
    fn capture_keys() {
        assert_eq!(captured(0x1B), Captured::Cancel);
        assert_eq!(captured(0x08), Captured::Clear);
        assert_eq!(captured(0x2E), Captured::Clear);
        assert_eq!(captured(0x4B), Captured::Bind(0x4B));
        assert!(!bindable(0x01), "mouse");
        assert!(!bindable(0x10) && !bindable(0xA2), "modifiers");
        assert!(bindable(0x20) && bindable(0x70));
    }

    #[test]
    fn defaults_are_distinct_and_named() {
        let mut keys: Vec<i32> = BINDINGS.iter().map(|b| b.default).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), BINDINGS.len());
        assert!(BINDINGS.iter().all(|b| !key_name(b.default).starts_with("Key")));
        assert_eq!(command(Action::ToggleWindow), None);
        assert_eq!(command(Action::Back), Some(Command::SeekBy(-STEP)));
    }
}
