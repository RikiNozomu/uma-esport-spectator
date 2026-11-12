//! Main-thread side of the seek bar: reads the replay, applies seeks.
//!
//! `tick` runs once per race frame from clock's `RaceManagerReplayBase::AlterUpdate`
//! hook (after the original, `this` = race manager); it keeps running while
//! the race is paused. Every method is resolved once and its signature
//! checked before the first call. Optional steps (commentary, minimap,
//! last-spurt flag) are skipped if a game update removes them.
//!
//! The seek follows Hachimi Edge's `seek_sync`: move the event cursor, force
//! the race time, update horses / horse data / models / cameras, then reset
//! the commentary and the view flags that only fire once. Hachimi runs it
//! under an SEH guard; we cannot, so the same calls are only made with
//! checked, non-null objects.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use super::Replay;
use crate::il2cpp::{self, Method, Obj, GAME};
use crate::settings::{self, SEEK_BAR};
use crate::{api, race_ui};

/// A backward jump bigger than this resets the commentary and minimap.
const BACKWARD: f32 = 0.5;

struct Game {
    // RaceManager / RaceManagerReplayBase
    horse_manager_off: usize,
    is_paused: Method,
    pause: Method,
    resume: Method,
    playing_cut_in: Method,
    cut_in_playing_or_reserved: Method,
    force_set_race_time: Method,
    update_horses: Method,
    update_horse_datas: Method,
    update_horse_models: Method,
    update_camera_events: Method,
    late_update_camera_events: Method,
    // Replay data
    replay_manager_class: usize,
    reader_off: usize,
    cur_time_off: usize,
    last_frame_time: Method,
    // Start dash (Hachimi's gate): the player's horse, else horse 0.
    player_index: Method,
    horse_infos: Method,
    is_start_dash: Method,
    // Optional steps.
    event_player: Option<(usize, usize, Method)>,
    jikkyo: Option<(Method, Method, Method, Method, Method)>,
    last_spurt: Option<(Method, usize, usize)>,
    /// (get_RaceMainView, _raceUI, RaceUI class, _minimap, RaceUIMiniMap class, shown, hidden)
    minimap: Option<(Method, usize, usize, usize, usize, usize, usize)>,
}

unsafe impl Send for Game {}
unsafe impl Sync for Game {}

unsafe fn getter(k: *mut c_void, name: &str, ret: &[&str]) -> Result<Method, String> {
    let m = il2cpp::method(k, name, 0)?;
    il2cpp::expect(m, false, ret, 0).map_err(|e| format!("{name}: {e}"))?;
    Ok(m)
}

unsafe fn void_method(k: *mut c_void, name: &str, params: &[&str]) -> Result<Method, String> {
    let m = il2cpp::method_params(k, name, params)?;
    il2cpp::expect(m, false, &["System.Void"], params.len() as u32).map_err(|e| format!("{name}: {e}"))?;
    Ok(m)
}

fn optional<T>(what: &str, r: Result<T, String>) -> Option<T> {
    r.map_err(|e| api::get().warn(&format!("seek bar: {what} not reset after seeking ({e})"))).ok()
}

impl Game {
    unsafe fn resolve() -> Result<Game, String> {
        use il2cpp::{class, field_offset};
        let rm = class(GAME, "Gallop", "RaceManager")?;
        let rm_replay = class(GAME, "Gallop", "RaceManagerReplayBase")?;
        let hm_base = class(GAME, "Gallop", "RaceHorseManagerBase")?;
        let hm_replay = class(GAME, "Gallop", "RaceHorseManagerReplay")?;
        let reader = class(GAME, "Gallop", "RaceSimulateReader")?;
        let horse = class(GAME, "Gallop", "HorseRaceInfo")?;
        const F32: &str = "System.Single";
        const BOOL: &str = "System.Boolean";

        let horse_infos = il2cpp::method(hm_base, "GetHorseRaceInfos", 0)?;
        if !il2cpp::return_type(horse_infos).ends_with("[]") {
            return Err("GetHorseRaceInfos does not return an array".to_owned());
        }

        let event_player = optional("race events", (|| {
            let ep = class(GAME, "Gallop", "RaceEventPlayer")?;
            let change = void_method(ep, "ChangeLastEventIndexByTime", &[F32])?;
            Ok((field_offset(rm_replay, "_eventPlayer")?, ep as usize, change))
        })());
        let jikkyo = optional("commentary", (|| {
            let j = class(GAME, "Gallop", "Jikkyo")?;
            let get = il2cpp::method(rm_replay, "get_Jikkyo", 0)?;
            il2cpp::expect(get, false, &["Gallop.Jikkyo"], 0)?;
            Ok((
                get,
                void_method(j, "SkipToStart", &[])?,
                void_method(j, "ClearDisplay", &[])?,
                void_method(j, "ClearVoice", &[])?,
                void_method(j, "ClearReserve", &[])?,
            ))
        })());
        let last_spurt = optional("last-spurt effect", (|| {
            let view = class(GAME, "Gallop", "RaceViewReplay")?;
            let get = il2cpp::method(rm, "get_RaceView", 0)?;
            il2cpp::expect(get, false, &["Gallop.IRaceView"], 0)?;
            Ok((get, view as usize, field_offset(view, "_lastSpurtProcessed")?))
        })());
        let minimap = optional("minimap", (|| {
            let main_view = class(GAME, "Gallop", "RaceMainViewController")?;
            let race_ui = class(GAME, "Gallop", "RaceUI")?;
            let map = class(GAME, "Gallop", "RaceUIMiniMap")?;
            let get = il2cpp::method(rm, "get_RaceMainView", 0)?;
            il2cpp::expect(get, false, &["Gallop.RaceMainViewController"], 0)?;
            Ok((
                get,
                field_offset(main_view, "_raceUI")?,
                race_ui as usize,
                field_offset(race_ui, "_minimap")?,
                map as usize,
                field_offset(map, "_hasMiniMapShown")?,
                field_offset(map, "_hasMiniMapHidden")?,
            ))
        })());

        Ok(Game {
            horse_manager_off: field_offset(rm, "_horseManager")?,
            is_paused: getter(rm_replay, "IsPaused", &[BOOL])?,
            pause: void_method(rm_replay, "PauseRace", &[])?,
            resume: void_method(rm_replay, "ResumeRace", &[])?,
            playing_cut_in: getter(rm_replay, "get_IsPlayingCutIn", &[BOOL])?,
            cut_in_playing_or_reserved: getter(rm_replay, "IsCutInPlayingOrReserved", &[BOOL])?,
            force_set_race_time: void_method(rm_replay, "ForceSetRaceTime", &[F32, BOOL])?,
            update_horses: void_method(rm_replay, "UpdateHorses", &[F32])?,
            update_horse_datas: void_method(rm_replay, "UpdateHorseDatas", &[F32])?,
            update_horse_models: void_method(rm_replay, "UpdateHorseModels", &[])?,
            update_camera_events: void_method(rm_replay, "UpdateCameraEventAll", &[BOOL])?,
            late_update_camera_events: void_method(rm_replay, "LateUpdateCameraEventAll", &[BOOL])?,
            replay_manager_class: hm_replay as usize,
            reader_off: field_offset(hm_replay, "_reader")?,
            cur_time_off: field_offset(reader, "_curTime")?,
            last_frame_time: getter(reader, "GetLastFrameTime", &[F32])?,
            player_index: getter(hm_base, "GetPlayerHorseIndex", &["System.Int32"])?,
            horse_infos,
            is_start_dash: getter(horse, "get_IsStartDash", &[BOOL])?,
            event_player,
            jikkyo,
            last_spurt,
            minimap,
        })
    }

    unsafe fn call0<T>(&self, m: Method, this: Obj) -> T {
        let f: extern "C" fn(Obj, usize) -> T = std::mem::transmute(m.code);
        f(this, m.info)
    }

    unsafe fn bool0(&self, m: Method, this: Obj) -> bool {
        self.call0::<u8>(m, this) != 0
    }

    unsafe fn void0(&self, m: Method, this: Obj) {
        self.call0::<()>(m, this)
    }

    unsafe fn void_f32(&self, m: Method, this: Obj, v: f32) {
        let f: extern "C" fn(Obj, f32, usize) = std::mem::transmute(m.code);
        f(this, v, m.info)
    }

    unsafe fn void_bool(&self, m: Method, this: Obj, v: bool) {
        let f: extern "C" fn(Obj, bool, usize) = std::mem::transmute(m.code);
        f(this, v, m.info)
    }

    /// The replay reader, if this race manager plays a replay.
    unsafe fn reader(&self, rm: Obj) -> Option<Obj> {
        let hm: Obj = il2cpp::read(rm, self.horse_manager_off);
        if !il2cpp::is_instance(hm, self.replay_manager_class as *mut c_void) {
            return None;
        }
        let reader: Obj = il2cpp::read(hm, self.reader_off);
        (!reader.is_null()).then_some(reader)
    }

    /// Hachimi's start-dash gate: the player's horse (horse 0 without one).
    unsafe fn start_dash(&self, rm: Obj) -> bool {
        let hm: Obj = il2cpp::read(rm, self.horse_manager_off);
        if hm.is_null() {
            return false;
        }
        let items = il2cpp::array_items(self.call0::<Obj>(self.horse_infos, hm));
        let idx = self.call0::<i32>(self.player_index, hm);
        let idx = if idx >= 0 && (idx as usize) < items.len() { idx as usize } else { 0 };
        items.get(idx).is_some_and(|&h| !h.is_null() && self.bool0(self.is_start_dash, h))
    }

    unsafe fn seek(&self, rm: Obj, reader: Obj, t: f32) {
        let prev: f32 = il2cpp::read(reader, self.cur_time_off);
        let backward = t < prev - BACKWARD;

        if let Some((off, class, change)) = self.event_player {
            let ep: Obj = il2cpp::read(rm, off);
            if il2cpp::is_instance(ep, class as *mut c_void) {
                self.void_f32(change, ep, t);
            }
        }
        let f: extern "C" fn(Obj, f32, bool, usize) = std::mem::transmute(self.force_set_race_time.code);
        f(rm, t, true, self.force_set_race_time.info);
        self.void_f32(self.update_horses, rm, t);
        self.void_f32(self.update_horse_datas, rm, t);
        self.void0(self.update_horse_models, rm);
        self.void_bool(self.update_camera_events, rm, true);
        self.void_bool(self.late_update_camera_events, rm, true);

        if let Some((get, skip_to_start, clear_display, clear_voice, clear_reserve)) = self.jikkyo {
            let j: Obj = self.call0(get, rm);
            if !j.is_null() {
                if backward {
                    self.void0(skip_to_start, j);
                } else {
                    self.void0(clear_display, j);
                    self.void0(clear_voice, j);
                    self.void0(clear_reserve, j);
                }
            }
        }
        if let Some((get, class, off)) = self.last_spurt {
            let view: Obj = self.call0(get, rm);
            if il2cpp::is_instance(view, class as *mut c_void) {
                write::<u8>(view, off, 0);
            }
        }
        if backward {
            if let Some((get, race_ui_off, race_ui_class, minimap_off, map_class, shown_off, hidden_off)) = self.minimap {
                let main_view: Obj = self.call0(get, rm);
                let race_ui: Obj = if main_view.is_null() { std::ptr::null_mut() } else { il2cpp::read(main_view, race_ui_off) };
                // `_raceUI` is an interface: only read RaceUI's field from a RaceUI.
                let map: Obj = if il2cpp::is_instance(race_ui, race_ui_class as *mut c_void) {
                    il2cpp::read(race_ui, minimap_off)
                } else {
                    std::ptr::null_mut()
                };
                if il2cpp::is_instance(map, map_class as *mut c_void) {
                    write::<u8>(map, shown_off, 0);
                    write::<u8>(map, hidden_off, 0);
                }
            }
        }
    }
}

unsafe fn write<T: Copy>(obj: Obj, offset: usize, v: T) {
    std::ptr::write_unaligned((obj as *mut u8).add(offset) as *mut T, v)
}

// ---------------------------------------------------------------- state

static GAME_DATA: OnceLock<Result<Game, String>> = OnceLock::new();

/// Race this state belongs to: race manager and race_ui::race_gen().
static RACE: AtomicUsize = AtomicUsize::new(0);
static GEN: AtomicU32 = AtomicU32::new(0);
/// The result board came up (or the race scene closed): no more seeking.
static ENDED: AtomicBool = AtomicBool::new(false);
/// The gates opened in this race (race time went above 0 once).
static STARTED: AtomicBool = AtomicBool::new(false);
/// A drag paused the race: resume when it ends.
static PAUSED_BY_US: AtomicBool = AtomicBool::new(false);
static SEEK_COUNT: Mutex<u32> = Mutex::new(0);

pub fn error() -> Option<String> {
    match GAME_DATA.get() {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------- speed
//
// Playback speed rides on the race's own time scale: every
// `RaceManagerReplayBase::SetDeltaTimeScale(scale)` the game makes (its own
// fast-forward, pausing) goes through our hook and becomes `scale * speed`;
// a speed change re-applies the game's last scale times the new speed.

const ONE: u32 = 0x3f80_0000; // 1.0f32
/// Our speed (f32 bits).
static SPEED: AtomicU32 = AtomicU32::new(ONE);
/// The scale the game last asked for (f32 bits).
static GAME_SCALE: AtomicU32 = AtomicU32::new(ONE);
static ORIG_SET_SCALE: AtomicUsize = AtomicUsize::new(0);
static SET_SCALE_INFO: AtomicUsize = AtomicUsize::new(0);

fn speed() -> f32 {
    f32::from_bits(SPEED.load(Ordering::Relaxed))
}

type SetScaleFn = extern "C" fn(Obj, f32, *const c_void);

/// Hook (main thread): the game sets the race time scale.
extern "C" fn set_delta_time_scale(this: Obj, scale: f32, mi: *const c_void) {
    GAME_SCALE.store(scale.to_bits(), Ordering::Relaxed);
    let orig = ORIG_SET_SCALE.load(Ordering::Acquire);
    if orig != 0 {
        let f: SetScaleFn = unsafe { std::mem::transmute(orig) };
        f(this, scale * speed(), mi);
    }
}

/// Main thread: sets our speed on race manager `rm`.
unsafe fn set_speed(rm: Obj, s: f32) {
    let orig = ORIG_SET_SCALE.load(Ordering::Acquire);
    if orig == 0 {
        return;
    }
    SPEED.store(s.to_bits(), Ordering::Relaxed);
    let f: SetScaleFn = std::mem::transmute(orig);
    f(rm, f32::from_bits(GAME_SCALE.load(Ordering::Relaxed)) * s, SET_SCALE_INFO.load(Ordering::Relaxed) as *const c_void);
}

/// At game ready: hook the race time scale (without it the speed buttons do nothing).
pub fn install() {
    let result = unsafe {
        il2cpp::class(GAME, "Gallop", "RaceManagerReplayBase")
            .and_then(|k| void_method(k, "SetDeltaTimeScale", &["System.Single"]))
            .and_then(|m| il2cpp::hook(m.code, set_delta_time_scale as *const () as usize).map(|t| (m, t)))
    };
    match result {
        Ok((m, t)) => {
            SET_SCALE_INFO.store(m.info, Ordering::Relaxed);
            ORIG_SET_SCALE.store(t, Ordering::Release);
        }
        Err(e) => api::get().warn(&format!("seek bar: playback speed not available ({e})")),
    }
}

pub fn speed_available() -> bool {
    ORIG_SET_SCALE.load(Ordering::Acquire) != 0
}

/// The race is over (result board, winning circle, leaving): stop.
pub fn race_ended() {
    ENDED.store(true, Ordering::Relaxed);
    super::publish(None);
    super::on_cancel();
    super::with_commands(|c| *c = Default::default());
}

/// Once per race frame (main thread), after the clock's tick.
pub fn tick(rm: Obj) {
    if rm.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { tick_inner(rm) }));
}

unsafe fn tick_inner(rm: Obj) {
    let gen = race_ui::race_gen();
    if RACE.swap(rm as usize, Ordering::Relaxed) != rm as usize || GEN.swap(gen, Ordering::Relaxed) != gen {
        // A new race: forget the last one, back to 1x.
        ENDED.store(false, Ordering::Relaxed);
        STARTED.store(false, Ordering::Relaxed);
        PAUSED_BY_US.store(false, Ordering::Relaxed);
        super::on_cancel();
        super::with_commands(|c| *c = Default::default());
        GAME_SCALE.store(ONE, Ordering::Relaxed);
        if speed() != 1.0 {
            set_speed(rm, 1.0);
        }
    }
    if !settings::is_on(SEEK_BAR) || ENDED.load(Ordering::Relaxed) {
        // Result board, winning circle, bar turned off: normal speed again.
        if speed() != 1.0 {
            set_speed(rm, 1.0);
        }
        super::publish(None);
        return;
    }
    let g = match GAME_DATA.get_or_init(|| {
        let r = Game::resolve();
        match &r {
            Ok(_) => api::get().info("seek bar: ready"),
            Err(e) => api::get().error(&format!("seek bar not available: {e}")),
        }
        r
    }) {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(reader) = g.reader(rm) else {
        super::publish(None);
        return;
    };
    let total = g.call0::<f32>(g.last_frame_time, reader);
    let cur: f32 = il2cpp::read(reader, g.cur_time_off);
    if !(total > 0.0) || !cur.is_finite() {
        super::publish(None);
        return;
    }
    if cur > 0.0 {
        STARTED.store(true, Ordering::Relaxed);
    }
    // From the gates opening to the end of the timeline (the result board
    // ends it, see `race_ended`). Unlike Hachimi's slider it stays after the
    // finish, so the finish can be watched again.
    if !STARTED.load(Ordering::Relaxed) {
        super::publish(None);
        return;
    }

    let start_dash = g.start_dash(rm);
    let interactable = !start_dash && !g.bool0(g.playing_cut_in, rm);
    apply_commands(g, rm, reader, start_dash);

    let cur: f32 = il2cpp::read(reader, g.cur_time_off);
    super::publish(Some(Replay {
        cur: cur.clamp(0.0, total),
        total,
        paused: g.bool0(g.is_paused, rm),
        interactable,
        speed: speed(),
    }));
}

unsafe fn apply_commands(g: &Game, rm: Obj, reader: Obj, start_dash: bool) {
    super::with_commands(|c| {
        if c.is_empty() {
            return;
        }
        // Hachimi waits out cut-ins before seeking; so do we. A reserved
        // cut-in may only start once the race runs, so a released drag never
        // waits: it drops its last seek and resumes.
        if let Some(s) = c.speed.take() {
            if s != speed() {
                set_speed(rm, s);
                api::get().info(&format!("seek bar: speed {}", super::fmt_speed(s)));
            }
        }
        let cut_in = g.bool0(g.cut_in_playing_or_reserved, rm);
        if start_dash {
            c.begin = false;
            c.target = None;
            c.toggle_pause = false;
        }
        if !cut_in && std::mem::take(&mut c.toggle_pause) {
            if g.bool0(g.is_paused, rm) {
                g.void0(g.resume, rm);
            } else {
                g.void0(g.pause, rm);
            }
            PAUSED_BY_US.store(false, Ordering::Relaxed);
        }
        if std::mem::take(&mut c.begin) && !g.bool0(g.is_paused, rm) {
            g.void0(g.pause, rm);
            PAUSED_BY_US.store(true, Ordering::Relaxed);
        }
        if let Some(t) = c.target {
            if !cut_in {
                c.target = None;
                let total = g.call0::<f32>(g.last_frame_time, reader);
                let t = t.clamp(0.0, total.max(0.0));
                g.seek(rm, reader, t);
                let mut n = SEEK_COUNT.lock().unwrap_or_else(|e| e.into_inner());
                if *n == 0 {
                    api::get().info(&format!("seek bar: first seek done ({t:.1}s)"));
                }
                *n += 1;
            } else if c.end {
                c.target = None;
            }
        }
        if c.target.is_none() && std::mem::take(&mut c.end) && PAUSED_BY_US.swap(false, Ordering::Relaxed) && g.bool0(g.is_paused, rm) {
            g.void0(g.resume, rm);
        }
    });
}
