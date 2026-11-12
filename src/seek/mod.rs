//! Seek bar: drag to jump anywhere in a race replay.
//!
//! Our own version of Hachimi's `race_playback_slider`, which never shows on
//! Global: Hachimi keeps the scene id from a `SceneManager::AlterUpdate` hook
//! that the Global client does not run during races, so its race-scene check
//! always fails. The seek itself follows Hachimi's sequence (`seek_sync` in
//! its `RaceManagerReplayBase.rs`).
//!
//! Three threads meet here:
//! - `game` (main thread, once per race frame from clock's AlterUpdate hook):
//!   publishes the replay state (`publish`) and applies `Commands`.
//! - `input` (window thread, Windows): mouse messages -> `on_down`/`on_move`/
//!   `on_up`, which write `Commands`; it swallows the messages that hit the bar.
//! - the overlay renderer (render thread): `next_frame` says what to draw,
//!   `set_geometry` says where it was drawn, for hit testing.
//!
//! The bar shows while the mouse moves (`SHOW_FOR` after the last move), while
//! hovered, dragging or paused, and fades out otherwise: it stays off the
//! broadcast picture unless someone reaches for it.

pub mod draw;
pub mod game;
#[cfg(windows)]
pub mod input;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use egui::{Pos2, Rect};

use crate::settings::{self, SEEK_BAR};

/// Replay state published by the main thread each race frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Replay {
    /// Seconds into the replay (raw reader time, like Hachimi's slider).
    pub cur: f32,
    /// Length of the replay data.
    pub total: f32,
    pub paused: bool,
    /// Seeking allowed now: not during the start dash or a skill cut-in.
    pub interactable: bool,
    /// Playback speed (one of `SPEEDS`).
    pub speed: f32,
}

/// Older than this = no race on screen (the per-frame hook stopped).
const STALE: Duration = Duration::from_millis(500);
static REPLAY: Mutex<Option<(Replay, Instant)>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Main thread: `None` = nothing to seek (no replay, race over, bar off).
pub(crate) fn publish(r: Option<Replay>) {
    *lock(&REPLAY) = r.map(|r| (r, Instant::now()));
}

fn replay() -> Option<Replay> {
    lock(&REPLAY).filter(|(_, at)| at.elapsed() < STALE).map(|(r, _)| r)
}

// ---------------------------------------------------------------- commands

/// What the mouse asked for, applied by the main thread in this order.
#[derive(Debug, Default, PartialEq)]
pub struct Commands {
    /// A drag started: pause the race (remembering we did).
    pub begin: bool,
    /// Jump here (seconds). Only the latest matters.
    pub target: Option<f32>,
    /// The drag ended: resume if the drag paused the race.
    pub end: bool,
    /// The play/pause button was clicked.
    pub toggle_pause: bool,
    /// New playback speed.
    pub speed: Option<f32>,
}

impl Commands {
    pub fn is_empty(&self) -> bool {
        *self == Commands::default()
    }
}

static COMMANDS: Mutex<Commands> = Mutex::new(Commands { begin: false, target: None, end: false, toggle_pause: false, speed: None });

/// Main thread: runs `f` on the pending commands (it takes what it applies).
pub(crate) fn with_commands<R>(f: impl FnOnce(&mut Commands) -> R) -> R {
    f(&mut lock(&COMMANDS))
}

// ---------------------------------------------------------------- speed

/// Playback speeds, slowest first.
pub const SPEEDS: [f32; 6] = [0.25, 0.5, 0.75, 1.0, 2.0, 4.0];

/// The speed one step slower (`dir` < 0) or faster (`dir` > 0) than `cur`,
/// staying at the ends.
pub fn step_speed(cur: f32, dir: i32) -> f32 {
    let i = SPEEDS
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - cur).abs().total_cmp(&(b.1 - cur).abs()))
        .map_or(3, |(i, _)| i) as i32;
    SPEEDS[(i + dir.signum()).clamp(0, SPEEDS.len() as i32 - 1) as usize]
}

/// "0.25x", "0.5x", "1.0x", "4.0x"
pub fn fmt_speed(s: f32) -> String {
    if ((s * 10.0).round() - s * 10.0).abs() < 1e-4 { format!("{s:.1}x") } else { format!("{s:.2}x") }
}

// ---------------------------------------------------------------- geometry

/// Where the bar is, in back-buffer pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geom {
    /// Pixels per 1080p pixel.
    pub s: f32,
    pub panel: Rect,
    /// Play/pause button.
    pub button: Rect,
    /// Speed controls: one step slower, the current speed (click = 1.0x), one step faster.
    pub slower: Rect,
    pub speed: Rect,
    pub faster: Rect,
    /// Left edge of the time text.
    pub text_x: f32,
    pub track_x0: f32,
    pub track_x1: f32,
    pub track_y: f32,
}

const PANEL_WIDTH: f32 = 0.6; // of the screen width
const PANEL_HEIGHT: f32 = 48.0; // 1080p pixels, like the rest below
const BOTTOM_MARGIN: f32 = 36.0;
const PAD: f32 = 16.0;
const BUTTON: f32 = 28.0;
const SPEED_BUTTON: f32 = 24.0;
const SPEED_LABEL: f32 = 60.0;
const TEXT_WIDTH: f32 = 170.0;
/// Extra height around the panel that still counts as a hit.
const HIT_SLOP: f32 = 10.0;

/// Bar layout for a `w` x `h` pixel screen.
pub fn geometry(w: f32, h: f32) -> Geom {
    let s = (h / 1080.0).max(0.1);
    let width = (w * PANEL_WIDTH).max(640.0 * s).min(w - 2.0 * PAD * s);
    let x0 = (w - width) / 2.0;
    let y1 = h - BOTTOM_MARGIN * s;
    let panel = Rect::from_min_max(Pos2::new(x0, y1 - PANEL_HEIGHT * s), Pos2::new(x0 + width, y1));
    let cy = panel.center().y;
    let bx = panel.min.x + PAD * s;
    let button = Rect::from_min_max(Pos2::new(bx, cy - BUTTON * s / 2.0), Pos2::new(bx + BUTTON * s, cy + BUTTON * s / 2.0));
    let row = |x: f32, width: f32, height: f32| {
        Rect::from_min_max(Pos2::new(x, cy - height * s / 2.0), Pos2::new(x + width * s, cy + height * s / 2.0))
    };
    let slower = row(button.max.x + 12.0 * s, SPEED_BUTTON, SPEED_BUTTON);
    let speed = row(slower.max.x + 2.0 * s, SPEED_LABEL, SPEED_BUTTON);
    let faster = row(speed.max.x + 2.0 * s, SPEED_BUTTON, SPEED_BUTTON);
    let text_x = faster.max.x + 16.0 * s;
    let track_x0 = text_x + TEXT_WIDTH * s;
    let track_x1 = (panel.max.x - PAD * s - 6.0 * s).max(track_x0 + 1.0);
    Geom { s, panel, button, slower, speed, faster, text_x, track_x0, track_x1, track_y: cy }
}

impl Geom {
    /// Area that takes clicks (panel plus a little slop).
    pub fn hit(&self) -> Rect {
        self.panel.expand2(egui::vec2(0.0, HIT_SLOP * self.s))
    }

    /// What is under `p` (within the panel's hit area).
    pub fn part_at(&self, p: Pos2) -> Part {
        let near = |r: Rect| r.expand(4.0 * self.s).contains(p);
        if !self.hit().contains(p) {
            Part::None
        } else if near(self.button) {
            Part::PlayPause
        } else if near(self.slower) {
            Part::Slower
        } else if near(self.speed) {
            Part::Speed
        } else if near(self.faster) {
            Part::Faster
        } else if p.x >= self.text_x {
            Part::Track
        } else {
            Part::Panel
        }
    }

    /// Replay time under pixel column `x`.
    pub fn time_at(&self, x: f32, total: f32) -> f32 {
        ((x - self.track_x0) / (self.track_x1 - self.track_x0)).clamp(0.0, 1.0) * total.max(0.0)
    }

    /// Pixel column of replay time `t`.
    pub fn x_at(&self, t: f32, total: f32) -> f32 {
        let f = if total > 0.0 { (t / total).clamp(0.0, 1.0) } else { 0.0 };
        self.track_x0 + f * (self.track_x1 - self.track_x0)
    }
}

/// A part of the bar, for clicks and hover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    None,
    /// The panel, but nothing on it.
    Panel,
    PlayPause,
    Slower,
    Speed,
    Faster,
    /// The track and the time text: click or drag to seek.
    Track,
}

/// "1:05.3" (tenths truncated).
pub fn fmt_time(t: f32) -> String {
    let tenths = (t.max(0.0) * 10.0 + 1e-3).floor() as u32;
    let s = tenths / 10;
    format!("{}:{:02}.{}", s / 60, s % 60, tenths % 10)
}

// ---------------------------------------------------------------- pointer

/// How long the bar stays after the mouse last moved.
pub const SHOW_FOR: Duration = Duration::from_millis(2500);
/// Fade in/out time.
const FADE: f32 = 0.2;

#[derive(Default)]
struct Pointer {
    /// Last position, back-buffer pixels.
    pos: Option<Pos2>,
    last_move: Option<Instant>,
    dragging: bool,
    drag_time: f32,
    /// A button-down we swallowed: swallow its button-up too.
    swallowing: bool,
}

static POINTER: Mutex<Pointer> =
    Mutex::new(Pointer { pos: None, last_move: None, dragging: false, drag_time: 0.0, swallowing: false });

/// Drawn layout and back-buffer size, set by the renderer while the bar is on screen.
static GEOM: Mutex<Option<(Geom, (u32, u32))>> = Mutex::new(None);
/// Last back-buffer size seen; kept while the bar is hidden, so the pointer
/// is still tracked then.
static BUFFER: Mutex<Option<(u32, u32)>> = Mutex::new(None);
/// The bar is visible enough to take clicks.
static CLICKABLE: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_geometry(g: Option<(Geom, (u32, u32))>, clickable: bool) {
    if let Some((_, size)) = g {
        *lock(&BUFFER) = Some(size);
    }
    *lock(&GEOM) = g;
    CLICKABLE.store(clickable && g.is_some(), Ordering::Relaxed);
}

/// Back-buffer size the pointer coordinates should be scaled to.
pub(crate) fn buffer_size() -> Option<(u32, u32)> {
    *lock(&BUFFER)
}

fn geom() -> Option<Geom> {
    lock(&GEOM).map(|(g, _)| g)
}

/// Mouse moved, to `p` (back-buffer pixels) if it can be placed yet: any
/// movement shows the bar, even before the first frame told us the buffer
/// size. True = swallow (we are dragging).
pub(crate) fn on_move(p: Option<Pos2>) -> bool {
    let mut ptr = lock(&POINTER);
    ptr.last_move = Some(Instant::now());
    let Some(p) = p else { return ptr.dragging };
    ptr.pos = Some(p);
    if !ptr.dragging {
        return false;
    }
    if let (Some(g), Some(r)) = (geom(), replay()) {
        ptr.drag_time = g.time_at(p.x, r.total);
        with_commands(|c| c.target = Some(ptr.drag_time));
    }
    true
}

/// Left button down at `p`. True = it hit the bar (swallow it).
pub(crate) fn on_down(p: Pos2) -> bool {
    let mut ptr = lock(&POINTER);
    ptr.pos = Some(p);
    ptr.last_move = Some(Instant::now());
    let (Some(g), Some(r)) = (geom(), replay()) else { return false };
    let part = g.part_at(p);
    if !CLICKABLE.load(Ordering::Relaxed) || part == Part::None {
        return false;
    }
    ptr.swallowing = true;
    // Speed works any time; pause and seek wait for the start dash and cut-ins.
    let speed = match part {
        Part::Slower => Some(step_speed(r.speed, -1)),
        Part::Faster => Some(step_speed(r.speed, 1)),
        Part::Speed => Some(1.0),
        _ => None,
    };
    if let Some(s) = speed {
        with_commands(|c| c.speed = Some(s));
        return true;
    }
    if !r.interactable || part == Part::Panel {
        return true;
    }
    if part == Part::PlayPause {
        with_commands(|c| c.toggle_pause = true);
        return true;
    }
    ptr.dragging = true;
    ptr.drag_time = g.time_at(p.x, r.total);
    with_commands(|c| {
        c.begin = true;
        c.target = Some(ptr.drag_time);
    });
    true
}

/// Left button up at `p`. True = swallow (it ends something we started).
pub(crate) fn on_up(p: Pos2) -> bool {
    let mut ptr = lock(&POINTER);
    ptr.pos = Some(p);
    let swallow = std::mem::take(&mut ptr.swallowing);
    if ptr.dragging {
        ptr.dragging = false;
        if let (Some(g), Some(r)) = (geom(), replay()) {
            ptr.drag_time = g.time_at(p.x, r.total);
            with_commands(|c| c.target = Some(ptr.drag_time));
        }
        with_commands(|c| c.end = true);
        return true;
    }
    swallow
}

/// Focus or mouse capture lost: finish any drag where it is.
pub(crate) fn on_cancel() {
    let mut ptr = lock(&POINTER);
    ptr.swallowing = false;
    if std::mem::take(&mut ptr.dragging) {
        with_commands(|c| c.end = true);
    }
}

/// True while a drag is in progress (the window keeps the mouse captured).
pub(crate) fn dragging() -> bool {
    lock(&POINTER).dragging
}

/// Whether `p` is over the bar while it takes clicks.
pub(crate) fn over_bar(p: Pos2) -> bool {
    CLICKABLE.load(Ordering::Relaxed) && geom().is_some_and(|g| g.hit().contains(p))
}

// ---------------------------------------------------------------- frame

/// What the renderer draws this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub replay: Replay,
    /// Time to show (the drag position while dragging).
    pub shown_time: f32,
    pub dragging: bool,
    /// Part under the mouse.
    pub hover: Part,
    /// 0..1
    pub alpha: f32,
}

/// Whether the bar wants to be on screen.
pub fn wants_visible(since_move: Option<Duration>, hover: bool, dragging: bool, paused: bool) -> bool {
    dragging || hover || paused || since_move.is_some_and(|d| d < SHOW_FOR)
}

/// Moves `alpha` towards `target` (0 or 1) over `FADE` seconds.
pub fn fade(alpha: f32, target: bool, dt: f32) -> f32 {
    let step = if FADE > 0.0 { dt.max(0.0) / FADE } else { 1.0 };
    if target { (alpha + step).min(1.0) } else { (alpha - step).max(0.0) }
}

static ALPHA: Mutex<(f32, Option<Instant>)> = Mutex::new((0.0, None));

/// Render thread, every frame: None = draw nothing.
pub fn next_frame() -> Option<Frame> {
    let replay = replay().filter(|_| settings::is_on(SEEK_BAR));
    let mut alpha = lock(&ALPHA);
    let now = Instant::now();
    let dt = alpha.1.map_or(0.0, |t| now.duration_since(t).as_secs_f32()).min(0.1);
    alpha.1 = Some(now);

    let Some(replay) = replay else {
        alpha.0 = 0.0;
        set_geometry(None, false);
        return None;
    };
    let g = geom();
    let ptr = lock(&POINTER);
    let hover = ptr.pos.zip(g).map_or(Part::None, |(p, g)| g.part_at(p));
    let hover_panel = hover != Part::None;
    let since_move = ptr.last_move.map(|t| now.duration_since(t));
    let target = wants_visible(since_move, hover_panel && alpha.0 > 0.0, ptr.dragging, replay.paused);
    alpha.0 = fade(alpha.0, target, dt);
    if alpha.0 <= 0.0 {
        set_geometry(None, false);
        return None;
    }
    Some(Frame {
        replay,
        shown_time: if ptr.dragging { ptr.drag_time } else { replay.cur },
        dragging: ptr.dragging,
        hover: if alpha.0 > 0.3 { hover } else { Part::None },
        alpha: alpha.0,
    })
}

/// Text under the checkbox in the plugin menu.
pub fn status() -> String {
    if !settings::is_on(SEEK_BAR) {
        return "off".to_owned();
    }
    if let Some(e) = game::error() {
        return format!("not available: {e}");
    }
    if !cfg!(windows) {
        return "Windows only".to_owned();
    }
    if let Some(e) = input_error() {
        return format!("no mouse input: {e}");
    }
    match replay() {
        Some(r) => format!("move the mouse to show it ({} / {}, {})", fmt_time(r.cur), fmt_time(r.total), fmt_speed(r.speed)),
        None => "shows during race replays when the mouse moves".to_owned(),
    }
}

fn input_error() -> Option<String> {
    #[cfg(windows)]
    {
        input::error()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_and_x_round_trip() {
        let g = geometry(1920.0, 1080.0);
        assert!(g.panel.min.x > 0.0 && g.panel.max.x < 1920.0);
        assert!(g.track_x0 < g.track_x1);
        assert_eq!(g.time_at(g.track_x0 - 50.0, 90.0), 0.0);
        assert_eq!(g.time_at(g.track_x1 + 50.0, 90.0), 90.0);
        let x = g.x_at(30.0, 90.0);
        assert!((g.time_at(x, 90.0) - 30.0).abs() < 1e-3);
        assert_eq!(g.x_at(5.0, 0.0), g.track_x0);
    }

    #[test]
    fn geometry_scales_with_height() {
        let a = geometry(1920.0, 1080.0);
        let b = geometry(3840.0, 2160.0);
        assert!((b.panel.height() - 2.0 * a.panel.height()).abs() < 1e-3);
        assert!((b.panel.width() - 2.0 * a.panel.width()).abs() < 1e-3);
        // Narrow window: the panel still fits.
        let n = geometry(500.0, 1080.0);
        assert!(n.panel.min.x >= 0.0 && n.panel.max.x <= 500.0);
        assert!(n.track_x1 > n.track_x0);
    }

    #[test]
    fn button_and_track_inside_panel() {
        let g = geometry(1600.0, 900.0);
        assert!(g.panel.contains_rect(g.button));
        assert!(g.button.max.x < g.text_x && g.text_x < g.track_x0);
        assert!(g.track_x1 <= g.panel.max.x);
        assert!(g.hit().contains_rect(g.panel));
    }

    #[test]
    fn speed_controls_in_a_row() {
        let g = geometry(1920.0, 1080.0);
        assert!(g.button.max.x < g.slower.min.x);
        assert!(g.slower.max.x <= g.speed.min.x && g.speed.max.x <= g.faster.min.x);
        assert!(g.faster.max.x < g.text_x);
        assert_eq!(g.part_at(g.button.center()), Part::PlayPause);
        assert_eq!(g.part_at(g.slower.center()), Part::Slower);
        assert_eq!(g.part_at(g.speed.center()), Part::Speed);
        assert_eq!(g.part_at(g.faster.center()), Part::Faster);
        assert_eq!(g.part_at(Pos2::new(g.x_at(10.0, 90.0), g.track_y)), Part::Track);
        assert_eq!(g.part_at(Pos2::new(g.panel.center().x, g.panel.min.y - 100.0)), Part::None);
    }

    #[test]
    fn speed_steps() {
        assert_eq!(step_speed(1.0, 1), 2.0);
        assert_eq!(step_speed(2.0, 1), 4.0);
        assert_eq!(step_speed(4.0, 1), 4.0, "stays at the top");
        assert_eq!(step_speed(1.0, -1), 0.75);
        assert_eq!(step_speed(0.5, -1), 0.25);
        assert_eq!(step_speed(0.25, -1), 0.25, "stays at the bottom");
        assert_eq!(step_speed(1.01, 1), 2.0, "snaps to the nearest step first");
    }

    #[test]
    fn formats_speed() {
        let shown: Vec<String> = SPEEDS.iter().map(|&s| fmt_speed(s)).collect();
        assert_eq!(shown, ["0.25x", "0.5x", "0.75x", "1.0x", "2.0x", "4.0x"]);
    }

    #[test]
    fn formats_time() {
        assert_eq!(fmt_time(0.0), "0:00.0");
        assert_eq!(fmt_time(5.29), "0:05.2");
        assert_eq!(fmt_time(65.3), "1:05.3");
        assert_eq!(fmt_time(-1.0), "0:00.0");
    }

    #[test]
    fn visibility() {
        let recent = Some(Duration::from_millis(100));
        let old = Some(SHOW_FOR + Duration::from_millis(1));
        assert!(wants_visible(recent, false, false, false));
        assert!(!wants_visible(old, false, false, false));
        assert!(!wants_visible(None, false, false, false));
        assert!(wants_visible(old, true, false, false));
        assert!(wants_visible(None, false, true, false));
        assert!(wants_visible(None, false, false, true), "stays while paused");
    }

    #[test]
    fn fades() {
        assert_eq!(fade(0.0, true, 1.0), 1.0);
        assert_eq!(fade(1.0, false, 1.0), 0.0);
        let half = fade(0.0, true, FADE / 2.0);
        assert!((half - 0.5).abs() < 1e-4);
        assert_eq!(fade(0.3, true, -1.0), 0.3);
    }

    #[test]
    fn commands_empty() {
        assert!(Commands::default().is_empty());
        assert!(!Commands { end: true, ..Default::default() }.is_empty());
    }
}
