//! Main-thread side of the record cards.
//!
//! - Hook on `RaceViewReplay::SetupGateIn`: the moment the horses start loading
//!   into the gates, right after the fanfare (trace: ~23 s after
//!   `StateGateIn_Enter`, ~27 s before the start). Shows the cards.
//! - `tick` (clock's per-frame hook): publishes the race on screen for the
//!   menu's editor, every `PUBLISH_EVERY` frames.
//!
//! Race on screen: static `RaceManager.get_RaceInfo`, its ids, and its
//! `RaceCourseSet` (master `race_course_set` row: course id, distance, ground).

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use super::{Course, Ground, RaceNow};
use crate::il2cpp::{self, Method, Obj, GAME};
use crate::api;

const PUBLISH_EVERY: u32 = 30;

/// Offsets in the master `RaceCourseSet` row.
struct CourseFields {
    track: usize,
    distance: usize,
    ground: usize,
}

struct Game {
    race_info: Method,
    race_id: Method,
    instance_id: Method,
    course_set: Method,
    fields: Result<CourseFields, String>,
}

unsafe impl Send for Game {}
unsafe impl Sync for Game {}

static GAME_DATA: OnceLock<Result<Game, String>> = OnceLock::new();
static CURRENT: Mutex<Option<RaceNow>> = Mutex::new(None);
static STATUS: OnceLock<String> = OnceLock::new();

unsafe fn call0<R>(m: Method, this: Obj) -> R {
    let f: extern "C" fn(Obj, usize) -> R = std::mem::transmute(m.code);
    f(this, m.info)
}

/// The first of `names` that is a field of `klass`.
unsafe fn field(klass: *mut c_void, names: &[&str]) -> Result<usize, String> {
    names
        .iter()
        .find_map(|n| il2cpp::field_offset(klass, n).ok())
        .ok_or_else(|| format!("none of the fields {names:?}"))
}

impl Game {
    unsafe fn resolve() -> Result<Game, String> {
        use il2cpp::{class, expect, method, nested};
        let rm = class(GAME, "Gallop", "RaceManager")?;
        let info = class(GAME, "Gallop", "RaceInfo")?;
        let race_info = method(rm, "get_RaceInfo", 0)?;
        expect(race_info, true, &["Gallop.RaceInfo"], 0)?;
        let race_id = method(info, "get_RaceId", 0)?;
        expect(race_id, false, &["System.Int32"], 0)?;
        let instance_id = method(info, "get_RaceInstanceId", 0)?;
        expect(instance_id, false, &["System.Int32"], 0)?;
        let course_set = method(info, "get_RaceCourseSet", 0)?;
        expect(course_set, false, &["Gallop.MasterRaceCourseSet.RaceCourseSet"], 0)?;
        // Optional: without it only race records show.
        let fields = class(GAME, "Gallop", "MasterRaceCourseSet").and_then(|m| nested(m, "RaceCourseSet")).and_then(|k| {
            let found = (|| {
                Ok(CourseFields {
                    track: field(k, &["RaceTrackId", "raceTrackId", "_raceTrackId", "<RaceTrackId>k__BackingField"])?,
                    distance: field(k, &["Distance", "distance", "_distance", "<Distance>k__BackingField"])?,
                    ground: field(k, &["Ground", "ground", "_ground", "<Ground>k__BackingField"])?,
                })
            })();
            if found.is_err() {
                // The real member names, for the next fix.
                api::get().warn(&format!("record cards: RaceCourseSet members:\n{}", il2cpp::describe_class(k)));
            }
            found
        });
        Ok(Game { race_info, race_id, instance_id, course_set, fields })
    }

    unsafe fn now(&self) -> Option<RaceNow> {
        let f: extern "C" fn(usize) -> Obj = std::mem::transmute(self.race_info.code);
        let info = f(self.race_info.info);
        if info.is_null() {
            return None;
        }
        let instance: i32 = call0(self.instance_id, info);
        let race: i32 = call0(self.race_id, info);
        let course = self.fields.as_ref().ok().and_then(|f| {
            let cs: Obj = call0(self.course_set, info);
            if cs.is_null() {
                return None;
            }
            let id: i32 = il2cpp::read(cs, f.track);
            let distance: i32 = il2cpp::read(cs, f.distance);
            let ground = Ground::from_master(il2cpp::read(cs, f.ground))?;
            // Sanity: master course ids are 1xxxx, distances 800-4000 m.
            ((10_000..20_000).contains(&id) && (800..=4000).contains(&distance)).then_some(Course { id, ground, distance })
        });
        Some(RaceNow { instance, race, course })
    }
}

fn game() -> Option<&'static Game> {
    GAME_DATA
        .get_or_init(|| {
            let r = unsafe { Game::resolve() };
            match &r {
                Ok(g) => {
                    if let Err(e) = &g.fields {
                        api::get().warn(&format!("record cards: course not readable, track records off: {e}"));
                    }
                }
                Err(e) => api::get().error(&format!("record cards not available: {e}")),
            }
            r
        })
        .as_ref()
        .ok()
}

fn publish(now: Option<RaceNow>) {
    if now.is_some() {
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = now;
    }
}

/// The race on screen (or the last one), for the menu.
pub fn current() -> Option<RaceNow> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Once per race frame (main thread), from clock's AlterUpdate hook.
pub fn tick() {
    static FRAME: AtomicU32 = AtomicU32::new(0);
    if FRAME.fetch_add(1, Ordering::Relaxed) % PUBLISH_EVERY != 0 {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(g) = game() {
            publish(unsafe { g.now() });
        }
    }));
}

/// Shows this race's cards now (also the menu's preview: main thread only).
fn show_for_current_race() {
    let Some(g) = game() else { return };
    let now = unsafe { g.now() };
    publish(now.clone());
    if let Some(now) = now {
        super::show(super::cards_for(&super::records(), &now));
    }
}

// ---------------------------------------------------------------- hook

static ORIG_SETUP_GATE_IN: AtomicUsize = AtomicUsize::new(0);

/// `RaceViewReplay::SetupGateIn()`: gate loading begins.
extern "C" fn setup_gate_in(this: Obj, mi: *const c_void) {
    let orig = ORIG_SETUP_GATE_IN.load(Ordering::Acquire);
    if orig != 0 {
        let f: extern "C" fn(Obj, *const c_void) = unsafe { std::mem::transmute(orig) };
        f(this, mi);
    }
    if super::is_enabled() {
        let _ = catch_unwind(show_for_current_race);
    }
}

pub fn status() -> String {
    STATUS.get().cloned().unwrap_or_else(|| "waiting for the game".to_owned())
}

/// Called once when IL2CPP is ready.
pub fn install() {
    let result = unsafe {
        il2cpp::class(GAME, "Gallop", "RaceViewReplay")
            .and_then(|k| il2cpp::method(k, "SetupGateIn", 0))
            .and_then(|m| il2cpp::expect(m, false, &["System.Void"], 0).map(|_| m))
            .and_then(|m| il2cpp::hook_unique(m, setup_gate_in as *const () as usize))
    };
    let status = match result {
        Ok(t) => {
            ORIG_SETUP_GATE_IN.store(t, Ordering::Release);
            "ok".to_owned()
        }
        Err(e) => {
            api::get().error(&format!("record cards: cannot hook RaceViewReplay::SetupGateIn: {e}"));
            format!("FAILED: cannot hook RaceViewReplay::SetupGateIn ({e})")
        }
    };
    api::get().info(&format!("record cards: {status}"));
    let _ = STATUS.set(status);
}
