//! Track map: a broadcast-style course map in the top-left corner. The race's
//! course drawn as a tilted 3D band (turf green / dirt tan), the goal, every
//! runner as a dot in its bracket colour (the leader as a red marker), the
//! surface and length ("TURF 2,000m") and a stats row (leader's metres left,
//! leader's speed).
//!
//! - `game` (main thread, per race frame from clock's AlterUpdate hook): samples
//!   the course once per race with `RaceManager::GetLaneTransform(distance)`
//!   and reads every runner's world position each frame; publishes `MapState`.
//! - `draw` (render thread): paints `next_frame()` with egui.
//! - here: shared state and the projection (world x/z -> screen), pure.
//!
//! Only the race's own path is sampled (0..course length): long races draw
//! most of the oval, sprints only their part of it.

pub mod draw;
pub mod game;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::{clock, race, settings};

const KEY: &str = "track_map";
static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    settings::set_value(KEY, on);
}

pub fn load_settings() {
    ENABLED.store(settings::get_bool(KEY, true), Ordering::Relaxed);
    let v = View {
        scale: settings::get_f32(KEY_SCALE, DEFAULT_VIEW.scale),
        x: settings::get_f32(KEY_X, DEFAULT_VIEW.x),
        y: settings::get_f32(KEY_Y, DEFAULT_VIEW.y),
    };
    *lock(&VIEW) = v.clamped();
}

const KEY_SCALE: &str = "track_map_scale";
const KEY_X: &str = "track_map_x";
const KEY_Y: &str = "track_map_y";
pub const MIN_SCALE: f32 = 0.5;
pub const MAX_SCALE: f32 = 2.0;
pub const DEFAULT_VIEW: View = View { scale: 1.0, x: 0.0, y: 0.0 };
static VIEW: Mutex<View> = Mutex::new(DEFAULT_VIEW);

/// Map size and placement (menu sliders). `x`/`y` place the panel in the
/// free screen space: 0 = left/top edge, 1 = right/bottom edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub scale: f32,
    pub x: f32,
    pub y: f32,
}

impl View {
    fn clamped(self) -> View {
        View { scale: self.scale.clamp(MIN_SCALE, MAX_SCALE), x: self.x.clamp(0.0, 1.0), y: self.y.clamp(0.0, 1.0) }
    }
}

pub fn view() -> View {
    *lock(&VIEW)
}

pub fn set_view(v: View) {
    let v = v.clamped();
    *lock(&VIEW) = v;
    settings::set_value(KEY_SCALE, format!("{:.2}", v.scale));
    settings::set_value(KEY_X, format!("{:.3}", v.x));
    settings::set_value(KEY_Y, format!("{:.3}", v.y));
}

/// Shift of a panel drawn at `min` (size `size`) so it sits at `view`'s place
/// on a `screen`, keeping `margin` from the edges. All in the same points.
pub fn placement(min: (f32, f32), size: (f32, f32), screen: (f32, f32), margin: (f32, f32), view: View) -> (f32, f32) {
    let axis = |min: f32, size: f32, screen: f32, margin: f32, t: f32| {
        let free = (screen - size - 2.0 * margin).max(0.0);
        margin + free * t.clamp(0.0, 1.0) - min
    };
    (axis(min.0, size.0, screen.0, margin.0, view.x), axis(min.1, size.1, screen.1, margin.1, view.y))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ground {
    Turf,
    Dirt,
}

/// One runner, from the main thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dot {
    /// Index in the game's horse array (matches `race::Runner::index`).
    pub index: i32,
    pub x: f32,
    pub z: f32,
    pub distance: f32,
}

/// Published by the main thread.
#[derive(Clone, Debug)]
pub struct MapState {
    /// Course centre line, world x/z, from the start (first) to the goal (last).
    pub course: Vec<(f32, f32)>,
    pub length: f32,
    pub ground: Option<Ground>,
    pub dots: Vec<Dot>,
    /// Leader's speed, km/h (None until it can be measured).
    pub speed_kmh: Option<f32>,
}

static STATE: Mutex<Option<(MapState, Instant)>> = Mutex::new(None);
/// Older than this = no race on screen.
const STALE: Duration = Duration::from_millis(500);
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(500);
const SNAPSHOT_MAX_AGE: Duration = Duration::from_secs(3);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn publish(s: Option<MapState>) {
    *lock(&STATE) = s.map(|s| (s, Instant::now()));
}

/// What the renderer draws this frame.
#[derive(Clone, Debug)]
pub struct Frame {
    pub state: MapState,
    /// Gate number per horse index, from the leaderboard snapshot.
    pub gates: Vec<(i32, i32)>,
    pub field: usize,
    /// Leader's metres left.
    pub remaining: Option<f32>,
}

/// Render thread, every frame: None = draw nothing.
pub fn next_frame() -> Option<Frame> {
    if !is_enabled() || !clock::race_live() {
        return None;
    }
    let state = lock(&STATE).as_ref().filter(|(_, at)| at.elapsed() < STALE).map(|(s, _)| s.clone())?;
    if state.course.len() < 2 {
        return None;
    }
    // Gates and bracket colours come from the leaderboard's runner snapshot.
    race::request_snapshot(SNAPSHOT_INTERVAL);
    let runners = race::latest(SNAPSHOT_MAX_AGE).map(|s| s.runners).unwrap_or_default();
    let gates = runners.iter().filter(|r| r.index >= 0).map(|r| (r.index, r.gate)).collect();
    Some(Frame { field: runners.len().max(state.dots.len()), gates, state, remaining: clock::remaining_distance() })
}

// ---------------------------------------------------------------- projection

/// Vertical squash of the course (the "tilt" of the 3D look).
pub const TILT: f32 = 0.42;
/// Extra turn after tilting, radians (the track rises slightly to the right).
pub const SKEW: f32 = -0.16;

/// World x/z -> screen, fitted into a box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    cx: f32,
    cz: f32,
    /// Rotation that lays the course's long axis horizontally (cos, sin).
    rot: (f32, f32),
    /// 180° turn so the goal is at the front (bottom).
    flip: bool,
    scale: f32,
    /// Screen position of the projected origin.
    ox: f32,
    oy: f32,
}

impl Projection {
    /// Before scaling: rotate, flip, tilt, skew. Screen y grows downwards.
    fn shape(&self, x: f32, z: f32) -> (f32, f32) {
        let (dx, dz) = (x - self.cx, z - self.cz);
        let (c, s) = self.rot;
        let mut u = dx * c + dz * s;
        // World z points "up" on a map; screen y points down.
        let mut v = -(-dx * s + dz * c);
        if self.flip {
            u = -u;
            v = -v;
        }
        v *= TILT;
        let (cs, ss) = (SKEW.cos(), SKEW.sin());
        (u * cs - v * ss, u * ss + v * cs)
    }

    pub fn project(&self, x: f32, z: f32) -> (f32, f32) {
        let (u, v) = self.shape(x, z);
        (self.ox + u * self.scale, self.oy + v * self.scale)
    }

    /// Fits `course` (world x/z, start to goal) into a `w` x `h` box at
    /// (`left`, `top`). None for fewer than two distinct points.
    pub fn fit(course: &[(f32, f32)], left: f32, top: f32, w: f32, h: f32) -> Option<Projection> {
        if course.len() < 2 {
            return None;
        }
        let n = course.len() as f32;
        let cx = course.iter().map(|p| p.0).sum::<f32>() / n;
        let cz = course.iter().map(|p| p.1).sum::<f32>() / n;
        // Principal axis: the course's long direction.
        let (mut sxx, mut szz, mut sxz) = (0.0f32, 0.0f32, 0.0f32);
        for &(x, z) in course {
            let (dx, dz) = (x - cx, z - cz);
            sxx += dx * dx;
            szz += dz * dz;
            sxz += dx * dz;
        }
        if sxx + szz <= f32::EPSILON {
            return None;
        }
        let angle = 0.5 * (2.0 * sxz).atan2(sxx - szz);
        let mut p = Projection { cx, cz, rot: (angle.cos(), angle.sin()), flip: false, scale: 1.0, ox: 0.0, oy: 0.0 };
        // The goal (last point) at the front: below the centre on screen.
        let goal = *course.last()?;
        if p.shape(goal.0, goal.1).1 < 0.0 {
            p.flip = true;
        }
        let (mut min_u, mut max_u, mut min_v, mut max_v) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &(x, z) in course {
            let (u, v) = p.shape(x, z);
            min_u = min_u.min(u);
            max_u = max_u.max(u);
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }
        let (sw, sh) = ((max_u - min_u).max(1e-3), (max_v - min_v).max(1e-3));
        p.scale = (w / sw).min(h / sh);
        // Centre the shape in the box.
        p.ox = left + (w - sw * p.scale) / 2.0 - min_u * p.scale;
        p.oy = top + (h - sh * p.scale) / 2.0 - min_v * p.scale;
        Some(p)
    }
}

/// "2,000m"
pub fn fmt_length(m: f32) -> String {
    let m = m.max(0.0).round() as u32;
    if m >= 1000 { format!("{},{:03}m", m / 1000, m % 1000) } else { format!("{m}m") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stadium-shaped loop, `long` x `short`, centred at (cx, cz).
    fn oval(cx: f32, cz: f32, long: f32, short: f32, angle: f32) -> Vec<(f32, f32)> {
        (0..200)
            .map(|i| {
                let t = i as f32 / 200.0 * std::f32::consts::TAU;
                let (u, v) = (long / 2.0 * t.cos(), short / 2.0 * t.sin());
                (cx + u * angle.cos() - v * angle.sin(), cz + u * angle.sin() + v * angle.cos())
            })
            .collect()
    }

    #[test]
    fn fits_inside_the_box() {
        let course = oval(500.0, -300.0, 600.0, 200.0, 0.7);
        let p = Projection::fit(&course, 40.0, 30.0, 400.0, 180.0).unwrap();
        for &(x, z) in &course {
            let (sx, sy) = p.project(x, z);
            assert!((40.0 - 0.01..=440.01).contains(&sx), "x {sx}");
            assert!((30.0 - 0.01..=210.01).contains(&sy), "y {sy}");
        }
    }

    #[test]
    fn long_axis_is_horizontal_and_tilted() {
        // Long axis along world z: still drawn wider than tall.
        let course = oval(0.0, 0.0, 800.0, 250.0, std::f32::consts::FRAC_PI_2);
        let p = Projection::fit(&course, 0.0, 0.0, 1000.0, 1000.0).unwrap();
        let xs: Vec<f32> = course.iter().map(|&(x, z)| p.project(x, z).0).collect();
        let ys: Vec<f32> = course.iter().map(|&(x, z)| p.project(x, z).1).collect();
        let w = xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min);
        let h = ys.iter().cloned().fold(f32::MIN, f32::max) - ys.iter().cloned().fold(f32::MAX, f32::min);
        assert!(w > 2.0 * h, "w {w} h {h}");
    }

    #[test]
    fn goal_is_at_the_front() {
        // Goal in the middle of either long side (points 50 and 150).
        for (angle, goal_point) in [(0.0f32, 50), (0.0, 150), (1.0, 50), (2.5, 150), (4.0, 50)] {
            let mut course = oval(0.0, 0.0, 600.0, 200.0, angle);
            course.rotate_left(goal_point + 1);
            let p = Projection::fit(&course, 0.0, 0.0, 400.0, 200.0).unwrap();
            let goal = *course.last().unwrap();
            let centre_y = course.iter().map(|&(x, z)| p.project(x, z).1).sum::<f32>() / course.len() as f32;
            assert!(p.project(goal.0, goal.1).1 >= centre_y - 1.0, "angle {angle}");
        }
    }

    #[test]
    fn degenerate_courses() {
        assert!(Projection::fit(&[], 0.0, 0.0, 10.0, 10.0).is_none());
        assert!(Projection::fit(&[(1.0, 1.0)], 0.0, 0.0, 10.0, 10.0).is_none());
        assert!(Projection::fit(&[(1.0, 1.0), (1.0, 1.0)], 0.0, 0.0, 10.0, 10.0).is_none());
    }

    #[test]
    fn placement_keeps_the_panel_on_screen() {
        let v = |x, y| View { scale: 1.0, x, y };
        // Default: the panel stays where it is drawn (its corner = the margin).
        assert_eq!(placement((16.0, 14.0), (388.0, 250.0), (1920.0, 1080.0), (16.0, 14.0), v(0.0, 0.0)), (0.0, 0.0));
        // Far corner: right/bottom edge minus the margin.
        let (dx, dy) = placement((16.0, 14.0), (388.0, 250.0), (1920.0, 1080.0), (16.0, 14.0), v(1.0, 1.0));
        assert_eq!((16.0 + dx + 388.0, 14.0 + dy + 250.0), (1920.0 - 16.0, 1080.0 - 14.0));
        // Bigger than the screen: pinned to the margin.
        assert_eq!(placement((16.0, 14.0), (2000.0, 250.0), (1920.0, 1080.0), (16.0, 14.0), v(0.7, 0.0)).0, 0.0);
    }

    #[test]
    fn formats_lengths() {
        assert_eq!(fmt_length(2000.0), "2,000m");
        assert_eq!(fmt_length(1200.4), "1,200m");
        assert_eq!(fmt_length(612.0), "612m");
        assert_eq!(fmt_length(-5.0), "0m");
    }
}
