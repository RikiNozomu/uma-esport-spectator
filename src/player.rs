//! Race player: play/pause, seek, fast forward, rewind and slow motion.
//!
//! Buttons (player window) and keyboard shortcuts run on the render thread:
//! they only change `CONTROL`. The race manager's per-frame update (hooked by
//! `clock`, main thread) calls `tick`, which applies it:
//! - speed / pause: `RaceManagerReplayBase::SetDeltaTimeScale` is hooked; the
//!   game's own scale (it slows the finish down by itself) is multiplied by
//!   the player's factor, so neither overrides the other. Pause = factor 0,
//!   which keeps the per-frame update running (seeking still works).
//! - seek: `seek::seek` (the full procedure of Hachimi's playback slider:
//!   race time, horses, models, cameras, commentary), clamped to the race
//!   data (`RaceSimulateReader.GetLastFrameTime`); waits while a skill cut-in
//!   plays, like Hachimi's slider.
//! - rewind: each frame, seek back by real elapsed time x rate.
//!
//! Controls only act while the race is live (gates open .. result board).
//! A new race, or the end of one, resets to 1x so result scenes and the next
//! race play normally.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::il2cpp::{self, Method, Obj};
use crate::{api, clock, race_ui};

const GAME: &str = "umamusume.dll";

/// Playback speeds, slowest first. Index `NORMAL` is 1x.
pub const SPEEDS: [f32; 7] = [0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0];
pub const NORMAL: usize = 3;
/// Rewind rates (race seconds per real second).
pub const REWIND_RATES: [f32; 3] = [1.0, 2.0, 4.0];
/// Seek step for the buttons and arrow keys.
pub const STEP: f32 = 5.0;

// ---------------------------------------------------------------- control (pure)

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    TogglePause,
    Play,
    Pause,
    /// Index into `SPEEDS`; also resumes playback.
    Speed(usize),
    Faster,
    Slower,
    /// Cycles through `REWIND_RATES`, then off.
    Rewind,
    SeekTo(f32),
    SeekBy(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Control {
    pub speed: usize,
    pub paused: bool,
    /// Index into `REWIND_RATES` while rewinding.
    pub rewind: Option<usize>,
    /// Absolute target time, applied (and cleared) on the next tick.
    pub seek: Option<Seek>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seek {
    To(f32),
    By(f32),
}

impl Default for Control {
    fn default() -> Self {
        Control { speed: NORMAL, paused: false, rewind: None, seek: None }
    }
}

impl Control {
    pub fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::TogglePause => {
                if self.rewind.is_some() {
                    self.rewind = None;
                    self.paused = false;
                } else {
                    self.paused = !self.paused;
                }
            }
            Command::Play => {
                self.paused = false;
                self.rewind = None;
            }
            Command::Pause => {
                self.paused = true;
                self.rewind = None;
            }
            Command::Speed(i) => {
                self.speed = i.min(SPEEDS.len() - 1);
                self.paused = false;
                self.rewind = None;
            }
            Command::Faster => self.apply(Command::Speed((self.speed + 1).min(SPEEDS.len() - 1))),
            Command::Slower => self.apply(Command::Speed(self.speed.saturating_sub(1))),
            Command::Rewind => {
                self.rewind = match self.rewind {
                    None => Some(0),
                    Some(i) if i + 1 < REWIND_RATES.len() => Some(i + 1),
                    Some(_) => None,
                };
                // Leaving rewind pauses (the race stays where it got to).
                if self.rewind.is_none() {
                    self.paused = true;
                }
            }
            Command::SeekTo(t) => self.seek = Some(Seek::To(t)),
            Command::SeekBy(d) => {
                self.seek = Some(match self.seek {
                    Some(Seek::By(prev)) => Seek::By(prev + d),
                    _ => Seek::By(d),
                })
            }
        }
    }

    /// Multiplier for the game's time scale.
    pub fn factor(&self) -> f32 {
        if self.paused || self.rewind.is_some() { 0.0 } else { SPEEDS[self.speed] }
    }

    pub fn label(&self) -> String {
        match self.rewind {
            Some(i) => format!("rewinding {}x", REWIND_RATES[i]),
            None if self.paused => "paused".to_owned(),
            None => format!("playing {}x", SPEEDS[self.speed]),
        }
    }
}

/// Where a seek lands, kept inside the race data.
pub fn seek_target(seek: Seek, now: f32, end: f32) -> f32 {
    let t = match seek {
        Seek::To(t) => t,
        Seek::By(d) => now + d,
    };
    t.clamp(0.0, (end - 0.05).max(0.0))
}

static CONTROL: Mutex<Control> = Mutex::new(Control { speed: NORMAL, paused: false, rewind: None, seek: None });

fn control() -> std::sync::MutexGuard<'static, Control> {
    CONTROL.lock().unwrap_or_else(|e| e.into_inner())
}

/// From the window or the keyboard (any thread). Ignored outside a live race.
pub fn send(cmd: Command) {
    if !is_active() {
        return;
    }
    control().apply(cmd);
}

pub fn current() -> Control {
    *control()
}

// ---------------------------------------------------------------- status (any thread)

static TIME_BITS: AtomicU32 = AtomicU32::new(0);
static END_BITS: AtomicU32 = AtomicU32::new(0);

/// Controls are usable: hooks installed and a race is live.
pub fn is_active() -> bool {
    META.get().is_some_and(|m| m.is_ok()) && clock::race_live()
}

/// (current race time, end of the race data), while active.
pub fn position() -> Option<(f32, f32)> {
    is_active().then(|| {
        (f32::from_bits(TIME_BITS.load(Ordering::Relaxed)), f32::from_bits(END_BITS.load(Ordering::Relaxed)))
    })
}

pub fn status() -> String {
    match META.get() {
        None => "waiting for the game".to_owned(),
        Some(Err(e)) => format!("unavailable: {e}"),
        Some(Ok(_)) if !clock::race_live() => "waiting for a race to start".to_owned(),
        Some(Ok(_)) => {
            let (t, end) = position().unwrap_or((0.0, 0.0));
            format!("{:.1} / {:.1} s, {}", t, end, current().label())
        }
    }
}

// ---------------------------------------------------------------- game side

struct Meta {
    last_frame_time: Method,
    reader_off: usize,
    cur_time_off: usize,
}

unsafe impl Send for Meta {}
unsafe impl Sync for Meta {}

static META: OnceLock<Result<Meta, String>> = OnceLock::new();
/// SetDeltaTimeScale trampoline.
static ORIG_SET_SCALE: AtomicUsize = AtomicUsize::new(0);
static SET_SCALE_INFO: AtomicUsize = AtomicUsize::new(0);
/// Last scale the game asked for, and the factor we applied on top (f32 bits).
static GAME_SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0
static APPLIED: AtomicU32 = AtomicU32::new(0x3f80_0000);

/// Per-race bookkeeping (main thread).
struct Tracking {
    race: usize,
    gen: u32,
    last_tick: Option<Instant>,
}

static TRACK: Mutex<Tracking> = Mutex::new(Tracking { race: 0, gen: 0, last_tick: None });

/// Resolves and hooks. Main thread, at game ready.
pub fn install() {
    let result = unsafe { resolve() };
    let api = api::get();
    match &result {
        Ok(_) => api.info("race player ready"),
        Err(e) => api.warn(&format!("race player unavailable: {e}")),
    }
    let _ = META.set(result);
}

unsafe fn resolve() -> Result<Meta, String> {
    use il2cpp::{class, expect, field_offset, method};
    let rm = class(GAME, "Gallop", "RaceManagerReplayBase")?;
    let reader = class(GAME, "Gallop", "RaceSimulateReader")?;

    let set_scale = method(rm, "SetDeltaTimeScale", 1)?;
    expect(set_scale, false, &["System.Void"], 1)?;
    let last_frame_time = method(reader, "GetLastFrameTime", 0)?;
    expect(last_frame_time, false, &["System.Single"], 0)?;
    let reader_off = field_offset(rm, "_simulateReader")?;
    let cur_time_off = field_offset(reader, "_curTime")?;

    SET_SCALE_INFO.store(set_scale.info, Ordering::Relaxed);
    let trampoline = il2cpp::hook(set_scale.code, set_delta_time_scale as SetScaleFn as usize)?;
    ORIG_SET_SCALE.store(trampoline, Ordering::Release);

    // Speed works without seeking; seeking is reported separately.
    if let Err(e) = crate::seek::install() {
        api::get().warn(&format!("race player: seeking unavailable: {e}"));
    }
    Ok(Meta { last_frame_time, reader_off, cur_time_off })
}

type SetScaleFn = extern "C" fn(this: Obj, scale: f32, mi: *const c_void);

/// The game sets its own time scale (e.g. slow motion at the finish): keep
/// it, times the player's factor.
extern "C" fn set_delta_time_scale(this: Obj, scale: f32, mi: *const c_void) {
    GAME_SCALE.store(scale.to_bits(), Ordering::Relaxed);
    let factor = f32::from_bits(APPLIED.load(Ordering::Relaxed));
    call_set_scale(this, scale * factor, mi);
}

fn call_set_scale(this: Obj, scale: f32, mi: *const c_void) {
    let t = ORIG_SET_SCALE.load(Ordering::Acquire);
    if t != 0 {
        let f: SetScaleFn = unsafe { std::mem::transmute(t) };
        f(this, scale, mi);
    }
}

/// Applies `factor` now (main thread, valid race manager).
fn apply_factor(rm: Obj, factor: f32) {
    if f32::from_bits(APPLIED.swap(factor.to_bits(), Ordering::Relaxed)) == factor {
        return;
    }
    let game = f32::from_bits(GAME_SCALE.load(Ordering::Relaxed));
    call_set_scale(rm, game * factor, SET_SCALE_INFO.load(Ordering::Relaxed) as *const c_void);
}

/// Every frame of a race, from clock's AlterUpdate hook (main thread).
pub unsafe fn tick(rm: Obj) {
    let Some(Ok(m)) = META.get() else { return };
    if rm.is_null() {
        return;
    }

    let now = Instant::now();
    let dt = {
        let mut tr = TRACK.lock().unwrap_or_else(|e| e.into_inner());
        let gen = race_ui::race_gen();
        if tr.race != rm as usize || tr.gen != gen {
            // New race: normal playback, the game's scale starts at 1.
            tr.race = rm as usize;
            tr.gen = gen;
            tr.last_tick = None;
            *control() = Control::default();
            GAME_SCALE.store(1f32.to_bits(), Ordering::Relaxed);
            APPLIED.store(1f32.to_bits(), Ordering::Relaxed);
        }
        let dt = tr.last_tick.map(|t| now.duration_since(t).as_secs_f32().min(0.1)).unwrap_or(0.0);
        tr.last_tick = Some(now);
        dt
    };

    let reader: Obj = il2cpp::read(rm, m.reader_off);
    if reader.is_null() {
        return;
    }
    let t: f32 = il2cpp::read(reader, m.cur_time_off);
    let last: extern "C" fn(Obj, usize) -> f32 = std::mem::transmute(m.last_frame_time.code);
    let end = last(reader, m.last_frame_time.info);
    if !(t.is_finite() && end.is_finite() && end > 0.0) {
        return;
    }
    TIME_BITS.store(t.to_bits(), Ordering::Relaxed);
    END_BITS.store(end.to_bits(), Ordering::Relaxed);

    if !clock::race_live() {
        apply_factor(rm, 1.0);
        return;
    }

    // While a skill cut-in plays the game cannot seek: keep the request.
    let busy = crate::seek::busy(rm);
    // Take the pending seek and work out rewinding, then release the lock
    // before calling into the game.
    let (seek, rewind_to, factor) = {
        let mut c = control();
        let seek = if busy { None } else { c.seek.take().map(|s| seek_target(s, t, end)) };
        let mut rewind_to = None;
        if let Some(i) = c.rewind.filter(|_| !busy) {
            let target = t - dt * REWIND_RATES[i];
            if target <= 0.0 {
                c.rewind = None;
                c.paused = true;
            }
            rewind_to = Some(target.max(0.0));
        }
        (seek, rewind_to, c.factor())
    };

    apply_factor(rm, factor);
    if let Some(target) = seek.or(rewind_to) {
        if (target - t).abs() > 1e-4 && !crate::seek::seek(rm, target, target < t) {
            // Faulted or unavailable: stop rewinding, stay where we are.
            let mut c = control();
            c.rewind = None;
            c.paused = true;
        }
    }
}

/// The race ended (result board, winning circle, leaving): drop pending
/// commands. No call into the game here (the scene may be tearing down); the
/// next tick, no longer live, puts the time scale back to 1x.
pub fn race_ended() {
    *control() = Control::default();
}

// ---------------------------------------------------------------- keyboard

const KEY_SHORTCUTS: &str = "player_shortcuts";
static SHORTCUTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn load_settings() {
    SHORTCUTS.store(crate::settings::get_bool(KEY_SHORTCUTS, true), Ordering::Relaxed);
}

pub fn shortcuts_enabled() -> bool {
    SHORTCUTS.load(Ordering::Relaxed)
}

pub fn set_shortcuts(on: bool) {
    SHORTCUTS.store(on, Ordering::Relaxed);
    crate::settings::set_value(KEY_SHORTCUTS, on);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_and_play() {
        let mut c = Control::default();
        assert_eq!(c.factor(), 1.0);
        c.apply(Command::TogglePause);
        assert_eq!(c.factor(), 0.0);
        c.apply(Command::TogglePause);
        assert_eq!(c.factor(), 1.0);
    }

    #[test]
    fn speeds_step_and_clamp() {
        let mut c = Control::default();
        c.apply(Command::Faster);
        c.apply(Command::Faster);
        assert_eq!(c.factor(), 4.0);
        for _ in 0..10 {
            c.apply(Command::Faster);
        }
        assert_eq!(c.factor(), 8.0);
        for _ in 0..20 {
            c.apply(Command::Slower);
        }
        assert_eq!(c.factor(), 0.1, "slowest");
        c.apply(Command::Pause);
        c.apply(Command::Speed(NORMAL));
        assert_eq!(c.factor(), 1.0, "choosing a speed resumes");
    }

    #[test]
    fn rewind_cycles_rates_then_pauses() {
        let mut c = Control::default();
        c.apply(Command::Rewind);
        assert_eq!((c.rewind, c.factor()), (Some(0), 0.0), "game time frozen while rewinding");
        c.apply(Command::Rewind);
        c.apply(Command::Rewind);
        assert_eq!(c.rewind, Some(2));
        c.apply(Command::Rewind);
        assert_eq!((c.rewind, c.paused), (None, true));
        c.apply(Command::Rewind);
        c.apply(Command::TogglePause);
        assert_eq!((c.rewind, c.paused, c.factor()), (None, false, 1.0), "play stops rewinding");
    }

    #[test]
    fn seeks_accumulate_and_clamp() {
        let mut c = Control::default();
        c.apply(Command::SeekBy(5.0));
        c.apply(Command::SeekBy(5.0));
        assert_eq!(c.seek, Some(Seek::By(10.0)), "two key presses in one frame");
        assert_eq!(seek_target(Seek::By(10.0), 30.0, 150.0), 40.0);
        assert_eq!(seek_target(Seek::By(-50.0), 30.0, 150.0), 0.0);
        assert!(seek_target(Seek::To(999.0), 30.0, 150.0) < 150.0);
        c.apply(Command::SeekTo(12.0));
        assert_eq!(c.seek, Some(Seek::To(12.0)), "an absolute seek replaces steps");
    }
}
