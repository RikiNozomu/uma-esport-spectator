//! "Disable skill cut-ins": makes `Gallop.RaceEventPlayer::IsPlayableCutIn`
//! return false, for every horse including the player's.
//!
//! The trace showed every skill activation calls IsPlayableCutIn then
//! TryRequestCutIn; only a unique skill then reaches
//! RaceManagerReplayBase::PlaySkillCutIn and the SkillCutIn race state.
//! Blocking at the IsPlayableCutIn check keeps the normal skill popup/effect.
//!
//! The hook is installed once at load and stays; the checkbox only flips a
//! flag, so toggling is free and takes effect on the next skill.

use std::ffi::c_void;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::api::{self, cstr};
use crate::{sig, stub, tracer};

const ASSEMBLY: &str = "umamusume.dll";
const SETTINGS_FILE: &str = "settings.txt";
const SETTING_KEY: &str = "disable_skill_cutins";

/// User setting: true = cut-ins are blocked.
static DISABLE: AtomicBool = AtomicBool::new(false);
/// Original code of IsPlayableCutIn; 0 until the hook is in place.
static TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);
/// Set once by `install`: "active" or why the hook is missing.
static STATUS: OnceLock<String> = OnceLock::new();

// ---------------------------------------------------------------- hot path

/// Jump target that ignores the arguments and returns false.
extern "win64" fn ret_false() -> bool {
    false
}

extern "win64" fn enter(_index: u32) -> usize {
    if DISABLE.load(Ordering::Relaxed) {
        return ret_false as *const () as usize;
    }
    loop {
        let t = TRAMPOLINE.load(Ordering::Acquire);
        if t != 0 {
            return t;
        }
        std::hint::spin_loop();
    }
}

// ---------------------------------------------------------------- control

pub fn is_disabled() -> bool {
    DISABLE.load(Ordering::Relaxed)
}

pub fn set_disabled(on: bool) {
    DISABLE.store(on, Ordering::Relaxed);
    save_setting(on);
}

pub fn status() -> &'static str {
    STATUS.get().map(String::as_str).unwrap_or("hook: waiting for the game")
}

fn settings_path() -> PathBuf {
    tracer::plugin_dir().join(SETTINGS_FILE)
}

fn load_setting() {
    let Ok(text) = fs::read_to_string(settings_path()) else { return };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == SETTING_KEY {
                DISABLE.store(v.trim() == "true", Ordering::Relaxed);
            }
        }
    }
}

fn save_setting(on: bool) {
    let path = settings_path();
    let result = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|_| fs::write(&path, format!("{SETTING_KEY} = {on}\n")));
    if let Err(e) = result {
        api::get().warn(&format!("cannot save {}: {e}", path.display()));
    }
}

// ---------------------------------------------------------------- install

/// Called once when IL2CPP is ready. Logs the signatures of the cut-in
/// methods (for checking the choke point) and hooks IsPlayableCutIn.
pub fn install() {
    load_setting();
    let status = unsafe { try_install() }.unwrap_or_else(|e| {
        api::get().error(&format!("skill cut-in hook not installed: {e}"));
        format!("hook failed: {e}")
    });
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

unsafe fn try_install() -> Result<String, String> {
    let api = api::get();
    let image = (api.il2cpp_get_assembly_image)(cstr(ASSEMBLY).as_ptr());
    if image.is_null() {
        return Err(format!("assembly not found: {ASSEMBLY}"));
    }

    // Logged for reference; only the first one is hooked.
    let related = [
        ("RaceEventPlayer", "IsPlayableCutIn", 3),
        ("RaceEventPlayer", "TryRequestCutIn", 4),
        ("RaceManagerReplayBase", "PlaySkillCutIn", 6),
    ];
    for (class, name, argc) in related {
        let m = find_method(image, class, name, argc);
        let text = if m.is_null() { "not found".to_owned() } else { sig::signature(m) };
        api.info(&format!("signature Gallop.{class}::{name}: {text}"));
    }

    let method = find_method(image, "RaceEventPlayer", "IsPlayableCutIn", 3);
    if method.is_null() {
        return Err("Gallop.RaceEventPlayer::IsPlayableCutIn(3) not found".to_owned());
    }
    // Returning false from anything but a bool method would corrupt the caller.
    match sig::return_type(method).as_deref() {
        Some("System.Boolean") => {}
        Some(other) => return Err(format!("IsPlayableCutIn returns {other}, expected System.Boolean")),
        None => return Err("cannot read IsPlayableCutIn's return type".to_owned()),
    }
    // MethodInfo.methodPointer is the first field.
    let addr = *(method as *const usize);
    if addr == 0 {
        return Err("IsPlayableCutIn has no code".to_owned());
    }

    let code = stub::build(0, enter).ok_or("cannot allocate stub memory")?;
    let interceptor = (api.hachimi_get_interceptor)((api.hachimi_instance)());
    let trampoline = (api.interceptor_hook)(interceptor, addr as *mut c_void, code as *mut c_void) as usize;
    if trampoline == 0 {
        return Err("MinHook refused IsPlayableCutIn (already hooked?)".to_owned());
    }
    TRAMPOLINE.store(trampoline, Ordering::Release);
    api.info("skill cut-in hook installed on Gallop.RaceEventPlayer::IsPlayableCutIn");
    Ok("hook: active".to_owned())
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::*;

    extern "win64" fn original(_this: u64, _a: u64, _b: u64, _c: u64, _m: u64) -> bool {
        true
    }

    #[test]
    fn stub_returns_false_only_when_disabled() {
        TRAMPOLINE.store(original as *const () as usize, Ordering::SeqCst);
        let f: extern "win64" fn(u64, u64, u64, u64, u64) -> bool =
            unsafe { std::mem::transmute(stub::build(0, enter).expect("exec memory")) };

        DISABLE.store(false, Ordering::SeqCst);
        assert!(f(1, 2, 3, 4, 5));
        DISABLE.store(true, Ordering::SeqCst);
        assert!(!f(1, 2, 3, 4, 5));
    }
}
