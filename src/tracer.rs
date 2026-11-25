//! Function tracer: installs entry hooks on the targeted methods and writes
//! a log of which ones run while recording is on.
//!
//! Hooks are installed once (the first time recording is turned on) and stay
//! for the rest of the session. Turning recording off only stops logging;
//! an idle hook costs a few nanoseconds per call.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::api::{self, cstr};
use crate::targets::{self, Targets};
use crate::{sig, stub, sys};

const ASSEMBLY: &str = "umamusume.dll";
const PLUGIN_DIR: &str = "uma-esport-spectator";
/// Safety limits. Each hook briefly suspends every game thread while it is
/// installed, so thousands of hooks mean a freeze of several seconds.
const MAX_HOOKS: usize = 8000;
/// An address shared by more methods than this is linker-folded trivial code
/// (empty bodies, simple getters); hooking it would log under the wrong name.
const MAX_SHARED_NAMES: usize = 4;
/// A new log file every this many lines (~140 MB), so each part still opens
/// in an editor.
const PART_LINES: u64 = 2_000_000;
/// Line limit per recording, in millions (menu slider, `trace_max_lines_m`).
pub const MIN_LIMIT_M: f32 = 1.0;
pub const MAX_LIMIT_M: f32 = 100.0;
const DEFAULT_LIMIT_M: f32 = 10.0;
const LIMIT_KEY: &str = "trace_max_lines_m";

pub fn limit_millions() -> f32 {
    crate::settings::get_f32(LIMIT_KEY, DEFAULT_LIMIT_M).clamp(MIN_LIMIT_M, MAX_LIMIT_M).round()
}

pub fn set_limit_millions(m: f32) {
    crate::settings::set_value(LIMIT_KEY, m.clamp(MIN_LIMIT_M, MAX_LIMIT_M).round());
}

pub static GAME_READY: AtomicBool = AtomicBool::new(false);
pub static EVERY_CALL: AtomicBool = AtomicBool::new(false);
static RECORDING: AtomicBool = AtomicBool::new(false);
static START_AFTER_INSTALL: AtomicBool = AtomicBool::new(false);

const NOT_INSTALLED: u8 = 0;
const INSTALLING: u8 = 1;
const INSTALLED: u8 = 2;
const INSTALL_FAILED: u8 = 3;
static INSTALL_STATE: AtomicU8 = AtomicU8::new(NOT_INSTALLED);

static HOOKED: AtomicUsize = AtomicUsize::new(0);
static FAILED: AtomicUsize = AtomicUsize::new(0);

struct Entry {
    name: String,
    /// Address of the original code; 0 until the hook is in place.
    trampoline: AtomicUsize,
    calls: AtomicU64,
}

static TABLE: OnceLock<Box<[Entry]>> = OnceLock::new();

struct Session {
    out: BufWriter<File>,
    /// First part's path; later parts add `_partN`.
    path: PathBuf,
    part: u32,
    started: Instant,
    lines: u64,
    max_lines: u64,
    /// Lines per file (`PART_LINES`; small in tests).
    part_lines: u64,
}

impl Session {
    fn part_path(&self, part: u32) -> PathBuf {
        if part <= 1 {
            return self.path.clone();
        }
        let stem = self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.path.with_file_name(format!("{stem}_part{part}.log"))
    }

    /// Continues the log in a new file. Keeps the current one on failure.
    fn next_part(&mut self) {
        let path = self.part_path(self.part + 1);
        if let Ok(file) = File::create(&path) {
            let _ = writeln!(self.out, "# continued in {}", path.display());
            let _ = self.out.flush();
            self.out = BufWriter::with_capacity(256 * 1024, file);
            self.part += 1;
            let _ = writeln!(self.out, "# uma-esport-spectator function trace, part {}", self.part);
        }
    }

    /// Writes one call line, starting a new part file every `part_lines`.
    /// False once the line limit is reached.
    fn write_line(&mut self, thread: u32, name: &str, flush: bool) -> bool {
        if self.lines >= self.max_lines {
            return false;
        }
        if self.lines > 0 && self.lines % self.part_lines == 0 {
            self.next_part();
        }
        self.lines += 1;
        let ms = self.started.elapsed().as_secs_f64() * 1000.0;
        let _ = writeln!(self.out, "{ms:>12.3}\t{thread}\t{name}");
        if flush {
            let _ = self.out.flush();
        }
        true
    }
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn session() -> std::sync::MutexGuard<'static, Option<Session>> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------- hot path

/// Called by every stub. Runs on whatever thread the game calls the method on,
/// so it must not panic and must not call into il2cpp.
extern "win64" fn enter(index: u32) -> usize {
    let entry = &TABLE.get().expect("stub exists only after TABLE is set")[index as usize];
    if RECORDING.load(Ordering::Relaxed) {
        let previous = entry.calls.fetch_add(1, Ordering::Relaxed);
        if previous == 0 || EVERY_CALL.load(Ordering::Relaxed) {
            write_call(entry, previous == 0);
        }
    }
    // The hook goes live a moment before we learn its trampoline address;
    // wait out that window instead of jumping to 0.
    loop {
        let t = entry.trampoline.load(Ordering::Acquire);
        if t != 0 {
            return t;
        }
        std::hint::spin_loop();
    }
}

fn write_call(entry: &Entry, first: bool) {
    let mut guard = session();
    let Some(s) = guard.as_mut() else { return };
    // First-seen lines are rare and the most valuable if the game crashes: flushed.
    s.write_line(sys::thread_id(), &entry.name, first);
}

// ---------------------------------------------------------------- control

/// What the checkbox should show: recording, or about to once hooks are in.
pub fn wants_recording() -> bool {
    RECORDING.load(Ordering::Relaxed) || START_AFTER_INSTALL.load(Ordering::Relaxed)
}

pub fn set_recording(on: bool) {
    let api = api::get();
    if !on {
        START_AFTER_INSTALL.store(false, Ordering::Relaxed);
        stop();
        return;
    }
    match INSTALL_STATE.load(Ordering::Acquire) {
        INSTALLED => start(),
        INSTALLING => START_AFTER_INSTALL.store(true, Ordering::Relaxed),
        INSTALL_FAILED => api.notify("Function trace: hook install failed, see hachimi.log"),
        _ => {
            INSTALL_STATE.store(INSTALLING, Ordering::Release);
            START_AFTER_INSTALL.store(true, Ordering::Relaxed);
            api.notify("Function trace: installing hooks, the game may freeze for a few seconds");
            // il2cpp metadata must be walked on the game's main thread.
            unsafe { (api.il2cpp_schedule_on_thread)((api.il2cpp_get_main_thread)(), install_on_main_thread) };
        }
    }
}

pub fn status() -> String {
    let hooks = match INSTALL_STATE.load(Ordering::Acquire) {
        NOT_INSTALLED => "hooks: not installed yet".to_owned(),
        INSTALLING => "hooks: installing...".to_owned(),
        INSTALL_FAILED => "hooks: install failed (see hachimi.log)".to_owned(),
        _ => format!(
            "hooks: {} installed, {} failed",
            HOOKED.load(Ordering::Relaxed),
            FAILED.load(Ordering::Relaxed)
        ),
    };
    match session().as_ref() {
        Some(s) if s.lines >= s.max_lines => {
            format!("{hooks} | recording, line limit reached ({} lines, {} files)", s.lines, s.part)
        }
        Some(s) => format!("{hooks} | recording, {} / {} lines, {} files", s.lines, s.max_lines, s.part),
        None => format!("{hooks} | idle"),
    }
}

pub fn plugin_dir() -> PathBuf {
    let base = unsafe { (api::get().hachimi_get_base_dir)() };
    let base = if base.is_null() {
        PathBuf::from(".")
    } else {
        PathBuf::from(unsafe { CStr::from_ptr(base) }.to_string_lossy().into_owned())
    };
    base.join(PLUGIN_DIR)
}

fn start() {
    let api = api::get();
    let mut guard = session();
    if guard.is_some() {
        return;
    }
    let dir = plugin_dir().join("logs");
    let path = dir.join(format!("trace_{}.log", sys::timestamp()));
    let file = fs::create_dir_all(&dir).and_then(|_| File::create(&path));
    let file = match file {
        Ok(f) => f,
        Err(e) => {
            api.error(&format!("cannot create {}: {e}", path.display()));
            api.notify("Function trace: cannot create the log file");
            return;
        }
    };
    if let Some(table) = TABLE.get() {
        for e in table.iter() {
            e.calls.store(0, Ordering::Relaxed);
        }
    }
    let mut out = BufWriter::with_capacity(256 * 1024, file);
    let mode = if EVERY_CALL.load(Ordering::Relaxed) { "every call" } else { "first call of each method" };
    let _ = writeln!(out, "# uma-esport-spectator function trace ({mode})");
    let _ = writeln!(out, "# columns: ms since start, thread id, method(arg count)");
    let max_lines = limit_millions() as u64 * 1_000_000;
    *guard = Some(Session { out, path, part: 1, started: Instant::now(), lines: 0, max_lines, part_lines: PART_LINES });
    drop(guard);
    RECORDING.store(true, Ordering::Release);
    api.notify("Function trace: recording");
}

fn stop() {
    RECORDING.store(false, Ordering::Release);
    let Some(mut s) = session().take() else { return };
    let api = api::get();

    let mut called: Vec<(u64, &str)> = TABLE
        .get()
        .map(|t| t.iter().map(|e| (e.calls.load(Ordering::Relaxed), e.name.as_str())).collect())
        .unwrap_or_default();
    called.retain(|(n, _)| *n > 0);
    called.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));

    let secs = s.started.elapsed().as_secs_f64();
    let _ = writeln!(s.out, "\n# ---- summary: {} methods called in {secs:.1}s ----", called.len());
    if s.lines >= s.max_lines {
        let _ = writeln!(s.out, "# line limit reached ({}); later calls are only counted below", s.max_lines);
    }
    let _ = writeln!(s.out, "# calls\tmethod");
    for (n, name) in &called {
        let _ = writeln!(s.out, "{n}\t{name}");
    }
    let _ = s.out.flush();
    let saved = if s.part > 1 {
        format!("{} (+{} parts)", s.path.display(), s.part - 1)
    } else {
        s.path.display().to_string()
    };
    api.info(&format!("trace saved: {saved}"));
    api.notify(&format!("Function trace saved: {saved}"));
}

// ---------------------------------------------------------------- install

unsafe extern "C" fn install_on_main_thread() {
    let api = api::get();
    match install() {
        Ok(()) => {
            INSTALL_STATE.store(INSTALLED, Ordering::Release);
            let msg = format!(
                "Function trace: {} hooks installed, {} failed",
                HOOKED.load(Ordering::Relaxed),
                FAILED.load(Ordering::Relaxed)
            );
            api.info(&msg);
            api.notify(&msg);
            if START_AFTER_INSTALL.swap(false, Ordering::Relaxed) {
                start();
            }
        }
        Err(e) => {
            INSTALL_STATE.store(INSTALL_FAILED, Ordering::Release);
            START_AFTER_INSTALL.store(false, Ordering::Relaxed);
            api.error(&format!("hook install failed: {e}"));
            api.notify("Function trace: hook install failed, see hachimi.log");
        }
    }
}

type ClassCountFn = unsafe extern "C" fn(image: *const c_void) -> usize;
type ClassAtFn = unsafe extern "C" fn(image: *const c_void, index: usize) -> *mut c_void;
type ClassStrFn = unsafe extern "C" fn(klass: *mut c_void) -> *const c_char;
type ClassBoolFn = unsafe extern "C" fn(klass: *mut c_void) -> bool;
type ClassParentFn = unsafe extern "C" fn(klass: *mut c_void) -> *mut c_void;
type MethodStrFn = unsafe extern "C" fn(method: *const c_void) -> *const c_char;
type MethodU32Fn = unsafe extern "C" fn(method: *const c_void) -> u32;

struct Il2Cpp {
    class_count: ClassCountFn,
    class_at: ClassAtFn,
    class_name: ClassStrFn,
    class_namespace: ClassStrFn,
    class_is_generic: ClassBoolFn,
    class_declaring_type: ClassParentFn,
    method_name: MethodStrFn,
    method_param_count: MethodU32Fn,
}

impl Il2Cpp {
    unsafe fn resolve() -> Result<Il2Cpp, String> {
        let api = api::get();
        macro_rules! sym {
            ($name:literal) => {
                api.il2cpp_fn($name).ok_or_else(|| format!("il2cpp symbol not found: {}", $name))?
            };
        }
        Ok(Il2Cpp {
            class_count: sym!("il2cpp_image_get_class_count"),
            class_at: sym!("il2cpp_image_get_class"),
            class_name: sym!("il2cpp_class_get_name"),
            class_namespace: sym!("il2cpp_class_get_namespace"),
            class_is_generic: sym!("il2cpp_class_is_generic"),
            class_declaring_type: sym!("il2cpp_class_get_declaring_type"),
            method_name: sym!("il2cpp_method_get_name"),
            method_param_count: sym!("il2cpp_method_get_param_count"),
        })
    }

    /// `Namespace.Outer+Inner`
    unsafe fn full_class_name(&self, klass: *mut c_void) -> String {
        let mut parts = vec![lossy((self.class_name)(klass))];
        let mut outermost = klass;
        // Depth limit guards against a malformed chain.
        for _ in 0..16 {
            let parent = (self.class_declaring_type)(outermost);
            if parent.is_null() {
                break;
            }
            parts.push(lossy((self.class_name)(parent)));
            outermost = parent;
        }
        parts.reverse();
        let ns = lossy((self.class_namespace)(outermost));
        let name = parts.join("+");
        if ns.is_empty() { name } else { format!("{ns}.{name}") }
    }
}

unsafe fn lossy(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
}

/// Every loaded assembly image except `skip`.
unsafe fn other_images(skip: *const c_void) -> Vec<*const c_void> {
    type DomainGet = unsafe extern "C" fn() -> *mut c_void;
    type GetAssemblies = unsafe extern "C" fn(domain: *mut c_void, size: *mut usize) -> *const *const c_void;
    type AssemblyImage = unsafe extern "C" fn(assembly: *const c_void) -> *const c_void;
    let api = api::get();
    let (Some(domain_get), Some(get_assemblies), Some(image_of)) = (
        api.il2cpp_fn::<DomainGet>("il2cpp_domain_get"),
        api.il2cpp_fn::<GetAssemblies>("il2cpp_domain_get_assemblies"),
        api.il2cpp_fn::<AssemblyImage>("il2cpp_assembly_get_image"),
    ) else {
        return Vec::new();
    };
    let mut n = 0usize;
    let list = get_assemblies(domain_get(), &mut n);
    if list.is_null() {
        return Vec::new();
    }
    (0..n.min(4096)).map(|i| image_of(*list.add(i))).filter(|img| !img.is_null() && *img != skip).collect()
}

unsafe fn install() -> Result<(), String> {
    let api = api::get();
    let il2cpp = Il2Cpp::resolve()?;

    let image = (api.il2cpp_get_assembly_image)(cstr(ASSEMBLY).as_ptr());
    if image.is_null() {
        return Err(format!("assembly not found: {ASSEMBLY}"));
    }
    // The game assembly first, then every other loaded one (CriWare, Cute.*,
    // Unity modules): targets only match the classes they name.
    let mut images = vec![image];
    images.extend(other_images(image));

    let dir = plugin_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let targets = Targets::load_or_create(&dir.join(targets::FILE_NAME));

    // 1. Collect candidate methods, grouped by code address (several methods
    //    can share one address after the linker folds identical code).
    let mut order: Vec<usize> = Vec::new();
    // (label for the trace log, signature for hooked_methods.txt)
    let mut by_addr: HashMap<usize, Vec<(String, String)>> = HashMap::new();
    let classes: Vec<*mut c_void> =
        images.iter().flat_map(|&img| (0..(il2cpp.class_count)(img)).map(move |i| (il2cpp.class_at)(img, i))).collect();
    for klass in classes {
        if klass.is_null() || (il2cpp.class_is_generic)(klass) {
            continue;
        }
        let class_name = il2cpp.full_class_name(klass);
        if !targets.wants_class(&class_name) {
            continue;
        }
        let mut iter: *mut c_void = std::ptr::null_mut();
        loop {
            let method = (api.il2cpp_class_get_methods)(klass, &mut iter);
            if method.is_null() {
                break;
            }
            // MethodInfo.methodPointer is the first field. Null = abstract/generic.
            let addr = *(method as *const usize);
            if addr == 0 {
                continue;
            }
            let full = format!("{class_name}::{}", lossy((il2cpp.method_name)(method)));
            if !targets.wants_method(&full) {
                continue;
            }
            let label = format!("{full}({})", (il2cpp.method_param_count)(method));
            let signature = sig::signature(method);
            by_addr.entry(addr).or_insert_with(|| { order.push(addr); Vec::new() }).push((label, signature));
        }
    }

    let total = order.len();
    order.retain(|a| by_addr[a].len() <= MAX_SHARED_NAMES);
    let folded = total - order.len();
    let over_limit = order.len().saturating_sub(MAX_HOOKS);
    order.truncate(MAX_HOOKS);
    if order.is_empty() {
        return Err(format!("no methods matched {}", targets::FILE_NAME));
    }

    // 2. Publish the table before any hook exists; stubs index into it.
    let entries: Vec<Entry> = order
        .iter()
        .map(|a| Entry {
            name: by_addr[a].iter().map(|(label, _)| label.as_str()).collect::<Vec<_>>().join(" | "),
            trampoline: AtomicUsize::new(0),
            calls: AtomicU64::new(0),
        })
        .collect();
    if TABLE.set(entries.into_boxed_slice()).is_err() {
        return Err("hooks were already installed".to_owned());
    }
    let table = TABLE.get().expect("just set");

    // 3. Hook.
    let interceptor = (api.hachimi_get_interceptor)((api.hachimi_instance)());
    let mut report = String::new();
    for (i, addr) in order.iter().enumerate() {
        let trampoline = match stub::build(i as u32, enter) {
            Some(code) => (api.interceptor_hook)(interceptor, *addr as *mut c_void, code as *mut c_void) as usize,
            None => 0,
        };
        // 0 = MinHook refused: body too short, or Hachimi already hooks it.
        let state = if trampoline != 0 {
            table[i].trampoline.store(trampoline, Ordering::Release);
            HOOKED.fetch_add(1, Ordering::Relaxed);
            "ok"
        } else {
            FAILED.fetch_add(1, Ordering::Relaxed);
            "FAILED"
        };
        let sigs: Vec<&str> = by_addr[addr].iter().map(|(_, s)| s.as_str()).collect();
        let _ = writeln!(report, "{state}\t{}\t{}", table[i].name, sigs.join(" | "));
    }

    let header = format!(
        "# {} hooked, {} failed, {folded} skipped (shared code), {over_limit} skipped (over {MAX_HOOKS} limit)\n\
         # columns: ok/FAILED, method(arg count), signature\n",
        HOOKED.load(Ordering::Relaxed),
        FAILED.load(Ordering::Relaxed)
    );
    let _ = fs::write(dir.join("hooked_methods.txt"), header + &report);
    if over_limit > 0 {
        api.warn(&format!("{over_limit} methods skipped: narrow {} down", targets::FILE_NAME));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_into_parts_and_stops_at_the_limit() {
        let dir = std::env::temp_dir().join(format!("uma_trace_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trace_test.log");
        let out = BufWriter::new(File::create(&path).unwrap());
        let mut s =
            Session { out, path: path.clone(), part: 1, started: Instant::now(), lines: 0, max_lines: 25, part_lines: 10 };
        let written = (0..40).filter(|_| s.write_line(1, "A::b(0)", false)).count();
        s.out.flush().unwrap();
        drop(s);

        assert_eq!(written, 25);
        let lines = |p: &std::path::Path| fs::read_to_string(p).unwrap().lines().filter(|l| !l.starts_with('#')).count();
        assert_eq!(lines(&path), 10);
        assert_eq!(lines(&dir.join("trace_test_part2.log")), 10);
        assert_eq!(lines(&dir.join("trace_test_part3.log")), 5);
        assert!(!dir.join("trace_test_part4.log").exists());
        assert!(fs::read_to_string(&path).unwrap().contains("# continued in"));
        let _ = fs::remove_dir_all(&dir);
    }
}
