//! Race clock, top corner (TOP_RIGHT), broadcast style:
//!   - running: white whole seconds ("12", "1.05")
//!   - split:   when the leader passes 1000 m (half the course for Short
//!              races <= 1400 m), yellow time with tenths and the distance
//!              underneath ("57.8" / "1000m"), held for SPLIT_HOLD seconds
//!   - finish:  when the leader crosses the line, the winner's official time
//!              ("1.56.1"), held until the winning-circle scene
//!
//! Driven by a hook on `RaceManagerReplayBase::AlterUpdate` (once per frame,
//! main thread, `this` = the race manager). Race time is the replay reader's
//! `_curTime` (what Hachimi's playback slider shows). The game's official
//! finish time is "scaled"; the clock is multiplied by
//! `winner official / winner raw` so it ends on the official time.

use std::ffi::c_void;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::il2cpp::{self, Method, Obj, GAME};
use crate::label::{Label, Layout, Unity, TEXT_ANCHOR_UPPER_LEFT, TEXT_ANCHOR_UPPER_RIGHT};
use crate::settings::{self, CLOCK};
use crate::{api, race_ui, tracer};

const SPLIT_HOLD: f32 = 5.0;
/// Races up to this length are Short: split at half distance instead of 1000 m.
const SHORT_MAX: f32 = 1400.0;
const WHITE: &str = "#FFFFFF";
const YELLOW: &str = "#FFE600";

/// Which top corner the clock sits in. Text is aligned to that corner.
const TOP_RIGHT: bool = true;
/// Distance from the screen edge (canvas units; about screen pixels at 2000 px wide).
const MARGIN_X: f32 = 48.0;
/// Width of ".0" at size 72 with outline (measured ~90 on screen). At the top
/// right, the running seconds sit this much further left, so they end where
/// the whole seconds of "57.8" / "2.22.0" will end and the tenths fill the
/// gap. At the top left nothing moves: the tenths extend to the right.
const TENTHS_GAP: f32 = if TOP_RIGHT { 90.0 } else { 0.0 };
const BOX_WIDTH: f32 = 600.0;
/// Split and final time ("57.8", "2.22.0").
const TIME_LAYOUT: Layout = Layout {
    right: TOP_RIGHT,
    alignment: if TOP_RIGHT { TEXT_ANCHOR_UPPER_RIGHT } else { TEXT_ANCHOR_UPPER_LEFT },
    font_size: 72,
    line_spacing: 1.0,
    margin_x: MARGIN_X,
    margin_y: 24.0,
    width: BOX_WIDTH,
    height: 110.0,
    outline_color: &["Black"],
    outline_size: &["L", "M"],
};
/// Running whole seconds: same label, shifted inwards by TENTHS_GAP.
const RUNNING_LAYOUT: Layout = Layout { margin_x: MARGIN_X + TENTHS_GAP, ..TIME_LAYOUT };
/// The distance under the split time ("1000m"). Its own label so its gap to
/// the time does not depend on the font's line height: higher `margin_y` =
/// lower on screen.
const DISTANCE_LAYOUT: Layout = Layout { font_size: 40, margin_y: 104.0, height: 60.0, ..TIME_LAYOUT };

// ---------------------------------------------------------------- pure logic

/// Where the split is taken for a course of `course` metres.
pub fn split_distance(course: f32) -> f32 {
    if course <= SHORT_MAX { course / 2.0 } else { 1000.0 }
}

/// Time at which distance `at` was reached between two samples.
fn cross(t0: f32, d0: f32, t1: f32, d1: f32, at: f32) -> f32 {
    if d1 <= d0 { t1 } else { t0 + (at - d0) / (d1 - d0) * (t1 - t0) }
}

/// Leader crossings in raw race time, updated from per-frame samples.
#[derive(Default)]
pub struct Timing {
    prev: Option<(f32, f32)>,
    pub split_raw: Option<f32>,
    pub finish_raw: Option<f32>,
}

/// Samples further apart than this are a seek, not a frame: no interpolation.
const MAX_STEP: f32 = 1.0;

impl Timing {
    pub fn step(&mut self, t: f32, leader: f32, course: f32) {
        if let Some((pt, _)) = self.prev {
            if t + 0.05 < pt {
                // Seek backwards (playback slider): forget what is now in the future.
                if self.split_raw.is_some_and(|s| t < s) {
                    self.split_raw = None;
                }
                if self.finish_raw.is_some_and(|f| t < f) {
                    self.finish_raw = None;
                }
                self.prev = None;
            }
        }
        if let Some((pt, pd)) = self.prev {
            let smooth = t - pt <= MAX_STEP;
            let split = split_distance(course);
            if self.split_raw.is_none() && smooth && pd < split && leader >= split {
                self.split_raw = Some(cross(pt, pd, t, leader, split));
            }
            if self.finish_raw.is_none() && course > 0.0 && pd < course && leader >= course {
                self.finish_raw = Some(if smooth { cross(pt, pd, t, leader, course) } else { t });
            }
        }
        self.prev = Some((t, leader));
    }
}

#[derive(Debug, PartialEq)]
pub enum View {
    Hidden,
    Running(f32),
    Split(f32, u32),
    Final(f32),
}

/// What to show at raw time `t`. `k` converts raw to displayed time;
/// `official` is the winner's official time when known.
pub fn view(t: f32, timing: &Timing, course: f32, k: f32, official: Option<f32>) -> View {
    if !(t > 0.0) {
        return View::Hidden;
    }
    if let Some(f) = timing.finish_raw {
        return View::Final(official.unwrap_or(f * k));
    }
    if let Some(s) = timing.split_raw {
        if (t - s) * k < SPLIT_HOLD {
            return View::Split(s * k, split_distance(course).round() as u32);
        }
    }
    View::Running(t * k)
}

/// "12", "1.05"
pub fn fmt_running(t: f32) -> String {
    let s = t.max(0.0).floor() as u32;
    if s < 60 { s.to_string() } else { format!("{}.{:02}", s / 60, s % 60) }
}

/// "57.8", "1.56.1" (tenths truncated, like the race result)
pub fn fmt_tenths(t: f32) -> String {
    let tenths = (t.max(0.0) * 10.0 + 1e-3).floor() as u32;
    let (s, d) = (tenths / 10, tenths % 10);
    if s < 60 { format!("{s}.{d}") } else { format!("{}.{:02}.{d}", s / 60, s % 60) }
}

/// Unity rich text for the time label and the distance label.
#[derive(Debug, PartialEq)]
pub struct Texts {
    pub time: String,
    pub distance: Option<String>,
    /// Whole seconds only: shift by TENTHS_GAP.
    pub running: bool,
}

/// None = hide both labels.
pub fn render(v: &View) -> Option<Texts> {
    let color = |c: &str, s: String| format!("<color={c}>{s}</color>");
    match *v {
        View::Hidden => None,
        View::Running(t) => Some(Texts { time: color(WHITE, fmt_running(t)), distance: None, running: true }),
        View::Split(t, m) => Some(Texts {
            time: color(YELLOW, fmt_tenths(t)),
            distance: Some(color(YELLOW, format!("{m}m"))),
            running: false,
        }),
        View::Final(t) => Some(Texts { time: color(WHITE, fmt_tenths(t)), distance: None, running: false }),
    }
}

/// raw -> displayed factor; 1.0 when unknown or implausible.
pub fn scale(official: Option<f32>, raw: Option<f32>) -> f32 {
    match (official, raw) {
        (Some(o), Some(r)) if r > 0.0 && (0.5..2.0).contains(&(o / r)) => o / r,
        _ => 1.0,
    }
}

// ---------------------------------------------------------------- race data

enum CourseKind {
    Single,
    Int,
}

struct RaceData {
    horse_manager_off: usize,
    replay_manager_class: usize,
    reader_off: usize,
    cur_time_off: usize,
    first_horse_distance: Method,
    course_distance: Method,
    course_kind: CourseKind,
    horse_infos: Method,
    info_replay_class: usize,
    finish_order: Method,
    finish_time_scaled: Method,
    /// Raw (unscaled) finish time getter, if the game has one.
    finish_time_raw: Option<(Method, &'static str)>,
}

struct Sample {
    t: f32,
    leader: f32,
    course: f32,
}

impl RaceData {
    unsafe fn resolve() -> Result<RaceData, String> {
        use il2cpp::{class, expect, field_offset, method};
        let rm = class(GAME, "Gallop", "RaceManager")?;
        let rm_replay = class(GAME, "Gallop", "RaceManagerReplayBase")?;
        let hm_replay = class(GAME, "Gallop", "RaceHorseManagerReplay")?;
        let reader = class(GAME, "Gallop", "RaceSimulateReader")?;
        let info_replay = class(GAME, "Gallop", "HorseRaceInfoReplay")?;

        let first_horse_distance = method(rm, "GetFirstHorseDistance", 0)?;
        expect(first_horse_distance, false, &["System.Single"], 0)?;
        let course_distance = method(rm_replay, "GetCourseDistance", 0)?;
        expect(course_distance, false, &["System.Single", "System.Int32"], 0)?;
        let course_kind =
            if il2cpp::return_type(course_distance) == "System.Single" { CourseKind::Single } else { CourseKind::Int };
        let horse_infos = method(rm, "GetHorseRaceInfos", 0)?;
        let infos_ret = il2cpp::return_type(horse_infos);
        if !infos_ret.ends_with("[]") {
            return Err(format!("GetHorseRaceInfos returns {infos_ret}, expected an array"));
        }
        let finish_order = method(info_replay, "get_FinishOrder", 0)?;
        expect(finish_order, false, &["System.Int32"], 0)?;
        let finish_time_scaled = method(info_replay, "get_FinishTimeScaled", 0)?;
        expect(finish_time_scaled, false, &["System.Single"], 0)?;

        let finish_time_raw = ["get_FinishTimeRaw", "get_FinishTime"].into_iter().find_map(|name| {
            let m = method(info_replay, name, 0).ok()?;
            expect(m, false, &["System.Single"], 0).ok()?;
            Some((m, name))
        });

        Ok(RaceData {
            horse_manager_off: field_offset(rm, "_horseManager")?,
            replay_manager_class: hm_replay as usize,
            reader_off: field_offset(hm_replay, "_reader")?,
            cur_time_off: field_offset(reader, "_curTime")?,
            first_horse_distance,
            course_distance,
            course_kind,
            horse_infos,
            info_replay_class: info_replay as usize,
            finish_order,
            finish_time_scaled,
            finish_time_raw,
        })
    }

    unsafe fn f32_getter(m: Method, this: Obj) -> f32 {
        let f: extern "C" fn(Obj, usize) -> f32 = std::mem::transmute(m.code);
        f(this, m.info)
    }

    unsafe fn sample(&self, rm: Obj) -> Option<Sample> {
        let hm: Obj = il2cpp::read(rm, self.horse_manager_off);
        if !il2cpp::is_instance(hm, self.replay_manager_class as *mut c_void) {
            return None;
        }
        let reader: Obj = il2cpp::read(hm, self.reader_off);
        if reader.is_null() {
            return None;
        }
        let t: f32 = il2cpp::read(reader, self.cur_time_off);
        let leader = Self::f32_getter(self.first_horse_distance, rm);
        let course = match self.course_kind {
            CourseKind::Single => Self::f32_getter(self.course_distance, rm),
            CourseKind::Int => {
                let f: extern "C" fn(Obj, usize) -> i32 = std::mem::transmute(self.course_distance.code);
                f(rm, self.course_distance.info) as f32
            }
        };
        (t.is_finite() && leader.is_finite() && course.is_finite()).then_some(Sample { t, leader, course })
    }

    /// Winner's (official, raw) finish time, from the replay's result data.
    unsafe fn winner(&self, rm: Obj) -> Option<(f32, Option<f32>)> {
        let f: extern "C" fn(Obj, usize) -> Obj = std::mem::transmute(self.horse_infos.code);
        let arr = f(rm, self.horse_infos.info);
        let order: extern "C" fn(Obj, usize) -> i32 = std::mem::transmute(self.finish_order.code);
        let mut best: Option<(i32, Obj)> = None;
        for &info in il2cpp::array_items(arr) {
            if !il2cpp::is_instance(info, self.info_replay_class as *mut c_void) {
                continue;
            }
            let o = order(info, self.finish_order.info);
            if o >= 0 && best.map_or(true, |(b, _)| o < b) {
                best = Some((o, info));
            }
        }
        let (_, info) = best?;
        let official = Self::f32_getter(self.finish_time_scaled, info);
        if !(official > 0.0) {
            return None;
        }
        let raw = self.finish_time_raw.map(|(m, _)| Self::f32_getter(m, info)).filter(|r| *r > 0.0);
        Some((official, raw))
    }
}

// ---------------------------------------------------------------- state

struct Resolved {
    unity: Unity,
    data: RaceData,
}

static RESOLVED: OnceLock<Resolved> = OnceLock::new();
static STATUS: OnceLock<String> = OnceLock::new();

struct State {
    race: usize,
    /// race_ui::race_gen() of the race this state belongs to (the race
    /// manager object can be reused across races).
    gen: u32,
    ended: bool,
    timing: Timing,
    winner: Option<(f32, Option<f32>)>,
    time_label: Label,
    distance_label: Label,
    error_logged: bool,
}

impl State {
    unsafe fn hide(&mut self, u: &Unity) {
        self.time_label.hide(u);
        self.distance_label.hide(u);
    }

    unsafe fn show(&mut self, u: &Unity, texts: &Texts) -> Result<(), String> {
        let layout = if texts.running { &RUNNING_LAYOUT } else { &TIME_LAYOUT };
        self.time_label.show(u, layout, &texts.time)?;
        match &texts.distance {
            Some(d) => self.distance_label.show(u, &DISTANCE_LAYOUT, d)?,
            None => self.distance_label.hide(u),
        }
        Ok(())
    }
}

static STATE: Mutex<State> = Mutex::new(State {
    race: 0,
    gen: 0,
    ended: false,
    timing: Timing { prev: None, split_raw: None, finish_raw: None },
    winner: None,
    time_label: Label::new(),
    distance_label: Label::new(),
    error_logged: false,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn status() -> String {
    STATUS.get().cloned().unwrap_or_else(|| "waiting for the game".to_owned())
}

/// Race time in seconds of the current race (raw clock), as f32 bits.
static TIME_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Seconds since the start of the race in progress, while `race_live`. Any thread.
pub fn race_time() -> Option<f32> {
    race_live().then(|| f32::from_bits(TIME_BITS.load(Ordering::Relaxed)))
}

/// raw race time -> displayed time factor (official / raw), as f32 bits.
static SCALE_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000);

/// Displayed seconds per raw second of the race in progress (1.0 if unknown):
/// the game simulates faster than the times it shows. Any thread.
pub fn time_scale() -> f32 {
    let k = f32::from_bits(SCALE_BITS.load(Ordering::Relaxed));
    if k.is_finite() && k > 0.0 { k } else { 1.0 }
}

/// Leader's distance run (metres) in the current race, as f32 bits.
static LEAD_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Metres the leader still has to run, while `race_live` (<= 0 once they finished).
pub fn remaining_distance() -> Option<f32> {
    let course = course_distance()?;
    race_live().then(|| course - f32::from_bits(LEAD_BITS.load(Ordering::Relaxed)))
}

/// Course length in metres of the current race, as f32 bits (0 = unknown).
static COURSE_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Course length of the race in progress, if known. Any thread.
pub fn course_distance() -> Option<f32> {
    let c = f32::from_bits(COURSE_BITS.load(Ordering::Relaxed));
    (c > 0.0).then_some(c)
}

/// Last frame at which the race was live (see `race_live`).
static LAST_LIVE: Mutex<Option<Instant>> = Mutex::new(None);
/// How long `race_live` stays true without a frame confirming it (covers
/// leaving a race without the end hooks firing).
const LIVE_TIMEOUT: Duration = Duration::from_millis(500);

/// True from the moment the race clock starts counting (race time > 0, the
/// gates have opened) until the race ends (winning circle / leaving the race),
/// whether or not the clock itself is turned on. Any thread.
pub fn race_live() -> bool {
    let last = LAST_LIVE.lock().unwrap_or_else(|e| e.into_inner());
    last.is_some_and(|t| t.elapsed() < LIVE_TIMEOUT)
}

/// Whether `race_live` means anything: the per-frame hook and the race-time
/// reader are installed. False before game ready or if a game update broke them.
pub fn race_live_available() -> bool {
    RESOLVED.get().is_some() && ORIG_ALTER_UPDATE.load(Ordering::Acquire) != 0
}

/// Live = the same moments the clock shows a time.
pub fn is_live(ended: bool, race_time: Option<f32>) -> bool {
    !ended && race_time.is_some_and(|t| t > 0.0)
}

fn set_live(live: bool) {
    *LAST_LIVE.lock().unwrap_or_else(|e| e.into_inner()) = live.then(Instant::now);
}

/// Once per frame during a race (main thread).
unsafe fn tick(rm: Obj) {
    let Some(r) = RESOLVED.get() else { return };
    let mut st = state();
    let gen = race_ui::race_gen();
    if st.race != rm as usize || st.gen != gen {
        st.gen = gen;
        st.race = rm as usize;
        st.ended = false;
        st.timing = Timing::default();
        st.winner = None;
        SCALE_BITS.store(1.0f32.to_bits(), Ordering::Relaxed);
    }
    let sample = if st.ended { None } else { r.data.sample(rm) };
    // Published even with the clock turned off: the leaderboard follows it.
    set_live(is_live(st.ended, sample.as_ref().map(|s| s.t)));
    if let Some(s) = &sample {
        COURSE_BITS.store(s.course.to_bits(), Ordering::Relaxed);
        TIME_BITS.store(s.t.to_bits(), Ordering::Relaxed);
        LEAD_BITS.store(s.leader.to_bits(), Ordering::Relaxed);
        // The replay's result data is there from the start: the time scale
        // is known before anyone finishes (the track map's speed uses it).
        if st.winner.is_none() {
            st.winner = r.data.winner(rm);
        }
    }
    let Some(s) = sample.filter(|_| settings::is_on(CLOCK)) else {
        let k = scale(st.winner.map(|w| w.0), st.winner.and_then(|w| w.1));
        SCALE_BITS.store(k.to_bits(), Ordering::Relaxed);
        st.hide(&r.unity);
        return;
    };

    st.timing.step(s.t, s.leader, s.course);
    let official = st.winner.map(|w| w.0);
    let raw = st.winner.and_then(|w| w.1).or(st.timing.finish_raw);
    let k = scale(official, raw);
    SCALE_BITS.store(k.to_bits(), Ordering::Relaxed);

    match render(&view(s.t, &st.timing, s.course, k, official)) {
        Some(texts) => {
            if let Err(e) = st.show(&r.unity, &texts) {
                if !st.error_logged {
                    st.error_logged = true;
                    api::get().error(&format!("race clock: {e}"));
                }
            }
        }
        None => st.hide(&r.unity),
    }
}

/// The race is over (winning circle or leaving the race): hide and stay hidden.
unsafe fn race_ended() {
    crate::seek::game::race_ended();
    let Some(r) = RESOLVED.get() else { return };
    let mut st = state();
    st.ended = true;
    set_live(false);
    st.hide(&r.unity);
}

/// The result board (summary) comes up: the clock and the leaderboard stop
/// here, not at the winning circle. Called by race_ui's StateOverRunResult_Enter hook.
pub fn result_board_shown() {
    unsafe { race_ended() }
}

// ---------------------------------------------------------------- hooks

type VoidFn = extern "C" fn(Obj, *const c_void);

/// Runs the original through its trampoline (waits out the install window).
fn call_orig(slot: &AtomicUsize, this: Obj, mi: *const c_void) {
    let t = loop {
        let t = slot.load(Ordering::Acquire);
        if t != 0 {
            break t;
        }
        std::hint::spin_loop();
    };
    let f: VoidFn = unsafe { std::mem::transmute(t) };
    f(this, mi)
}

static ORIG_ALTER_UPDATE: AtomicUsize = AtomicUsize::new(0);

/// The per-frame hook for every race feature.
extern "C" fn alter_update(this: Obj, mi: *const c_void) {
    call_orig(&ORIG_ALTER_UPDATE, this, mi);
    race_ui::tick(this);
    if !this.is_null() {
        unsafe { tick(this) }
    }
    // After the clock: the seek bar reads `remaining_distance`.
    crate::seek::game::tick(this);
    crate::skill_popups::tick(this);
    crate::track_map::game::tick(this);
    crate::gate_banner::game::tick(this);
}

macro_rules! end_hook {
    ($detour:ident, $slot:ident, $ui:path) => {
        static $slot: AtomicUsize = AtomicUsize::new(0);
        extern "C" fn $detour(this: Obj, mi: *const c_void) {
            call_orig(&$slot, this, mi);
            $ui(this);
            unsafe { race_ended() }
        }
    };
}
end_hook!(winning_circle_enter, ORIG_WINNING_CIRCLE, race_ui::race_over);
end_hook!(go_to_race_exit, ORIG_GO_TO_RACE_EXIT, race_ui::race_ended);
end_hook!(change_next_scene, ORIG_CHANGE_NEXT_SCENE, race_ui::race_ended);

/// (class, method, detour, trampoline slot); all `void Method()` instance methods.
fn hooks() -> [(&'static str, &'static str, usize, &'static AtomicUsize); 4] {
    [
        ("RaceManagerReplayBase", "AlterUpdate", alter_update as *const () as usize, &ORIG_ALTER_UPDATE),
        ("RaceManagerReplayBase", "StateWinningCircleInit_Enter", winning_circle_enter as *const () as usize, &ORIG_WINNING_CIRCLE),
        ("RaceManagerReplayBase", "_GoToRaceExit", go_to_race_exit as *const () as usize, &ORIG_GO_TO_RACE_EXIT),
        ("RaceMainViewController", "ChangeNextScene", change_next_scene as *const () as usize, &ORIG_CHANGE_NEXT_SCENE),
    ]
}

// ---------------------------------------------------------------- install

/// Called once when IL2CPP is ready.
pub fn install() {
    let api = api::get();
    write_class_dump();

    // Without these the clock stays off, but the frame hooks below still
    // drive the other race features (race_ui).
    let resolved = unsafe { RaceData::resolve() }.and_then(|data| Ok(Resolved { unity: unsafe { Unity::resolve() }?, data }));
    let clock_state = match resolved {
        Ok(r) => {
            let source = match r.data.finish_time_raw {
                Some((_, name)) => format!("ok, time scale from HorseRaceInfoReplay::{name}"),
                None => "ok, time scale from the leader's finish".to_owned(),
            };
            let _ = RESOLVED.set(r);
            source
        }
        Err(e) => {
            api.error(&format!("race clock not available: {e}"));
            format!("FAILED ({e})")
        }
    };

    let mut status = String::new();
    for (class, name, detour, slot) in hooks() {
        let result = unsafe {
            il2cpp::class(GAME, "Gallop", class)
                .and_then(|k| il2cpp::method(k, name, 0))
                .and_then(|m| il2cpp::expect(m, false, &["System.Void"], 0).map(|_| m))
                .and_then(|m| il2cpp::hook(m.code, detour))
        };
        match result {
            Ok(t) => slot.store(t, Ordering::Release),
            Err(e) => {
                api.error(&format!("race clock: hook Gallop.{class}::{name} failed: {e}"));
                let _ = writeln!(status, "{name}: FAILED ({e})");
            }
        }
    }
    if ORIG_ALTER_UPDATE.load(Ordering::Acquire) == 0 {
        status.insert_str(0, "FAILED: no per-frame hook\n");
    } else {
        status.insert_str(0, &format!("{clock_state}\n"));
    }
    api.info(&format!("race clock: {}", status.trim_end().replace('\n', "; ")));
    let _ = STATUS.set(status.trim_end().to_owned());
}

/// Writes fields and methods of the race classes to race_classes.txt, so
/// later features can use real member names.
fn write_class_dump() {
    const CLASSES: [&str; 17] = [
        "RaceManager",
        "RaceHorseManagerBase",
        "HorseData",
        "RaceManagerReplayBase",
        "RaceHorseManagerReplay",
        "RaceSimulateReader",
        "HorseRaceInfo",
        "HorseRaceInfoReplay",
        "RaceSimulateHorseResultData",
        "RaceInfo",
        "RaceMainViewController",
        // Skill plates (skill_popups.rs).
        "RaceUI",
        "RaceUIMessagePlate",
        "MessagePlateCtrl",
        "MessagePlate",
        "MessagePlateNormalRace",
        "MessagePlateTeamStadium",
    ];
    let mut out = String::from("# Gallop race classes: fields (@offset) and methods (signature)\n\n");
    for name in CLASSES {
        match unsafe { il2cpp::class(GAME, "Gallop", name) } {
            Ok(k) => out.push_str(&unsafe { il2cpp::describe_class(k) }),
            Err(e) => {
                let _ = writeln!(out, "{e}");
            }
        }
        out.push('\n');
    }
    let path = tracer::plugin_dir().join("race_classes.txt");
    let result = std::fs::create_dir_all(tracer::plugin_dir()).and_then(|_| std::fs::write(&path, out));
    if let Err(e) = result {
        api::get().warn(&format!("cannot write {}: {e}", path.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_rule() {
        assert_eq!(split_distance(1200.0), 600.0);
        assert_eq!(split_distance(1400.0), 700.0);
        assert_eq!(split_distance(1600.0), 1000.0);
        assert_eq!(split_distance(2500.0), 1000.0);
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_running(0.4), "0");
        assert_eq!(fmt_running(12.9), "12");
        assert_eq!(fmt_running(65.2), "1.05");
        assert_eq!(fmt_tenths(57.86), "57.8");
        assert_eq!(fmt_tenths(116.1), "1.56.1");
        assert_eq!(fmt_tenths(59.99), "59.9");
        assert_eq!(fmt_tenths(60.0), "1.00.0");
    }

    /// Leader at constant 16 m/s, 30 fps.
    fn run(timing: &mut Timing, from: f32, to: f32, course: f32) {
        let mut t = from;
        while t <= to {
            timing.step(t, t * 16.0, course);
            t += 1.0 / 30.0;
        }
    }

    #[test]
    fn live_while_the_clock_counts() {
        assert!(!is_live(false, None), "no race data");
        assert!(!is_live(false, Some(0.0)), "before the gates open");
        assert!(is_live(false, Some(0.1)));
        assert!(!is_live(true, Some(95.0)), "after the race ended");
    }

    #[test]
    fn split_and_finish_are_interpolated() {
        let mut timing = Timing::default();
        run(&mut timing, 0.0, 200.0, 2000.0);
        assert!((timing.split_raw.unwrap() - 62.5).abs() < 0.01);
        assert!((timing.finish_raw.unwrap() - 125.0).abs() < 0.01);
    }

    #[test]
    fn views_over_a_race() {
        let mut timing = Timing::default();
        let course = 2000.0;
        let k = 1.0;
        assert_eq!(view(0.0, &timing, course, k, None), View::Hidden);

        run(&mut timing, 0.0, 30.0, course);
        assert_eq!(
            render(&view(30.0, &timing, course, k, None)).unwrap(),
            Texts { time: "<color=#FFFFFF>30</color>".into(), distance: None, running: true }
        );

        run(&mut timing, 30.0, 64.0, course);
        assert_eq!(view(64.0, &timing, course, k, None), View::Split(timing.split_raw.unwrap(), 1000));
        assert_eq!(
            render(&view(64.0, &timing, course, k, None)).unwrap(),
            Texts {
                time: format!("<color={YELLOW}>1.02.5</color>"),
                distance: Some(format!("<color={YELLOW}>1000m</color>")),
                running: false,
            }
        );

        run(&mut timing, 64.0, 70.0, course);
        assert!(matches!(view(70.0, &timing, course, k, None), View::Running(_)));

        run(&mut timing, 70.0, 130.0, course);
        assert_eq!(
            render(&view(130.0, &timing, course, k, Some(116.13))).unwrap(),
            Texts { time: "<color=#FFFFFF>1.56.1</color>".into(), distance: None, running: false }
        );
    }

    #[test]
    fn seeking_back_forgets_future_crossings() {
        let mut timing = Timing::default();
        run(&mut timing, 0.0, 130.0, 2000.0);
        timing.step(30.0, 480.0, 2000.0);
        assert_eq!(timing.split_raw, None);
        assert_eq!(timing.finish_raw, None);
    }

    #[test]
    fn jump_past_the_split_does_not_invent_a_split() {
        let mut timing = Timing::default();
        timing.step(10.0, 160.0, 2000.0);
        timing.step(70.0, 1120.0, 2000.0);
        assert_eq!(timing.split_raw, None);
    }

    #[test]
    fn scale_factor() {
        assert_eq!(scale(Some(118.0), Some(100.0)), 1.18);
        assert_eq!(scale(Some(118.0), None), 1.0);
        assert_eq!(scale(Some(500.0), Some(100.0)), 1.0);
    }
}
