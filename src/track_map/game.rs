//! Main-thread side of the track map: course outline (once per race) and
//! runner positions (every frame).
//!
//! Called from clock's `RaceManagerReplayBase::AlterUpdate` hook after the
//! clock's tick (it uses the clock's course length and race time). The course
//! is sampled only while the race is live: the course data exists then (the
//! game moves every horse along it), so `GetLaneTransform` cannot fail.

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use super::{Dot, Ground, MapState};
use crate::il2cpp::{self, Method, Obj, GAME};
use crate::{api, clock, race_ui};

struct Game {
    /// RaceManager::GetLaneTransform(float distance, out Vector3 position, out Quaternion rotation)
    lane_transform: Method,
    horse_infos: Method,
    info_class: usize,
    position_off: usize,
    distance_off: usize,
    /// static RaceManager::get_RaceInfo, RaceInfo::get_GroundType, and the Turf/Dirt values.
    race_info: Method,
    ground_type: Method,
    turf: i32,
    dirt: i32,
}

unsafe impl Send for Game {}
unsafe impl Sync for Game {}

static GAME_DATA: OnceLock<Result<Game, String>> = OnceLock::new();

pub fn error() -> Option<String> {
    match GAME_DATA.get() {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    }
}

unsafe fn call0<T>(m: Method, this: Obj) -> T {
    let f: extern "C" fn(Obj, usize) -> T = std::mem::transmute(m.code);
    f(this, m.info)
}

impl Game {
    unsafe fn resolve() -> Result<Game, String> {
        use il2cpp::{class, enum_value, expect, field_offset, method, method_params, nested};
        let rm = class(GAME, "Gallop", "RaceManager")?;
        let info = class(GAME, "Gallop", "HorseRaceInfo")?;
        let race_info = class(GAME, "Gallop", "RaceInfo")?;
        let ground = nested(class(GAME, "Gallop", "RaceDefine")?, "GroundType")?;

        let lane_transform =
            method_params(rm, "GetLaneTransform", &["System.Single", "UnityEngine.Vector3&", "UnityEngine.Quaternion&"])?;
        expect(lane_transform, false, &["System.Void"], 3)?;
        let horse_infos = method(rm, "GetHorseRaceInfos", 0)?;
        if !il2cpp::return_type(horse_infos).ends_with("[]") {
            return Err("GetHorseRaceInfos does not return an array".to_owned());
        }
        let get_race_info = method(rm, "get_RaceInfo", 0)?;
        expect(get_race_info, true, &["Gallop.RaceInfo"], 0)?;
        let ground_type = method(race_info, "get_GroundType", 0)?;
        expect(ground_type, false, &["Gallop.RaceDefine.GroundType"], 0)?;
        Ok(Game {
            lane_transform,
            horse_infos,
            info_class: info as usize,
            position_off: field_offset(info, "_position")?,
            distance_off: field_offset(info, "_distance")?,
            race_info: get_race_info,
            ground_type,
            turf: enum_value(ground, &["Turf"])?.0,
            dirt: enum_value(ground, &["Dirt"])?.0,
        })
    }

    /// World position of the course at `distance` metres.
    unsafe fn course_point(&self, rm: Obj, distance: f32) -> Option<(f32, f32)> {
        let mut pos = [0f32; 3];
        let mut rot = [0f32; 4];
        let f: extern "C" fn(Obj, f32, *mut [f32; 3], *mut [f32; 4], usize) = std::mem::transmute(self.lane_transform.code);
        f(rm, distance, &mut pos, &mut rot, self.lane_transform.info);
        (pos[0].is_finite() && pos[2].is_finite()).then_some((pos[0], pos[2]))
    }

    unsafe fn ground(&self) -> Option<Ground> {
        let f: extern "C" fn(usize) -> Obj = std::mem::transmute(self.race_info.code);
        let info = f(self.race_info.info);
        if info.is_null() {
            return None;
        }
        let g: i32 = call0(self.ground_type, info);
        if g == self.turf {
            Some(Ground::Turf)
        } else if g == self.dirt {
            Some(Ground::Dirt)
        } else {
            None
        }
    }
}

/// The course of the race in progress, sampled once.
struct Course {
    race: usize,
    gen: u32,
    length: f32,
    points: Vec<(f32, f32)>,
    ground: Option<Ground>,
}

static COURSE: Mutex<Option<Course>> = Mutex::new(None);
/// Leader speed estimate: (last time, last distance, smoothed m/s) as f32 bits.
static SPEED: Mutex<Option<(f32, f32, Option<f32>)>> = Mutex::new(None);
static RACE: AtomicUsize = AtomicUsize::new(0);
static GEN: AtomicU32 = AtomicU32::new(0);

/// About one point per this many metres (at most `MAX_POINTS`).
const STEP: f32 = 4.0;
const MAX_POINTS: usize = 700;
/// Speed smoothing (share of each new measurement).
const SPEED_SMOOTHING: f32 = 0.12;

unsafe fn sample(g: &Game, rm: Obj, length: f32) -> Vec<(f32, f32)> {
    let n = ((length / STEP) as usize).clamp(16, MAX_POINTS);
    let mut points = Vec::with_capacity(n + 1);
    for i in 0..=n {
        if let Some(p) = g.course_point(rm, length * i as f32 / n as f32) {
            points.push(p);
        }
    }
    // A course that never moves is not a course (data not ready).
    let moved = points.windows(2).any(|w| (w[0].0 - w[1].0).abs() + (w[0].1 - w[1].1).abs() > 0.01);
    if moved { points } else { Vec::new() }
}

/// Once per race frame (main thread), after the clock's tick.
pub fn tick(rm: Obj) {
    if rm.is_null() || !super::is_enabled() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe { tick_inner(rm) }));
}

unsafe fn tick_inner(rm: Obj) {
    let gen = race_ui::race_gen();
    if RACE.swap(rm as usize, Ordering::Relaxed) != rm as usize || GEN.swap(gen, Ordering::Relaxed) != gen {
        *COURSE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *SPEED.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    if !clock::race_live() {
        super::publish(None);
        return;
    }
    let g = match GAME_DATA.get_or_init(|| {
        let r = Game::resolve();
        match &r {
            Ok(_) => api::get().info("track map: ready"),
            Err(e) => api::get().error(&format!("track map not available: {e}")),
        }
        r
    }) {
        Ok(g) => g,
        Err(_) => return,
    };
    let Some(length) = clock::course_distance() else { return };

    let mut course = COURSE.lock().unwrap_or_else(|e| e.into_inner());
    let stale = course.as_ref().is_none_or(|c| c.race != rm as usize || c.gen != gen || c.length != length);
    if stale {
        let points = sample(g, rm, length);
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| api::get().info(&format!("track map: course of {length:.0} m sampled into {} points", points.len())));
        *course = Some(Course { race: rm as usize, gen, length, points, ground: g.ground() });
    }
    let Some(c) = course.as_ref().filter(|c| c.points.len() >= 2) else { return };

    let mut dots = Vec::with_capacity(18);
    let items = il2cpp::array_items(call0(g.horse_infos, rm));
    for (i, &info) in items.iter().enumerate() {
        if !il2cpp::is_instance(info, g.info_class as *mut c_void) {
            continue;
        }
        let pos: [f32; 3] = il2cpp::read(info, g.position_off);
        let distance: f32 = il2cpp::read(info, g.distance_off);
        if pos[0].is_finite() && pos[2].is_finite() {
            dots.push(Dot { index: i as i32, x: pos[0], z: pos[2], distance });
        }
    }

    let speed_kmh = leader_speed(&dots);
    super::publish(Some(MapState { course: c.points.clone(), length: c.length, ground: c.ground, dots, speed_kmh }));
}

/// The leader's speed from its distance over race time, smoothed:
/// km/h = Δdistance / (Δraw time × time scale) × 3.6.
fn leader_speed(dots: &[Dot]) -> Option<f32> {
    let leader = dots.iter().map(|d| d.distance).fold(f32::MIN, f32::max);
    let t = clock::race_time()?;
    let mut s = SPEED.lock().unwrap_or_else(|e| e.into_inner());
    let (prev_t, prev_d, smooth) = s.unwrap_or((t, leader, None));
    let dt = t - prev_t;
    let dd = leader - prev_d;
    let smooth = if dt <= 0.0 {
        smooth // paused, or the same frame
    } else if dt > 1.0 || dd < 0.0 {
        None // a seek: start over
    } else {
        // Race time is the game's raw simulation clock; the times it shows
        // (and our race clock) run `time_scale` slower. Metres per shown second.
        let v = dd / (dt * clock::time_scale());
        Some(match smooth {
            Some(prev) => prev + (v - prev) * SPEED_SMOOTHING,
            None => v,
        })
    };
    *s = Some((t, leader, smooth));
    smooth.filter(|v| v.is_finite() && *v > 0.0).map(|v| v * 3.6)
}
