//! Race patches: menu toggles that neutralize game methods.
//!
//! Each patched method is hooked once at load and stays hooked; its toggle
//! only flips a flag, so switching is free and applies on the next call.
//! While a toggle is on, the method's stub jumps to a replacement instead of
//! the original:
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
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::api::{self, cstr};
use crate::{sig, stub, tracer};

const ASSEMBLY: &str = "umamusume.dll";
const SETTINGS_FILE: &str = "settings.txt";

// ---------------------------------------------------------------- tables

pub struct Toggle {
    /// Key in settings.txt.
    key: &'static str,
    /// Checkbox text.
    pub label: &'static str,
    on: AtomicBool,
}

pub static TOGGLES: [Toggle; 2] = [
    Toggle { key: "disable_skill_cutins", label: "Disable skill cut-ins (all horses)", on: AtomicBool::new(false) },
    Toggle { key: "hide_skill_popups", label: "Hide skill popups (right side)", on: AtomicBool::new(false) },
];
const CUTINS: usize = 0;
const POPUPS: usize = 1;

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
    argc: i32,
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
const LOG_ONLY: [(&str, &str, i32); 4] = [
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
    if TOGGLES[patch.toggle].on.load(Ordering::Relaxed) {
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

// ---------------------------------------------------------------- control

pub fn is_on(toggle: usize) -> bool {
    TOGGLES[toggle].on.load(Ordering::Relaxed)
}

pub fn set(toggle: usize, on: bool) {
    TOGGLES[toggle].on.store(on, Ordering::Relaxed);
    save_settings();
}

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

fn settings_path() -> PathBuf {
    tracer::plugin_dir().join(SETTINGS_FILE)
}

fn load_settings() {
    let Ok(text) = fs::read_to_string(settings_path()) else { return };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        if let Some(t) = TOGGLES.iter().find(|t| t.key == k.trim()) {
            t.on.store(v.trim() == "true", Ordering::Relaxed);
        }
    }
}

fn save_settings() {
    let path = settings_path();
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

// ---------------------------------------------------------------- install

/// Called once when IL2CPP is ready: loads settings and hooks every patch.
pub fn install() {
    load_settings();
    let api = api::get();
    let image = unsafe { (api.il2cpp_get_assembly_image)(cstr(ASSEMBLY).as_ptr()) };
    if image.is_null() {
        api.error(&format!("patches not installed: assembly not found: {ASSEMBLY}"));
        let _ = STATUS.set(vec!["assembly not found".to_owned(); PATCHES.len()]);
        return;
    }

    for (class, name, argc) in LOG_ONLY {
        let m = unsafe { find_method(image, class, name, argc) };
        let text = if m.is_null() { "not found".to_owned() } else { unsafe { sig::signature(m) } };
        api.info(&format!("signature Gallop.{class}::{name}: {text}"));
    }

    let status = PATCHES
        .iter()
        .enumerate()
        .map(|(i, patch)| match unsafe { hook(image, i, patch) } {
            Ok(()) => "ok".to_owned(),
            Err(e) => {
                api.error(&format!("patch Gallop.{}::{} not installed: {e}", patch.class, patch.method));
                format!("FAILED ({e})")
            }
        })
        .collect();
    let _ = STATUS.set(status);
}

unsafe fn find_method(image: *const c_void, class: &str, method: &str, argc: i32) -> *const c_void {
    let api = api::get();
    let klass = (api.il2cpp_get_class)(image, cstr("Gallop").as_ptr(), cstr(class).as_ptr());
    if klass.is_null() {
        return std::ptr::null();
    }
    (api.il2cpp_get_method)(klass, cstr(method).as_ptr(), argc)
}

unsafe fn hook(image: *const c_void, index: usize, patch: &Patch) -> Result<(), String> {
    let api = api::get();
    let method = find_method(image, patch.class, patch.method, patch.argc);
    if method.is_null() {
        return Err("method not found".to_owned());
    }
    api.info(&format!("signature Gallop.{}::{}: {}", patch.class, patch.method, sig::signature(method)));

    // A replacement must give the caller the kind of value it expects.
    let expected = patch.action.return_type();
    match sig::return_type(method) {
        Some(ty) if ty == expected => {}
        Some(ty) => return Err(format!("returns {ty}, expected {expected}")),
        None => return Err("cannot read return type".to_owned()),
    }
    // MethodInfo.methodPointer is the first field.
    let addr = *(method as *const usize);
    if addr == 0 {
        return Err("method has no code".to_owned());
    }

    let code = stub::build(index as u32, enter).ok_or("cannot allocate stub memory")?;
    let interceptor = (api.hachimi_get_interceptor)((api.hachimi_instance)());
    let trampoline = (api.interceptor_hook)(interceptor, addr as *mut c_void, code as *mut c_void) as usize;
    if trampoline == 0 {
        return Err("MinHook refused (already hooked?)".to_owned());
    }
    TRAMPOLINES[index].store(trampoline, Ordering::Release);
    api.info(&format!("patch installed on Gallop.{}::{}", patch.class, patch.method));
    Ok(())
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::*;
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

        TOGGLES[CUTINS].on.store(false, Ordering::SeqCst);
        TOGGLES[POPUPS].on.store(false, Ordering::SeqCst);
        assert!(cutin(1, 2, 3, 4, 5));
        popup(1, 2, 3, 4, 5);
        assert_eq!(VOID_CALLS.load(Ordering::SeqCst), 1);

        TOGGLES[CUTINS].on.store(true, Ordering::SeqCst);
        TOGGLES[POPUPS].on.store(true, Ordering::SeqCst);
        assert!(!cutin(1, 2, 3, 4, 5));
        popup(1, 2, 3, 4, 5);
        assert_eq!(VOID_CALLS.load(Ordering::SeqCst), 1, "original must not run while hidden");
    }
}
