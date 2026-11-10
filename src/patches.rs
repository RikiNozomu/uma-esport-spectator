//! Race patches: menu toggles that neutralize game methods.
//!
//! Each patched method is hooked once at load and stays hooked; its toggle
//! (see settings.rs) only flips a flag, so switching is free and applies on
//! the next call. While a toggle is on, the method's stub jumps to a
//! replacement instead of the original:
//!   - `ReturnFalse`: for bool methods, the game gets `false`.
//!   - `Skip`: for void methods, the method does nothing.
//! The method's return type is checked before hooking, so a replacement can
//! never hand the caller a value of the wrong kind.
//!
//! Choke points come from the function trace:
//!   - every skill activation calls RaceEventPlayer::IsPlayableCutIn; only a
//!     unique skill then reaches RaceManagerReplayBase::PlaySkillCutIn.
//!   - the skill plates on the right are shown per activation by
//!     RaceMainViewController::PlaySkillInfo / RaceManagerReplayBase::ShowMessageUISkill.

use std::ffi::c_void;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::il2cpp::{self, GAME};
use crate::settings::{self, CUTINS, POPUPS};
use crate::{api, sig, stub};

#[derive(Clone, Copy)]
enum Action {
    ReturnFalse,
    Skip,
}

impl Action {
    fn return_type(self) -> &'static str {
        match self {
            Action::ReturnFalse => "System.Boolean",
            Action::Skip => "System.Void",
        }
    }
    fn replacement(self) -> usize {
        match self {
            Action::ReturnFalse => ret_false as *const () as usize,
            Action::Skip => ret_void as *const () as usize,
        }
    }
}

struct Patch {
    toggle: usize,
    class: &'static str,
    method: &'static str,
    argc: u32,
    action: Action,
}

const PATCHES: [Patch; 3] = [
    Patch { toggle: CUTINS, class: "RaceEventPlayer", method: "IsPlayableCutIn", argc: 3, action: Action::ReturnFalse },
    Patch { toggle: POPUPS, class: "RaceMainViewController", method: "PlaySkillInfo", argc: 3, action: Action::Skip },
    Patch { toggle: POPUPS, class: "RaceManagerReplayBase", method: "ShowMessageUISkill", argc: 3, action: Action::Skip },
];

/// Original code of each patch (same order as PATCHES); 0 until hooked.
static TRAMPOLINES: [AtomicUsize; 3] = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];
/// Per patch: "ok" or why it is not hooked. Set once by `install`.
static STATUS: OnceLock<Vec<String>> = OnceLock::new();

/// Related methods whose signatures are logged for reference (not hooked).
const LOG_ONLY: [(&str, &str, u32); 4] = [
    ("RaceEventPlayer", "TryRequestCutIn", 4),
    ("RaceManagerReplayBase", "PlaySkillCutIn", 6),
    ("RaceManagerReplayBase", "ReserveSkillInfoUI", 3),
    ("RaceManagerReplayBase", "ReserveMessageUISkill", 2),
];

// ---------------------------------------------------------------- hot path

/// Replacement for bool methods: ignores the arguments, returns false.
extern "win64" fn ret_false() -> bool {
    false
}

/// Replacement for void methods: ignores the arguments, does nothing.
extern "win64" fn ret_void() {}

/// Called by the stub of patch `index`; returns where the stub jumps.
extern "win64" fn enter(index: u32) -> usize {
    // index comes only from our own stubs, so it is always in range.
    let i = index as usize;
    let patch = &PATCHES[i];
    if settings::is_on(patch.toggle) {
        return patch.action.replacement();
    }
    // The hook goes live a moment before its trampoline is stored.
    loop {
        let t = TRAMPOLINES[i].load(Ordering::Acquire);
        if t != 0 {
            return t;
        }
        std::hint::spin_loop();
    }
}

// ---------------------------------------------------------------- status

/// One line per patch of this toggle, e.g. "PlaySkillInfo: ok".
pub fn status(toggle: usize) -> String {
    let Some(status) = STATUS.get() else { return "waiting for the game".to_owned() };
    let mut out = String::new();
    for (patch, s) in PATCHES.iter().zip(status) {
        if patch.toggle == toggle {
            if !out.is_empty() {
                out.push('\n');
            }
            let _ = write!(out, "{}: {s}", patch.method);
        }
    }
    out
}

// ---------------------------------------------------------------- install

/// Called once when IL2CPP is ready: hooks every patch.
pub fn install() {
    let api = api::get();

    for (class, name, argc) in LOG_ONLY {
        let text = unsafe { il2cpp::class(GAME, "Gallop", class).and_then(|k| il2cpp::method(k, name, argc)) }
            .map(|m| unsafe { sig::signature(m.info as *const c_void) })
            .unwrap_or_else(|e| e);
        api.info(&format!("signature Gallop.{class}::{name}: {text}"));
    }

    let status = PATCHES
        .iter()
        .enumerate()
        .map(|(i, patch)| match unsafe { hook(i, patch) } {
            Ok(()) => "ok".to_owned(),
            Err(e) => {
                api.error(&format!("patch Gallop.{}::{} not installed: {e}", patch.class, patch.method));
                format!("FAILED ({e})")
            }
        })
        .collect();
    let _ = STATUS.set(status);
}

unsafe fn hook(index: usize, patch: &Patch) -> Result<(), String> {
    let api = api::get();
    let klass = il2cpp::class(GAME, "Gallop", patch.class)?;
    let method = il2cpp::method(klass, patch.method, patch.argc)?;
    api.info(&format!(
        "signature Gallop.{}::{}: {}",
        patch.class,
        patch.method,
        sig::signature(method.info as *const c_void)
    ));

    // A replacement must give the caller the kind of value it expects.
    let expected = patch.action.return_type();
    let actual = il2cpp::return_type(method);
    if actual != expected {
        return Err(format!("returns {actual}, expected {expected}"));
    }

    let code = stub::build(index as u32, enter).ok_or("cannot allocate stub memory")?;
    let trampoline = il2cpp::hook(method.code, code as usize)?;
    TRAMPOLINES[index].store(trampoline, Ordering::Release);
    api.info(&format!("patch installed on Gallop.{}::{}", patch.class, patch.method));
    Ok(())
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::*;
    use crate::settings::set_for_test as set_toggle;
    use std::sync::atomic::AtomicU32;

    // Single test: TOGGLES/TRAMPOLINES are globals.
    static VOID_CALLS: AtomicU32 = AtomicU32::new(0);

    extern "win64" fn original_bool(_this: u64, _a: u64, _b: u64, _c: u64, _m: u64) -> bool {
        true
    }
    extern "win64" fn original_void(_this: u64, _a: u64, _b: u64, _c: u64, _m: u64) {
        VOID_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn stubs_follow_their_toggle() {
        TRAMPOLINES[0].store(original_bool as *const () as usize, Ordering::SeqCst);
        TRAMPOLINES[1].store(original_void as *const () as usize, Ordering::SeqCst);
        let cutin: extern "win64" fn(u64, u64, u64, u64, u64) -> bool =
            unsafe { std::mem::transmute(stub::build(0, enter).expect("exec memory")) };
        let popup: extern "win64" fn(u64, u64, u64, u64, u64) =
            unsafe { std::mem::transmute(stub::build(1, enter).expect("exec memory")) };

        set_toggle(CUTINS, false);
        set_toggle(POPUPS, false);
        assert!(cutin(1, 2, 3, 4, 5));
        popup(1, 2, 3, 4, 5);
        assert_eq!(VOID_CALLS.load(Ordering::SeqCst), 1);

        set_toggle(CUTINS, true);
        set_toggle(POPUPS, true);
        assert!(!cutin(1, 2, 3, 4, 5));
        popup(1, 2, 3, 4, 5);
        assert_eq!(VOID_CALLS.load(Ordering::SeqCst), 1, "original must not run while hidden");
    }
}
