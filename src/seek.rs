//! Seeking a race to a given time, following Hachimi Edge's own race
//! playback slider (`RaceManagerReplayBase::seek_sync` in kairusds/Hachimi-Edge):
//! setting the race time alone leaves horses, models, cameras and the
//! commentary out of step, so every one of them is brought to the new time.
//!
//! Main thread only. On Windows the whole seek runs under an SEH guard, as
//! Hachimi does: a fault inside the game during a seek is logged and the
//! seek reported as failed instead of taking the game down.
//! Steps the game does not have (after an update) are skipped.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::il2cpp::{self, Method, Obj};
use crate::api;

const GAME: &str = "umamusume.dll";

struct Steps {
    // RaceManagerReplayBase
    force_set_race_time: Method,
    update_horses: Option<Method>,
    update_horse_datas: Option<Method>,
    update_horse_models: Option<Method>,
    update_camera_events: Option<Method>,
    late_update_camera_events: Option<Method>,
    is_cut_in_playing_or_reserved: Option<Method>,
    get_jikkyo: Option<Method>,
    event_player_off: Option<usize>,
    // RaceEventPlayer
    event_player_class: *mut c_void,
    change_last_event_index_by_time: Option<Method>,
    // Jikkyo / JikkyoControllerBase
    skip_to_start: Option<Method>,
    clear_display: Option<Method>,
    clear_voice: Option<Method>,
    clear_reserve: Option<Method>,
    // RaceManager views
    get_race_view: Option<Method>,
    last_spurt_processed_off: Option<usize>,
    get_race_main_view: Option<Method>,
    race_ui_off: Option<usize>,
    minimap_off: Option<usize>,
    minimap_shown_off: Option<usize>,
    minimap_hidden_off: Option<usize>,
}

unsafe impl Send for Steps {}
unsafe impl Sync for Steps {}

static STEPS: OnceLock<Result<Steps, String>> = OnceLock::new();
/// Last step reached, for the log when a seek faults.
static STAGE: AtomicUsize = AtomicUsize::new(0);

/// Resolves everything once (main thread). Returns why seeking is
/// unavailable, if it is.
pub fn install() -> Result<(), String> {
    match STEPS.get_or_init(|| unsafe { resolve() }) {
        Ok(_) => Ok(()),
        Err(e) => Err(e.clone()),
    }
}

unsafe fn resolve() -> Result<Steps, String> {
    use il2cpp::{class, field_offset, method};
    let rm = class(GAME, "Gallop", "RaceManagerReplayBase")?;
    let race_manager = class(GAME, "Gallop", "RaceManager")?;
    let opt_class = |name: &str| class(GAME, "Gallop", name).ok();
    let event_player = opt_class("RaceEventPlayer");
    let jikkyo = opt_class("Jikkyo");
    let jikkyo_base = opt_class("JikkyoControllerBase");
    let view = opt_class("RaceViewReplay");
    let main_view = opt_class("RaceMainViewController");
    let race_ui = opt_class("RaceUI");
    let minimap = opt_class("RaceUIMiniMap");

    let m = |k: Option<*mut c_void>, name: &str, argc: u32| k.and_then(|k| method(k, name, argc).ok());
    let f = |k: Option<*mut c_void>, name: &str| k.and_then(|k| field_offset(k, name).ok());

    let steps = Steps {
        force_set_race_time: method(rm, "ForceSetRaceTime", 2)?,
        update_horses: m(Some(rm), "UpdateHorses", 1),
        update_horse_datas: m(Some(rm), "UpdateHorseDatas", 1),
        update_horse_models: m(Some(rm), "UpdateHorseModels", 0),
        update_camera_events: m(Some(rm), "UpdateCameraEventAll", 1),
        late_update_camera_events: m(Some(rm), "LateUpdateCameraEventAll", 1),
        is_cut_in_playing_or_reserved: m(Some(rm), "IsCutInPlayingOrReserved", 0),
        get_jikkyo: m(Some(rm), "get_Jikkyo", 0),
        event_player_off: f(Some(rm), "_eventPlayer"),
        event_player_class: event_player.unwrap_or(std::ptr::null_mut()),
        change_last_event_index_by_time: m(event_player, "ChangeLastEventIndexByTime", 1),
        skip_to_start: m(jikkyo, "SkipToStart", 0),
        clear_display: m(jikkyo_base, "ClearDisplay", 0),
        clear_voice: m(jikkyo_base, "ClearVoice", 0),
        clear_reserve: m(jikkyo_base, "ClearReserve", 0),
        get_race_view: m(Some(race_manager), "get_RaceView", 0),
        last_spurt_processed_off: f(view, "_lastSpurtProcessed"),
        get_race_main_view: m(Some(race_manager), "get_RaceMainView", 0),
        race_ui_off: f(main_view, "_raceUI"),
        minimap_off: f(race_ui, "_minimap"),
        minimap_shown_off: f(minimap, "_hasMiniMapShown"),
        minimap_hidden_off: f(minimap, "_hasMiniMapHidden"),
    };

    let missing: Vec<&str> = [
        ("UpdateHorses", steps.update_horses.is_some()),
        ("UpdateHorseDatas", steps.update_horse_datas.is_some()),
        ("UpdateHorseModels", steps.update_horse_models.is_some()),
        ("camera events", steps.update_camera_events.is_some() && steps.late_update_camera_events.is_some()),
        ("event cursor", steps.change_last_event_index_by_time.is_some() && steps.event_player_off.is_some()),
        ("commentary", steps.get_jikkyo.is_some()),
    ]
    .into_iter()
    .filter(|(_, ok)| !ok)
    .map(|(n, _)| n)
    .collect();
    if !missing.is_empty() {
        api::get().warn(&format!("race seek: skipping steps the game lacks: {}", missing.join(", ")));
    }
    Ok(steps)
}

// Typed calls; `mi` is the hidden MethodInfo argument.
unsafe fn call0(m: Method, this: Obj) {
    let f: extern "C" fn(Obj, usize) = std::mem::transmute(m.code);
    f(this, m.info)
}
unsafe fn call0_obj(m: Method, this: Obj) -> Obj {
    let f: extern "C" fn(Obj, usize) -> Obj = std::mem::transmute(m.code);
    f(this, m.info)
}
unsafe fn call0_bool(m: Method, this: Obj) -> bool {
    let f: extern "C" fn(Obj, usize) -> bool = std::mem::transmute(m.code);
    f(this, m.info)
}
unsafe fn call_f32(m: Method, this: Obj, v: f32) {
    let f: extern "C" fn(Obj, f32, usize) = std::mem::transmute(m.code);
    f(this, v, m.info)
}
unsafe fn call_bool(m: Method, this: Obj, v: bool) {
    let f: extern "C" fn(Obj, bool, usize) = std::mem::transmute(m.code);
    f(this, v, m.info)
}
unsafe fn call_f32_bool(m: Method, this: Obj, v: f32, b: bool) {
    let f: extern "C" fn(Obj, f32, bool, usize) = std::mem::transmute(m.code);
    f(this, v, b, m.info)
}
unsafe fn write_bool(obj: Obj, off: usize, v: bool) {
    *((obj as *mut u8).add(off) as *mut bool) = v;
}

/// The game wants no seeking right now (skill cut-in playing or about to).
pub fn busy(rm: Obj) -> bool {
    match STEPS.get() {
        Some(Ok(s)) => s.is_cut_in_playing_or_reserved.is_some_and(|m| unsafe { call0_bool(m, rm) }),
        _ => true,
    }
}

/// Brings the race to `target` (seconds). `backward` = earlier than now.
/// Returns false if seeking is unavailable or faulted (logged).
pub fn seek(rm: Obj, target: f32, backward: bool) -> bool {
    let Some(Ok(s)) = STEPS.get() else { return false };
    if rm.is_null() {
        return false;
    }
    guarded(|| unsafe { run(s, rm, target, backward) })
}

fn stage(n: usize) {
    STAGE.store(n, Ordering::Relaxed);
}

unsafe fn run(s: &Steps, rm: Obj, target: f32, backward: bool) {
    // 1. Event cursor (skills, commentary triggers) to the new time.
    stage(1);
    if let (Some(off), Some(m)) = (s.event_player_off, s.change_last_event_index_by_time) {
        let player: Obj = il2cpp::read(rm, off);
        if !player.is_null() && il2cpp::is_instance(player, s.event_player_class) {
            call_f32(m, player, target);
        }
    }
    // 2-5. Race time, horses, their data and models.
    stage(2);
    call_f32_bool(s.force_set_race_time, rm, target, true);
    stage(3);
    if let Some(m) = s.update_horses {
        call_f32(m, rm, target);
    }
    stage(4);
    if let Some(m) = s.update_horse_datas {
        call_f32(m, rm, target);
    }
    stage(5);
    if let Some(m) = s.update_horse_models {
        call0(m, rm);
    }
    // 6-7. Cameras follow the new positions.
    stage(6);
    if let Some(m) = s.update_camera_events {
        call_bool(m, rm, true);
    }
    stage(7);
    if let Some(m) = s.late_update_camera_events {
        call_bool(m, rm, true);
    }
    // 8. Commentary: restart when going back, drop what is queued when going forward.
    stage(8);
    let jikkyo = s.get_jikkyo.map(|m| call0_obj(m, rm)).unwrap_or(std::ptr::null_mut());
    if !jikkyo.is_null() {
        if backward {
            if let Some(m) = s.skip_to_start {
                call0(m, jikkyo);
            }
        } else {
            for m in [s.clear_display, s.clear_voice, s.clear_reserve].into_iter().flatten() {
                call0(m, jikkyo);
            }
        }
    }
    // 9. Let the view replay the last-spurt effect when it is reached again.
    stage(9);
    if let (Some(get_view), Some(off)) = (s.get_race_view, s.last_spurt_processed_off) {
        let view = call0_obj(get_view, rm);
        if !view.is_null() {
            write_bool(view, off, false);
        }
    }
    // 10. Going back: the minimap may show/hide again.
    if backward {
        stage(10);
        if let (Some(get_main), Some(ui_off), Some(map_off), Some(shown), Some(hidden)) = (
            s.get_race_main_view,
            s.race_ui_off,
            s.minimap_off,
            s.minimap_shown_off,
            s.minimap_hidden_off,
        ) {
            let main = call0_obj(get_main, rm);
            let race_ui: Obj = if main.is_null() { std::ptr::null_mut() } else { il2cpp::read(main, ui_off) };
            let map: Obj = if race_ui.is_null() { std::ptr::null_mut() } else { il2cpp::read(race_ui, map_off) };
            if !map.is_null() {
                write_bool(map, shown, false);
                write_bool(map, hidden, false);
            }
        }
    }
    stage(0);
}

#[cfg(windows)]
fn guarded(f: impl FnMut()) -> bool {
    match microseh::try_seh(f) {
        Ok(()) => true,
        Err(e) => {
            api::get().error(&format!(
                "race seek faulted at step {}: {} (seek skipped)",
                STAGE.load(Ordering::Relaxed),
                e.code()
            ));
            false
        }
    }
}

#[cfg(not(windows))]
fn guarded(mut f: impl FnMut()) -> bool {
    f();
    true
}
