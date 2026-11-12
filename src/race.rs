//! Reads the current race field (order, gate, character, trainer) from the game.
//!
//! Everything here calls into IL2CPP, so it must run on the game's main thread:
//! `request_snapshot` schedules `take_snapshot` there, and the overlay reads
//! the result through `latest`. Getter return types are checked against the
//! metadata before any call, so a game update that changes a signature turns
//! into "field missing" instead of a crash.

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::api::{self, cstr};
use crate::{sig, tracer};

const ASSEMBLY: &str = "umamusume.dll";
/// Member names of these classes are listed in this file (written by `clock`).
const DUMP_FILE: &str = "race_classes.txt";

/// One runner, as shown on the leaderboard.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Runner {
    /// Gate (post) number, 1-based.
    pub gate: i32,
    /// Display position, 0 = leader (see `rank`). -1 if unknown.
    pub order: i32,
    /// Official finishing position from the race result (any base, lower is
    /// better), -1 if unknown. Only used once the runner has finished.
    pub finish_order: i32,
    /// Character id (e.g. 1001), 0 if unknown.
    pub chara_id: i32,
    /// Card (outfit) id (e.g. 100101), 0 if unknown.
    pub card_id: i32,
    /// Mob (generic NPC runner) id (e.g. 8001), 0 for named characters or if unknown.
    pub mob_id: i32,
    pub name: String,
    /// Owner's trainer name; empty for NPCs.
    pub trainer: String,
    pub finished: bool,
    /// Index in the game's horse array (HorseRaceInfo.HorseIndex), -1 if unknown.
    pub index: i32,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub runners: Vec<Runner>,
    pub taken: Instant,
}

static LATEST: Mutex<Option<Snapshot>> = Mutex::new(None);
static PENDING: AtomicBool = AtomicBool::new(false);
static LAST_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

/// The most recent snapshot, if one was taken within `max_age`.
pub fn latest(max_age: Duration) -> Option<Snapshot> {
    let guard = LATEST.lock().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().filter(|s| s.taken.elapsed() <= max_age).cloned()
}

/// Asks the main thread for a fresh snapshot, at most once per `interval`.
/// Safe to call from any thread, every frame.
pub fn request_snapshot(interval: Duration) {
    if !tracer::GAME_READY.load(Ordering::Acquire) {
        return;
    }
    {
        let mut last = LAST_REQUEST.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|t| t.elapsed() < interval) {
            return;
        }
        if PENDING.swap(true, Ordering::AcqRel) {
            return;
        }
        *last = Some(Instant::now());
    }
    let api = api::get();
    unsafe { (api.il2cpp_schedule_on_thread)((api.il2cpp_get_main_thread)(), on_main_thread) };
}

unsafe extern "C" fn on_main_thread() {
    let result = catch_unwind(AssertUnwindSafe(|| take_snapshot()));
    PENDING.store(false, Ordering::Release);
    let runners = match result {
        Ok(r) => r,
        Err(_) => {
            api::get().error("race snapshot panicked");
            None
        }
    };
    let mut guard = LATEST.lock().unwrap_or_else(|e| e.into_inner());
    *guard = runners.map(|runners| Snapshot { runners, taken: Instant::now() });
}

// ---------------------------------------------------------------- metadata

type GetPtr = unsafe extern "C" fn(this: *mut c_void, method: *const c_void) -> *mut c_void;
type GetI32 = unsafe extern "C" fn(this: *mut c_void, method: *const c_void) -> i32;
type GetBool = unsafe extern "C" fn(this: *mut c_void, method: *const c_void) -> bool;

/// A resolved instance getter: code address plus its MethodInfo (passed as
/// the hidden last argument, as IL2CPP-compiled code expects).
#[derive(Clone, Copy)]
struct Getter {
    addr: usize,
    method: *const c_void,
}

impl Getter {
    unsafe fn ptr(&self, this: *mut c_void) -> *mut c_void {
        std::mem::transmute::<usize, GetPtr>(self.addr)(this, self.method)
    }
    unsafe fn i32(&self, this: *mut c_void) -> i32 {
        std::mem::transmute::<usize, GetI32>(self.addr)(this, self.method)
    }
    unsafe fn bool(&self, this: *mut c_void) -> bool {
        std::mem::transmute::<usize, GetBool>(self.addr)(this, self.method)
    }
}

/// An `Int32` value that is either a getter or a plain field.
#[derive(Clone, Copy)]
enum IntSource {
    Getter(Getter),
    Field(*mut c_void),
}

impl IntSource {
    unsafe fn read(&self, this: *mut c_void) -> i32 {
        match self {
            IntSource::Getter(g) => g.i32(this),
            IntSource::Field(f) => {
                let mut v = 0i32;
                (api::get().il2cpp_get_field_value)(this, *f, &mut v as *mut i32 as *mut c_void);
                v
            }
        }
    }
}

struct Meta {
    race_manager: *mut c_void,
    horse_manager_field: *mut c_void,
    get_horse_race_infos: Getter,
    // HorseRaceInfo
    cur_order: Getter,
    horse_data: Getter,
    chara_name: Option<Getter>,
    is_finished: Option<Getter>,
    info_chara_id: Option<IntSource>,
    // HorseData
    gate_no: Getter,
    trainer_name: Option<Getter>,
    data_chara_name: Option<Getter>,
    /// HorseRaceInfoReplay (the runtime class of replayed runners) and its
    /// official finish order.
    replay_class: *mut c_void,
    finish_order: Option<Getter>,
    /// HorseRaceInfo._distance (Single): metres run.
    distance_field: Option<*mut c_void>,
    chara_id: Option<IntSource>,
    card_id: Option<IntSource>,
    mob_id: Option<IntSource>,
    info_mob_id: Option<IntSource>,
}

unsafe impl Send for Meta {}
unsafe impl Sync for Meta {}

static META: OnceLock<Result<Meta, String>> = OnceLock::new();

unsafe fn class(image: *const c_void, name: &str) -> Result<*mut c_void, String> {
    let k = (api::get().il2cpp_get_class)(image, cstr("Gallop").as_ptr(), cstr(name).as_ptr());
    if k.is_null() { Err(format!("class Gallop.{name} not found")) } else { Ok(k) }
}

/// Finds a 0-argument instance getter whose return type is `ret`
/// (`None` accepts any reference type).
unsafe fn getter(klass: *mut c_void, name: &str, ret: Option<&str>) -> Option<Getter> {
    let method = (api::get().il2cpp_get_method)(klass, cstr(name).as_ptr(), 0);
    if method.is_null() {
        return None;
    }
    let ty = sig::return_type(method)?;
    let ok = match ret {
        Some(want) => ty == want,
        // Pointer-returning getters: anything but value types we'd misread.
        None => !matches!(
            ty.as_str(),
            "System.Void" | "System.Boolean" | "System.Int32" | "System.Single" | "System.Int64" | "System.Double"
        ),
    };
    // MethodInfo.methodPointer is the first field.
    let addr = *(method as *const usize);
    (ok && addr != 0).then_some(Getter { addr, method })
}

unsafe fn required(klass: *mut c_void, class_name: &str, name: &str, ret: Option<&str>) -> Result<Getter, String> {
    getter(klass, name, ret).ok_or_else(|| {
        format!("Gallop.{class_name}::{name}() not found or has an unexpected return type")
    })
}

/// A `System.Single` field by name.
unsafe fn float_field(klass: *mut c_void, name: &str) -> Option<*mut c_void> {
    let api = api::get();
    let f = (api.il2cpp_get_field_from_name)(klass, cstr(name).as_ptr());
    if f.is_null() {
        return None;
    }
    let field_type: Option<unsafe extern "C" fn(*mut c_void) -> *const c_void> = api.il2cpp_fn("il2cpp_field_get_type");
    match field_type {
        Some(ft) if sig::type_name_of(ft(f)) == "System.Single" => Some(f),
        _ => None,
    }
}

/// First of `getters` (then `fields`) that exists and is an Int32.
unsafe fn int_source(klass: *mut c_void, getters: &[&str], fields: &[&str]) -> Option<IntSource> {
    for name in getters {
        if let Some(g) = getter(klass, name, Some("System.Int32")) {
            return Some(IntSource::Getter(g));
        }
    }
    let api = api::get();
    let field_type: Option<unsafe extern "C" fn(*mut c_void) -> *const c_void> =
        api.il2cpp_fn("il2cpp_field_get_type");
    for name in fields {
        let f = (api.il2cpp_get_field_from_name)(klass, cstr(name).as_ptr());
        if f.is_null() {
            continue;
        }
        if let Some(field_type) = field_type {
            if sig::type_name_of(field_type(f)) != "System.Int32" {
                continue;
            }
        }
        return Some(IntSource::Field(f));
    }
    None
}

unsafe fn resolve() -> Result<Meta, String> {
    let api = api::get();
    let image = (api.il2cpp_get_assembly_image)(cstr(ASSEMBLY).as_ptr());
    if image.is_null() {
        return Err(format!("assembly not found: {ASSEMBLY}"));
    }
    let race_manager = class(image, "RaceManager")?;
    let horse_manager = class(image, "RaceHorseManagerBase")?;
    let info = class(image, "HorseRaceInfo")?;
    let replay = class(image, "HorseRaceInfoReplay").unwrap_or(std::ptr::null_mut());
    let data = class(image, "HorseData")?;

    let horse_manager_field = (api.il2cpp_get_field_from_name)(race_manager, cstr("_horseManager").as_ptr());
    if horse_manager_field.is_null() {
        return Err("field Gallop.RaceManager::_horseManager not found".to_owned());
    }

    let meta = Meta {
        race_manager,
        horse_manager_field,
        get_horse_race_infos: required(horse_manager, "RaceHorseManagerBase", "GetHorseRaceInfos", None)?,
        cur_order: required(info, "HorseRaceInfo", "get_CurOrder", Some("System.Int32"))?,
        horse_data: required(info, "HorseRaceInfo", "get_HorseData", None)?,
        chara_name: getter(info, "get_CharaName", Some("System.String")),
        is_finished: getter(info, "IsFinished", Some("System.Boolean")),
        info_chara_id: int_source(info, &["get_CharaId", "get_charaId"], &[]),
        info_mob_id: int_source(info, &["get_MobId", "get_mobId"], &[]),
        gate_no: required(data, "HorseData", "get_GateNo", Some("System.Int32"))?,
        trainer_name: getter(data, "get_TrainerName", Some("System.String")),
        data_chara_name: getter(data, "get_charaName", Some("System.String")),
        replay_class: replay,
        finish_order: if replay.is_null() { None } else { getter(replay, "get_FinishOrder", Some("System.Int32")) },
        distance_field: float_field(info, "_distance"),
        chara_id: int_source(data, &["get_charaId", "get_CharaId", "get_CharaID"], &["charaId", "_charaId", "CharaId"]),
        card_id: int_source(data, &["get_cardId", "get_CardId", "get_CardID"], &["cardId", "_cardId", "CardId"]),
        mob_id: int_source(data, &["get_mobId", "get_MobId", "get_MobID"], &["mobId", "_mobId", "MobId"]),
    };

    let missing: Vec<&str> = [
        ("character name", meta.chara_name.is_some() || meta.data_chara_name.is_some()),
        ("trainer name", meta.trainer_name.is_some()),
        ("character id", meta.chara_id.is_some() || meta.info_chara_id.is_some()),
        ("card id", meta.card_id.is_some()),
        ("mob id", meta.mob_id.is_some() || meta.info_mob_id.is_some()),
        ("finished flag", meta.is_finished.is_some()),
        ("finish order", meta.finish_order.is_some()),
    ]
    .into_iter()
    .filter(|(_, found)| !found)
    .map(|(what, _)| what)
    .collect();
    if !missing.is_empty() {
        api.warn(&format!("leaderboard: not found in game data: {} (see {DUMP_FILE})", missing.join(", ")));
    }
    Ok(meta)
}

// ---------------------------------------------------------------- snapshot

/// Most runners a race can have.
pub const MAX_RUNNERS: usize = 18;

/// Element pointers of a managed reference-type array.
unsafe fn array_items(array: *mut c_void) -> &'static [*mut c_void] {
    if array.is_null() {
        return &[];
    }
    // Il2CppArray (64-bit): object header (16), bounds (8), max_length (8), then the data.
    let len = *((array as usize + 24) as *const usize);
    if len > 64 {
        return &[]; // a race has at most 18 runners; anything else is not what we think
    }
    std::slice::from_raw_parts((array as usize + 32) as *const *mut c_void, len)
}

/// Main thread only. None when no race is loaded.
unsafe fn take_snapshot() -> Option<Vec<Runner>> {
    let meta = match META.get_or_init(|| resolve()) {
        Ok(m) => m,
        Err(e) => {
            static REPORTED: AtomicBool = AtomicBool::new(false);
            if !REPORTED.swap(true, Ordering::Relaxed) {
                api::get().error(&format!("leaderboard disabled: {e}"));
            }
            return None;
        }
    };
    let api = api::get();

    let race_manager = (api.il2cpp_get_singleton_like_instance)(meta.race_manager);
    if race_manager.is_null() {
        return None;
    }
    let mut horse_manager: *mut c_void = std::ptr::null_mut();
    (api.il2cpp_get_field_value)(
        race_manager,
        meta.horse_manager_field,
        &mut horse_manager as *mut *mut c_void as *mut c_void,
    );
    if horse_manager.is_null() {
        return None;
    }
    let infos = array_items(meta.get_horse_race_infos.ptr(horse_manager));
    if infos.is_empty() {
        return None;
    }

    let course = crate::clock::course_distance();
    let mut runners = Vec::with_capacity(MAX_RUNNERS);
    for (index, &info) in infos.iter().enumerate().take(MAX_RUNNERS) {
        if info.is_null() {
            continue;
        }
        let data = meta.horse_data.ptr(info);
        if data.is_null() {
            continue;
        }
        let name = match meta.chara_name.map(|g| api.string(g.ptr(info))).filter(|s| !s.is_empty()) {
            Some(n) => n,
            None => meta.data_chara_name.map(|g| api.string(g.ptr(data))).unwrap_or_default(),
        };
        let chara_id = match meta.chara_id {
            Some(src) => src.read(data),
            None => meta.info_chara_id.map(|src| src.read(info)).unwrap_or(0),
        };
        runners.push(Runner {
            gate: meta.gate_no.i32(data),
            order: meta.cur_order.i32(info),
            chara_id,
            card_id: meta.card_id.map(|src| src.read(data)).unwrap_or(0),
            mob_id: match meta.mob_id {
                Some(src) => src.read(data),
                None => meta.info_mob_id.map(|src| src.read(info)).unwrap_or(0),
            },
            name,
            trainer: meta.trainer_name.map(|g| api.string(g.ptr(data))).unwrap_or_default(),
            finished: meta.is_finished.map(|g| g.bool(info)).unwrap_or(false)
                || crossed_line(meta.distance_field.map(|f| read_f32(info, f)), course),
            finish_order: match meta.finish_order {
                Some(g) if crate::il2cpp::is_instance(info, meta.replay_class) => g.i32(info),
                _ => -1,
            },
            index: index as i32,
        });
    }
    if runners.is_empty() {
        return None;
    }
    normalize_gates(&mut runners);
    rank(&mut runners);
    Some(runners)
}

unsafe fn read_f32(obj: *mut c_void, field: *mut c_void) -> f32 {
    let mut v = 0f32;
    (api::get().il2cpp_get_field_value)(obj, field, &mut v as *mut f32 as *mut c_void);
    v
}

/// The runner has reached the finish line (distance run >= course length).
pub fn crossed_line(distance: Option<f32>, course: Option<f32>) -> bool {
    match (distance, course) {
        (Some(d), Some(c)) => d.is_finite() && c > 0.0 && d >= c,
        _ => false,
    }
}

/// Sets each runner's display `order` (0 = leader):
/// 1. runners who have crossed the line, by their official finishing position
///    (fixed from then on: they keep running and passing each other after
///    the line, which the live track position would show);
/// 2. runners still racing, by live track position.
pub fn rank(runners: &mut [Runner]) {
    let key = |r: &Runner| {
        let official = r.finished && r.finish_order >= 0;
        // (group, position within group, gate as a stable tie-break)
        let group = if official { 0 } else if r.finished { 1 } else { 2 };
        let pos = if official { r.finish_order } else if r.order >= 0 { r.order } else { i32::MAX };
        (group, pos, r.gate)
    };
    let mut idx: Vec<usize> = (0..runners.len()).collect();
    idx.sort_by_key(|&i| key(&runners[i]));
    for (place, i) in idx.into_iter().enumerate() {
        runners[i].order = place as i32;
    }
}

/// Makes gate numbers 1-based if the game reports them 0-based.
pub fn normalize_gates(runners: &mut [Runner]) {
    if runners.iter().any(|r| r.gate == 0) {
        for r in runners.iter_mut() {
            r.gate += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_based_gates_are_shifted() {
        let mut rs = vec![Runner { gate: 0, ..Default::default() }, Runner { gate: 1, ..Default::default() }];
        normalize_gates(&mut rs);
        assert_eq!((rs[0].gate, rs[1].gate), (1, 2));

        let mut rs = vec![Runner { gate: 1, ..Default::default() }, Runner { gate: 2, ..Default::default() }];
        normalize_gates(&mut rs);
        assert_eq!((rs[0].gate, rs[1].gate), (1, 2));
    }

    fn r(gate: i32, live: i32, finished: bool, finish: i32) -> Runner {
        Runner { gate, order: live, finished, finish_order: finish, ..Default::default() }
    }

    fn by_place(rs: &[Runner]) -> Vec<i32> {
        let mut v: Vec<&Runner> = rs.iter().collect();
        v.sort_by_key(|r| r.order);
        v.iter().map(|r| r.gate).collect()
    }

    #[test]
    fn finished_runners_keep_their_official_place() {
        // The screenshot case: after the line gate 1 ran past 17 and 18 on the
        // track, but the result is 17, 18, 3, 1, 2.
        let mut rs = vec![
            r(1, 0, true, 3), r(17, 1, true, 0), r(18, 2, true, 1), r(3, 3, true, 2), r(2, 4, true, 4),
        ];
        rank(&mut rs);
        assert_eq!(by_place(&rs), vec![17, 18, 3, 1, 2]);
    }

    #[test]
    fn finishers_ahead_of_runners_still_racing() {
        // 5 and 6 have finished (6 won); 7 is still racing but "ahead" on the
        // track number line, 8 is behind it.
        let mut rs = vec![r(5, 2, true, 1), r(6, 0, true, 0), r(7, 1, false, 5), r(8, 3, false, 2)];
        rank(&mut rs);
        assert_eq!(by_place(&rs), vec![6, 5, 7, 8]);
    }

    #[test]
    fn before_anyone_finishes_live_order_is_used() {
        // The precomputed finish order must not leak before the line.
        let mut rs = vec![r(1, 1, false, 0), r(2, 0, false, 1), r(3, 2, false, 2)];
        rank(&mut rs);
        assert_eq!(by_place(&rs), vec![2, 1, 3]);
    }

    #[test]
    fn unknown_finish_order_falls_back_to_live_order() {
        let mut rs = vec![r(1, 1, true, -1), r(2, 0, true, -1), r(3, 0, false, -1)];
        rank(&mut rs);
        assert_eq!(by_place(&rs), vec![2, 1, 3]);
    }

    #[test]
    fn line_crossing() {
        assert!(crossed_line(Some(2400.0), Some(2400.0)));
        assert!(crossed_line(Some(2431.5), Some(2400.0)));
        assert!(!crossed_line(Some(2399.9), Some(2400.0)));
        assert!(!crossed_line(Some(2500.0), None), "course unknown");
        assert!(!crossed_line(None, Some(2400.0)));
        assert!(!crossed_line(Some(f32::NAN), Some(2400.0)));
    }

    #[test]
    fn snapshot_age_filter() {
        *LATEST.lock().unwrap() = Some(Snapshot { runners: vec![], taken: Instant::now() });
        assert!(latest(Duration::from_secs(1)).is_some());
        *LATEST.lock().unwrap() = Some(Snapshot {
            runners: vec![],
            taken: Instant::now() - Duration::from_secs(5),
        });
        assert!(latest(Duration::from_secs(1)).is_none());
    }
}
