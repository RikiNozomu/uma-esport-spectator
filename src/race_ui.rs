//! "Hide race UI": from the fanfare (gate-in) through the scoreboard, until the winning circle,
//! the game's race HUD is hidden: buttons, rank/character icon, the marker
//! over the player's horse, commentary, message plates, ...
//!
//! Two layers:
//!   1. Switches: `void SetVisibleX(bool)` methods on the race UI objects.
//!      Each is hooked: we record what the game asks for (`WANTED`) and, while
//!      hiding, pass `false` instead. Turning hiding on calls `X(false)` for
//!      the elements the game has shown; turning it off replays the recorded
//!      values. Elements the game never showed are never touched, so we never
//!      poke a UI object that does not exist.
//!   2. Whole view: the race HUD view is faded out with a CanvasGroup, for
//!      parts that are visible by default and never go through a switch.
//!
//! All calls happen on the main thread: the switches are called by the game
//! there, and `tick` runs from the per-frame AlterUpdate hook (clock.rs).

use std::ffi::c_void;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::il2cpp::{self, Method, Obj, CORE, GAME, UI_MODULE};
use crate::settings::{self, HIDE_UI};
use crate::{api, sig};

const CONTROLLER_CLASS: &str = "RaceMainViewController";

/// (class, method): all `void Method(bool)` on Gallop classes.
const TARGETS: [(&str, &str); 14] = [
    (CONTROLLER_CLASS, "SetVisibleJikkyoWindow"),       // commentary / subtitles
    (CONTROLLER_CLASS, "SetVisibleSkipButton"),
    (CONTROLLER_CLASS, "SetVisiblePauseButton"),        // menu button
    (CONTROLLER_CLASS, "SetVisibleFastForwardButton"),  // ">> Off"
    (CONTROLLER_CLASS, "SetVisibleCameraButton"),       // "Photo"
    (CONTROLLER_CLASS, "SetVisibleMarkers"),
    (CONTROLLER_CLASS, "SetVisibleRank"),               // rank / character icon, bottom left
    (CONTROLLER_CLASS, "SetVisibleMiniMap"),
    (CONTROLLER_CLASS, "SetVisibleMessagePlate"),
    (CONTROLLER_CLASS, "SetVisibleRaceUIStatusPlate"),
    (CONTROLLER_CLASS, "SetVisibleSkillInfo"),
    (CONTROLLER_CLASS, "SetVisibleTeamStadiumScore"),
    (CONTROLLER_CLASS, "SetVisibleChallengeMatchPoint"),
    ("RaceViewReplay", "VisibleMarkers"),               // 3D marker above the player's horse
];
const N: usize = TARGETS.len();

const UNKNOWN: u8 = 0;
const HIDDEN: u8 = 1;
const SHOWN: u8 = 2;

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicUsize = AtomicUsize::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const NOT_SEEN: AtomicU8 = AtomicU8::new(UNKNOWN);
#[allow(clippy::declare_interior_mutable_const)]
const ZERO64: AtomicU64 = AtomicU64::new(0);

static ORIG: [AtomicUsize; N] = [ZERO; N];
static INFO: [AtomicUsize; N] = [ZERO; N];
/// What the game last asked each switch for, and on which object.
static WANTED: [AtomicU8; N] = [NOT_SEEN; N];
static THIS: [AtomicUsize; N] = [ZERO; N];
/// When each switch was last called (from SEQ). `apply` only replays calls
/// made since the current race started: an older `THIS` may belong to a
/// destroyed race view, and calling into it crashes the game.
static STAMP: [AtomicU64; N] = [ZERO64; N];
static SEQ: AtomicU64 = AtomicU64::new(0);
/// First SEQ of the current race; u64::MAX = no race, nothing may be replayed.
static RACE_SEQ: AtomicU64 = AtomicU64::new(u64::MAX);
/// Bumped at every race start (fanfare). Other features reset on a change.
static RACE_GEN: AtomicU32 = AtomicU32::new(0);
/// Race manager of the current race (a new one = a new race).
static RACE: AtomicUsize = AtomicUsize::new(0);
/// Between StateGateIn_Enter (fanfare) and StateWinningCircleInit_Enter / race end.
static RACING: AtomicBool = AtomicBool::new(false);
/// Scoreboard is up (StateOverRunResult_Enter): still hidden, but taps go
/// through, in case the game waits for one to move on.
static TAPS_ALLOWED: AtomicBool = AtomicBool::new(false);
/// Hiding is currently applied.
static HIDING: AtomicBool = AtomicBool::new(false);
static STATUS: OnceLock<String> = OnceLock::new();

// ---------------------------------------------------------------- switches

fn call(i: usize, this: Obj, visible: bool, mi: *const c_void) {
    let t = loop {
        let t = ORIG[i].load(Ordering::Acquire);
        if t != 0 {
            break t;
        }
        std::hint::spin_loop();
    };
    let f: extern "C" fn(Obj, bool, *const c_void) = unsafe { std::mem::transmute(t) };
    f(this, visible, mi)
}

extern "C" fn set_visible<const I: usize>(this: Obj, visible: bool, mi: *const c_void) {
    THIS[I].store(this as usize, Ordering::Relaxed);
    WANTED[I].store(if visible { SHOWN } else { HIDDEN }, Ordering::Relaxed);
    STAMP[I].store(SEQ.fetch_add(1, Ordering::Relaxed) + 1, Ordering::Relaxed);
    call(I, this, visible && !HIDING.load(Ordering::Relaxed), mi);
}

/// Current race number; changes at every fanfare.
pub fn race_gen() -> u32 {
    RACE_GEN.load(Ordering::Relaxed)
}

/// A race starts: switch calls from before now are off limits for `apply`.
fn mark_race_start() {
    RACE_SEQ.store(SEQ.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
    RACE_GEN.fetch_add(1, Ordering::Relaxed);
}

fn detours() -> [usize; N] {
    [
        set_visible::<0> as *const () as usize,
        set_visible::<1> as *const () as usize,
        set_visible::<2> as *const () as usize,
        set_visible::<3> as *const () as usize,
        set_visible::<4> as *const () as usize,
        set_visible::<5> as *const () as usize,
        set_visible::<6> as *const () as usize,
        set_visible::<7> as *const () as usize,
        set_visible::<8> as *const () as usize,
        set_visible::<9> as *const () as usize,
        set_visible::<10> as *const () as usize,
        set_visible::<11> as *const () as usize,
        set_visible::<12> as *const () as usize,
        set_visible::<13> as *const () as usize,
    ]
}

/// Pushes the hidden (or the game's own) state to every element the game has
/// shown during the current race.
fn apply(hide: bool) {
    let race_seq = RACE_SEQ.load(Ordering::Relaxed);
    for i in 0..N {
        let this = THIS[i].load(Ordering::Relaxed) as Obj;
        let current = STAMP[i].load(Ordering::Relaxed) >= race_seq;
        if current && !this.is_null() && ORIG[i].load(Ordering::Acquire) != 0 && WANTED[i].load(Ordering::Relaxed) == SHOWN
        {
            call(i, this, !hide, INFO[i].load(Ordering::Relaxed) as *const c_void);
        }
    }
}

// ---------------------------------------------------------------- whole view

/// Second layer: some HUD parts are visible by default and never go through
/// a switch (progress bar, rank icon, commentary). Fading the whole race view
/// (`RaceMainView`) with a CanvasGroup covers them. The clock is on the
/// notice canvas, outside this view, so it stays.
struct ViewFns {
    get_race_main_view: Method,
    get_main_view: Method,
    component_game_object: usize,
    get_component: Method,
    add_component: Method,
    canvas_group_type: usize,
    get_alpha: Method,
    set_alpha: Method,
    get_blocks_raycasts: Method,
    set_blocks_raycasts: Method,
}

static VIEW: OnceLock<Result<ViewFns, String>> = OnceLock::new();

impl ViewFns {
    unsafe fn resolve() -> Result<ViewFns, String> {
        use il2cpp::{class, expect, icall, method, method_params};
        let rm = class(GAME, "Gallop", "RaceManager")?;
        let ctrl = class(GAME, "Gallop", CONTROLLER_CLASS)?;
        let go = class(CORE, "UnityEngine", "GameObject")?;
        let cg = class(UI_MODULE, "UnityEngine", "CanvasGroup")?;

        let get_race_main_view = method(rm, "get_RaceMainView", 0)?;
        expect(get_race_main_view, false, &["Gallop.RaceMainViewController"], 0)?;
        let get_main_view = method(ctrl, "get_MainView", 0)?;
        expect(get_main_view, false, &["Gallop.RaceMainView"], 0)?;
        let get_component = method_params(go, "GetComponent", &["System.Type"])?;
        let add_component = method_params(go, "AddComponent", &["System.Type"])?;
        let get_alpha = method(cg, "get_alpha", 0)?;
        expect(get_alpha, false, &["System.Single"], 0)?;
        let set_alpha = method(cg, "set_alpha", 1)?;
        let get_blocks_raycasts = method(cg, "get_blocksRaycasts", 0)?;
        expect(get_blocks_raycasts, false, &["System.Boolean"], 0)?;
        let set_blocks_raycasts = method(cg, "set_blocksRaycasts", 1)?;
        Ok(ViewFns {
            get_race_main_view,
            get_main_view,
            component_game_object: icall("UnityEngine.Component::get_gameObject()")?,
            get_component,
            add_component,
            canvas_group_type: il2cpp::type_object(cg)? as usize,
            get_alpha,
            set_alpha,
            get_blocks_raycasts,
            set_blocks_raycasts,
        })
    }

    unsafe fn getter(m: Method, this: Obj) -> Obj {
        let f: extern "C" fn(Obj, usize) -> Obj = std::mem::transmute(m.code);
        f(this, m.info)
    }

    unsafe fn with_type(m: Method, this: Obj, ty: Obj) -> Obj {
        let f: extern "C" fn(Obj, Obj, usize) -> Obj = std::mem::transmute(m.code);
        f(this, ty, m.info)
    }

    /// Hides (alpha 0; clicks blocked unless `taps`) or restores the race
    /// view. Cheap enough to run every frame, which re-hides it if the game
    /// fades it back in.
    unsafe fn set_hidden(&self, rm: Obj, hide: bool, taps: bool) {
        if rm.is_null() {
            return;
        }
        let ctrl = Self::getter(self.get_race_main_view, rm);
        if ctrl.is_null() {
            return;
        }
        let view = Self::getter(self.get_main_view, ctrl);
        if view.is_null() {
            return;
        }
        let go_of: extern "C" fn(Obj) -> Obj = std::mem::transmute(self.component_game_object);
        let go = go_of(view);
        if go.is_null() {
            return;
        }
        let ty = self.canvas_group_type as Obj;
        let mut cg = Self::with_type(self.get_component, go, ty);
        if cg.is_null() {
            if !hide {
                return;
            }
            cg = Self::with_type(self.add_component, go, ty);
            if cg.is_null() {
                return;
            }
        }
        let get_alpha: extern "C" fn(Obj, usize) -> f32 = std::mem::transmute(self.get_alpha.code);
        let target = if hide { 0.0 } else { 1.0 };
        if get_alpha(cg, self.get_alpha.info) != target {
            let set_alpha: extern "C" fn(Obj, f32, usize) = std::mem::transmute(self.set_alpha.code);
            set_alpha(cg, target, self.set_alpha.info);
        }
        let get_blocks: extern "C" fn(Obj, usize) -> bool = std::mem::transmute(self.get_blocks_raycasts.code);
        let blocks = !hide || taps;
        if get_blocks(cg, self.get_blocks_raycasts.info) != blocks {
            let set_blocks: extern "C" fn(Obj, bool, usize) = std::mem::transmute(self.set_blocks_raycasts.code);
            set_blocks(cg, blocks, self.set_blocks_raycasts.info);
        }
    }
}

fn set_view_hidden(rm: Obj, hide: bool) {
    if let Some(Ok(v)) = VIEW.get() {
        unsafe { v.set_hidden(rm, hide, TAPS_ALLOWED.load(Ordering::Relaxed)) }
    }
}

// ---------------------------------------------------------------- race phase

/// Once per frame during a race (main thread, from clock.rs). `rm` = race manager.
pub fn tick(rm: Obj) {
    if !rm.is_null() && RACE.swap(rm as usize, Ordering::Relaxed) != rm as usize {
        // A new race: whatever the last one left behind is stale.
        RACING.store(false, Ordering::Relaxed);
        TAPS_ALLOWED.store(false, Ordering::Relaxed);
        HIDING.store(false, Ordering::Relaxed);
    }
    let hide = settings::is_on(HIDE_UI) && RACING.load(Ordering::Relaxed);
    if hide != HIDING.load(Ordering::Relaxed) {
        HIDING.store(hide, Ordering::Relaxed);
        apply(hide);
        if !hide {
            set_view_hidden(rm, false);
        }
    }
    if hide {
        set_view_hidden(rm, true);
    }
}

/// The race part is over but the scene goes on (winning circle): give the UI back.
pub fn race_over(rm: Obj) {
    RACING.store(false, Ordering::Relaxed);
    TAPS_ALLOWED.store(false, Ordering::Relaxed);
    tick(rm);
}

/// The race scene is closing: nothing to restore, just stop hiding.
pub fn race_ended(_rm: Obj) {
    RACING.store(false, Ordering::Relaxed);
    TAPS_ALLOWED.store(false, Ordering::Relaxed);
    HIDING.store(false, Ordering::Relaxed);
    // The race view is going away: never call into its objects again.
    RACE_SEQ.store(u64::MAX, Ordering::Relaxed);
}

static ORIG_GATE_IN_ENTER: AtomicUsize = AtomicUsize::new(0);
static ORIG_RESULT_ENTER: AtomicUsize = AtomicUsize::new(0);

fn call_void(slot: &AtomicUsize, this: Obj, mi: *const c_void) {
    let t = loop {
        let t = slot.load(Ordering::Acquire);
        if t != 0 {
            break t;
        }
        std::hint::spin_loop();
    };
    let f: extern "C" fn(Obj, *const c_void) = unsafe { std::mem::transmute(t) };
    f(this, mi)
}

/// The fanfare starts (gate-in sequence, right after the member list).
extern "C" fn gate_in_enter(this: Obj, mi: *const c_void) {
    // Before the original, so switch calls it makes count for this race.
    mark_race_start();
    call_void(&ORIG_GATE_IN_ENTER, this, mi);
    RACING.store(true, Ordering::Relaxed);
    TAPS_ALLOWED.store(false, Ordering::Relaxed);
    tick(this);
}

/// The scoreboard comes in: stay hidden (the UI comes back at the winning
/// circle, see race_over), but let taps through.
extern "C" fn result_enter(this: Obj, mi: *const c_void) {
    call_void(&ORIG_RESULT_ENTER, this, mi);
    TAPS_ALLOWED.store(true, Ordering::Relaxed);
    tick(this);
    // The race clock and the leaderboard end with the race, before the board.
    crate::clock::result_board_shown();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static CALLS: Mutex<Vec<(usize, bool)>> = Mutex::new(Vec::new());

    extern "C" fn fake0(_this: Obj, v: bool, _mi: *const c_void) {
        CALLS.lock().unwrap().push((0, v));
    }
    extern "C" fn fake1(_this: Obj, v: bool, _mi: *const c_void) {
        CALLS.lock().unwrap().push((1, v));
    }
    fn take() -> Vec<(usize, bool)> {
        std::mem::take(&mut *CALLS.lock().unwrap())
    }

    // Single test: the state is global.
    #[test]
    fn hides_while_racing_and_restores_only_what_the_game_showed() {
        ORIG[0].store(fake0 as *const () as usize, Ordering::SeqCst);
        ORIG[1].store(fake1 as *const () as usize, Ordering::SeqCst);
        let ctrl = 0x1000 as Obj;
        let mi = std::ptr::null();
        settings::set_for_test(HIDE_UI, true);

        // A previous race left an object behind; it must never be called again.
        let old = 0x0900 as Obj;
        mark_race_start();
        set_visible::<1>(old, true, mi);
        take();
        race_ended(std::ptr::null_mut());

        // New race. Before the fanfare's hiding: the game's values pass through.
        mark_race_start();
        set_visible::<0>(ctrl, true, mi);
        assert_eq!(take(), [(0, true)]);

        // Fanfare: element 0 (shown this race) is hidden; element 1 (only
        // shown on the old race's object) is not touched.
        RACING.store(true, Ordering::SeqCst);
        tick(std::ptr::null_mut());
        assert_eq!(take(), [(0, false)]);
        set_visible::<1>(ctrl, false, mi);
        take();

        // The game tries to show things during the race: forced off, but remembered.
        set_visible::<1>(ctrl, true, mi);
        assert_eq!(take(), [(1, false)]);

        // Toggle off mid-race: the game's own state comes back.
        settings::set_for_test(HIDE_UI, false);
        tick(std::ptr::null_mut());
        assert_eq!(take(), [(0, true), (1, true)]);

        // Toggle on again, then the winning circle: restored.
        settings::set_for_test(HIDE_UI, true);
        tick(std::ptr::null_mut());
        assert_eq!(take(), [(0, false), (1, false)]);
        race_over(std::ptr::null_mut());
        assert_eq!(take(), [(0, true), (1, true)]);

        // Hiding, then a new race manager shows up: stale hiding is dropped
        // and pre-race calls pass through again.
        RACING.store(true, Ordering::SeqCst);
        tick(0x5000 as Obj);
        take();
        tick(0x6000 as Obj);
        set_visible::<0>(0x2000 as Obj, true, mi);
        assert_eq!(take(), [(0, true)]);
        assert!(!RACING.load(Ordering::SeqCst));

        // After the scene closes, turning hiding on/off calls nothing.
        race_ended(std::ptr::null_mut());
        RACING.store(true, Ordering::SeqCst);
        tick(std::ptr::null_mut());
        assert_eq!(take(), []);
        race_ended(std::ptr::null_mut());
    }
}

// ---------------------------------------------------------------- install

pub fn status() -> String {
    STATUS.get().cloned().unwrap_or_else(|| "waiting for the game".to_owned())
}

pub fn install() {
    let api = api::get();
    let mut failed = Vec::new();

    for (i, (&(class, name), detour)) in TARGETS.iter().zip(detours()).enumerate() {
        let result = unsafe { il2cpp::class(GAME, "Gallop", class) }.and_then(|k| unsafe {
            let m = il2cpp::method(k, name, 1)?;
            il2cpp::expect(m, false, &["System.Void"], 1)?;
            let p = sig::param_type(m.info as *const c_void, 0).unwrap_or_default();
            if p != "System.Boolean" {
                return Err(format!("parameter is {p}, expected System.Boolean"));
            }
            INFO[i].store(m.info, Ordering::Relaxed);
            il2cpp::hook(m.code, detour)
        });
        match result {
            Ok(t) => ORIG[i].store(t, Ordering::Release),
            Err(e) => {
                api.warn(&format!("hide race UI: {class}::{name} not hooked: {e}"));
                failed.push(format!("{name}: {e}"));
            }
        }
    }

    let phases = [
        ("StateGateIn_Enter", gate_in_enter as *const () as usize, &ORIG_GATE_IN_ENTER),
        ("StateOverRunResult_Enter", result_enter as *const () as usize, &ORIG_RESULT_ENTER),
    ];
    for (name, detour, slot) in phases {
        let result = unsafe {
            il2cpp::class(GAME, "Gallop", "RaceManagerReplayBase")
                .and_then(|k| il2cpp::method(k, name, 0))
                .and_then(|m| il2cpp::expect(m, false, &["System.Void"], 0).map(|_| m))
                .and_then(|m| il2cpp::hook(m.code, detour))
        };
        match result {
            Ok(t) => slot.store(t, Ordering::Release),
            Err(e) => {
                api.warn(&format!("hide race UI: RaceManagerReplayBase::{name} not hooked: {e}"));
                failed.push(format!("{name}: {e}"));
            }
        }
    }

    let view = VIEW.get_or_init(|| unsafe { ViewFns::resolve() });
    if let Err(e) = view {
        api.warn(&format!("hide race UI: whole-view hiding unavailable: {e}"));
        failed.push(format!("whole view: {e}"));
    }

    let hooked = ORIG.iter().filter(|o| o.load(Ordering::Relaxed) != 0).count();
    let mut status = if ORIG_GATE_IN_ENTER.load(Ordering::Relaxed) == 0 {
        "FAILED: no race-start hook".to_owned()
    } else {
        format!("ok, {hooked}/{N} UI switches, whole view {}", if view.is_ok() { "ok" } else { "FAILED" })
    };
    for f in &failed {
        let _ = write!(status, "\n{f}");
    }
    api.info(&format!("hide race UI: {}", status.replace('\n', "; ")));
    let _ = STATUS.set(status);
}
