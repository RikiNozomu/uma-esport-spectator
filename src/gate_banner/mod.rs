//! Gate banner: our own picture on the starting gate's header, like the
//! broadcast banners on real gates. Option "polygon": we do not touch the
//! game's textures; we add two quads (one-sided meshes) to the gate
//! object, painted with a PNG, over the front and back of the gate's header
//! plates, and turn the game's header plates off.
//!
//! The gate (`Course/pfb_env_race00000_gate000_000`) is the same prefab on
//! every course; its header is the course's name plate under `pos_name_plate`
//! (e.g. `pfb_env_race10006_1000_000/gate_name_etc`, Tokyo) plus the URA logo
//! strip `gate_alpha_URA`.
//!
//! - `game` (main thread, per race frame from clock's AlterUpdate hook): finds
//!   the gate, builds the quads, applies the menu's placement; writes
//!   `gate_dump.txt` (every renderer in the scene) when the gates open.
//! - here: settings, which picture a race uses, placement math (pure, tested).
//!
//! A race gets a banner when a picture exists for it, in `gate_banner/`
//! under the plugin folder. Gates come in sizes (stalls = the highest
//! `gate_number/number_NN` of the gate model) and the header's length
//! follows. JRA courses (master `race.start_gate` 0) have two models: 18
//! stalls, header 11.98 x 0.25 (`gate000_000`/`gate001_000`, the latter with
//! the name plate at the other end), and 9 stalls for races of 9 runners or
//! fewer, header 5.98 x 0.25 (`gate010_000`/`gate011_000`). Oi, Kawasaki,
//! Funabashi, Morioka (100/103/104/105) and Longchamp (201) have their own gates.
//! So a picture can be made for one size: `<race instance id>_<n>.png`,
//! `<race id>_<n>.png`, `course_<course id>_<n>.png`, `all_<n>.png`, then the
//! same without `_<n>` (course id from the gate's name plate prefab). The
//! menu shows the ids, the stall count and the header's ratio. End-user
//! guide and id lists: docs/gate_banner/.

pub mod game;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{settings, tracer};

const KEY: &str = "gate_banner";
const DUMP_KEY: &str = "gate_banner_dump";
static ENABLED: AtomicBool = AtomicBool::new(true);
static DUMP: AtomicBool = AtomicBool::new(true);

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    settings::set_value(KEY, on);
}

pub fn dump_enabled() -> bool {
    DUMP.load(Ordering::Relaxed)
}

pub fn set_dump_enabled(on: bool) {
    DUMP.store(on, Ordering::Relaxed);
    settings::set_value(DUMP_KEY, on);
}

pub fn load_settings() {
    ENABLED.store(settings::get_bool(KEY, true), Ordering::Relaxed);
    DUMP.store(settings::get_bool(DUMP_KEY, true), Ordering::Relaxed);
}

pub fn folder() -> PathBuf {
    tracer::plugin_dir().join("gate_banner")
}

/// Which race is on screen: what picture names can refer to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RaceKey {
    pub instance: i32,
    pub race: i32,
    /// Racecourse (master `race_course_set.race_track_id`, e.g. 10006 =
    /// Tokyo), when known: read from the gate's name plate.
    pub course: Option<i32>,
}

/// File names tried for a race on a gate of `stalls` stalls, most specific
/// first: for that gate size the race instance's, the race's, the course's,
/// anything's; then the same whatever the size.
pub fn candidates(key: RaceKey, stalls: u32) -> Vec<String> {
    let mut stems = vec![key.instance.to_string(), key.race.to_string()];
    if let Some(c) = key.course {
        stems.push(format!("course_{c}"));
    }
    stems.push("all".to_owned());
    let sized = stems.iter().map(|s| format!("{s}_{stalls}.png"));
    let any = stems.iter().map(|s| format!("{s}.png"));
    sized.chain(any).collect()
}

/// The picture (a name from `files`, the folder's lowercase file names).
pub fn pick_picture(files: &[String], key: RaceKey, stalls: u32) -> Option<String> {
    candidates(key, stalls).into_iter().find(|c| files.contains(c))
}

/// Whether the race may have a picture, whatever the gate size and before the
/// course is known: worth looking for the gate.
pub fn has_any_picture(files: &[String], race_instance_id: i32, race_id: i32) -> bool {
    let named = |stem: &str, p: &str| {
        stem == p || stem.strip_prefix(p).and_then(|s| s.strip_prefix('_')).is_some_and(is_number)
    };
    files.iter().filter_map(|f| f.strip_suffix(".png")).any(|stem| {
        named(stem, &race_instance_id.to_string())
            || named(stem, &race_id.to_string())
            || named(stem, "all")
            || stem.strip_prefix("course_").is_some_and(|rest| {
                let (c, n) = rest.split_once('_').unwrap_or((rest, "0"));
                is_number(c) && is_number(n)
            })
    })
}

/// The racecourse id from the gate's parts: its name plate is the course's
/// prefab, `pfb_env_race<course>_<variant>` (the gate itself is `race00000`).
pub fn course_from_paths(paths: &[&[String]]) -> Option<i32> {
    paths.iter().flat_map(|p| p.iter()).find_map(|n| {
        let id: i32 = n.strip_prefix("pfb_env_race")?.get(..5)?.parse().ok()?;
        (id >= 10_000).then_some(id)
    })
}

fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Lowercase names of the files in `dir`.
pub fn picture_files(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_lowercase()).collect()
}

/// Stalls of a gate model, from the paths (below the gate) of its parts:
/// the highest `gate_number/number_NN`, else the highest `startgate_door_NN`.
pub fn stall_count(paths: &[&[String]]) -> Option<u32> {
    let highest = |prefix: &str| {
        paths
            .iter()
            .filter_map(|p| p.iter().find_map(|n| n.to_lowercase().strip_prefix(prefix).and_then(|d| d.parse::<u32>().ok())))
            .max()
    };
    highest("number_").or_else(|| highest("startgate_door_")).filter(|&n| n > 0)
}

/// "19.2:1, e.g. 4800x250 px" for a banner `length` x `height`.
pub fn ratio_hint(length: f32, height: f32) -> String {
    if !(length > 0.0 && height > 0.0) {
        return "?".to_owned();
    }
    let ratio = length / height;
    let h = 250u32;
    format!("{ratio:.1}:1, e.g. {}x{h} px", (ratio * h as f32).round() as u32)
}

// ---------------------------------------------------------------- look

/// Whose material (shader) the banner copies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// The course's big screen (`monitor_A`): shows any picture as is, with the scene's fog.
    Monitor,
    /// The sky dome: unlit, no fog.
    Sky,
    /// The gate's own name plate: its shader is made for white-on-green
    /// plates and washes colour pictures out.
    Plate,
}

impl Look {
    pub const ALL: [Look; 3] = [Look::Monitor, Look::Sky, Look::Plate];

    pub fn key(self) -> &'static str {
        match self {
            Look::Monitor => "monitor",
            Look::Sky => "sky",
            Look::Plate => "plate",
        }
    }

    /// Short, for the menu's dropdown.
    pub fn name(self) -> &'static str {
        match self {
            Look::Monitor => "Big screen",
            Look::Sky => "Unlit",
            Look::Plate => "Name plate",
        }
    }

    /// One line under the dropdown.
    pub fn description(self) -> &'static str {
        match self {
            Look::Monitor => "Picture's own colours, with the scene's distance haze (default).",
            Look::Sky => "Picture's own colours, no haze.",
            Look::Plate => "Lit like the game's name plate: colour pictures look pale.",
        }
    }

    /// The shader to look for in the scene (None = the header plate's).
    pub fn shader(self) -> Option<&'static str> {
        match self {
            Look::Monitor => Some("Gallop/3D/Bg/MonitorFog"),
            Look::Sky => Some("Gallop/3D/Bg/UnlitNoFog"),
            Look::Plate => None,
        }
    }

    pub fn from_key(key: &str) -> Option<Look> {
        Look::ALL.into_iter().find(|l| l.key() == key)
    }
}

const LOOK_KEY: &str = "gate_banner_look";

pub fn look() -> Look {
    settings::get_string(LOOK_KEY).and_then(|k| Look::from_key(&k)).unwrap_or(Look::Monitor)
}

pub fn set_look(l: Look) {
    settings::set_value(LOOK_KEY, l.key());
}

// ---------------------------------------------------------------- placement

/// Menu adjustments, in world units (the race world is small: the gate is
/// ~12 units long, its header ~0.3 high).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    /// Along the gate (its long axis).
    pub along: f32,
    pub up: f32,
    /// Times the header's length.
    pub width: f32,
    /// Times the header's height; the banner grows upwards from the
    /// header's bottom edge.
    pub height: f32,
    /// Times the vertex colour copied from the game's plate (its baked light).
    pub brightness: f32,
    /// Turn the game's own header plates off.
    pub hide_header: bool,
}

impl Default for Tuning {
    fn default() -> Tuning {
        Tuning { along: 0.0, up: 0.0, width: 1.0, height: HEIGHT_SCALE, brightness: 1.0, hide_header: true }
    }
}

/// Default banner height, times the header's: much taller than the game's
/// plate, like real sponsor banners (the templates are drawn at this shape).
pub const HEIGHT_SCALE: f32 = 2.5;

const TUNING_KEYS: [&str; 6] = [
    "gate_banner_x",
    "gate_banner_y",
    "gate_banner_w",
    "gate_banner_h_scale",
    "gate_banner_brightness",
    "gate_banner_hide_header",
];

pub fn tuning() -> Tuning {
    let d = Tuning::default();
    Tuning {
        along: settings::get_f32(TUNING_KEYS[0], d.along),
        up: settings::get_f32(TUNING_KEYS[1], d.up),
        width: settings::get_f32(TUNING_KEYS[2], d.width),
        height: settings::get_f32(TUNING_KEYS[3], d.height),
        brightness: settings::get_f32(TUNING_KEYS[4], d.brightness),
        hide_header: settings::get_bool(TUNING_KEYS[5], d.hide_header),
    }
}

pub fn set_tuning(t: Tuning) {
    let old = tuning();
    let pairs = [
        (old.along, t.along),
        (old.up, t.up),
        (old.width, t.width),
        (old.height, t.height),
        (old.brightness, t.brightness),
    ];
    for (key, (a, b)) in TUNING_KEYS.iter().zip(pairs) {
        if a != b {
            settings::set_value(key, format!("{b:.3}"));
        }
    }
    if old.hide_header != t.hide_header {
        settings::set_value(TUNING_KEYS[5], t.hide_header);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl V3 {
    pub const fn new(x: f32, y: f32, z: f32) -> V3 {
        V3 { x, y, z }
    }
}

/// One banner quad, in the gate's axes (units from the gate's origin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub center: V3,
    /// Rotation about the gate's up axis, degrees. The quad's front faces -Z
    /// (Unity's quad convention); its +X is the picture's left-to-right.
    pub yaw: f32,
}

/// The banner: one quad on each face of the header.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub faces: [Face; 2],
    pub width: f32,
    pub height: f32,
    /// The quads' width runs along the gate's X axis (else along Z).
    pub along_x: bool,
}

/// Gap between the header's faces and the banner (units): clear of the
/// plates (no z-fighting) yet tight on the gate. Fixed: 0.01 standoff + 0.01
/// out, what looked right in game.
const STANDOFF: f32 = 0.02;

/// Covers the header's box (`min`..`max`, units in the gate's axes) on both
/// faces. The banner is `header_height` x the tuning's height factor high,
/// its bottom edge on the header's (it grows upwards).
pub fn place(min: V3, max: V3, header_height: f32, t: &Tuning) -> Option<Placement> {
    let size = V3::new(max.x - min.x, max.y - min.y, max.z - min.z);
    if !(size.x.is_finite() && size.y.is_finite() && size.z.is_finite()) || size.x.max(size.z) <= 0.0 {
        return None;
    }
    let along_x = size.x >= size.z;
    let length = if along_x { size.x } else { size.z };
    let width = (length * t.width).max(0.01);
    let height = banner_height(header_height, t);
    let y = min.y + height / 2.0 + t.up;
    let mid = if along_x { (min.x + max.x) / 2.0 } else { (min.z + max.z) / 2.0 } + t.along;
    let (lo, hi) = if along_x { (min.z, max.z) } else { (min.x, max.x) };
    let (front, back) = (lo - STANDOFF, hi + STANDOFF);
    let faces = if along_x {
        // Front faces -Z: no turn; the back one faces +Z.
        [Face { center: V3::new(mid, y, front), yaw: 0.0 }, Face { center: V3::new(mid, y, back), yaw: 180.0 }]
    } else {
        // Yaw +90 turns the quad's -Z normal to -X.
        [Face { center: V3::new(front, y, mid), yaw: 90.0 }, Face { center: V3::new(back, y, mid), yaw: -90.0 }]
    };
    Some(Placement { faces, width, height, along_x })
}

/// The 8 corners of a box given by centre and half-size.
pub fn box_corners(center: V3, extents: V3) -> [V3; 8] {
    std::array::from_fn(|i| {
        let s = |bit: usize| if i & bit == 0 { -1.0 } else { 1.0 };
        V3::new(center.x + s(1) * extents.x, center.y + s(2) * extents.y, center.z + s(4) * extents.z)
    })
}

/// The banner's height for a header `header_height` high.
pub fn banner_height(header_height: f32, t: &Tuning) -> f32 {
    (header_height * t.height).max(0.01)
}

/// Quaternion (x, y, z, w) of a turn about Y.
pub fn yaw_quat(deg: f32) -> [f32; 4] {
    let half = deg.to_radians() / 2.0;
    [0.0, half.sin(), 0.0, half.cos()]
}

/// The banner mesh: a one-sided unit quad (-0.5..0.5 in X and Y) facing -Z,
/// like Unity's built-in Quad. One-sided: from inside the gate (seen from
/// above, through the open frame) the far banner must not show.
pub struct QuadMesh {
    pub vertices: [V3; 4],
    pub normals: [V3; 4],
    pub uv: [[f32; 2]; 4],
    pub triangles: [i32; 6],
}

pub fn quad_mesh() -> QuadMesh {
    let corners = [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)];
    let vertices = corners.map(|(x, y)| V3::new(x, y, 0.0));
    let uv = corners.map(|(x, y)| [x + 0.5, y + 0.5]);
    // Unity: front faces wind clockwise as seen by the camera.
    QuadMesh { vertices, normals: [V3::new(0.0, 0.0, -1.0); 4], uv, triangles: [0, 3, 1, 3, 0, 2] }
}

/// Average of a mesh's vertex colours (the game's plates carry their baked
/// light there); None for no colours.
pub fn average_color(colors: &[[f32; 4]]) -> Option<[f32; 4]> {
    let ok: Vec<&[f32; 4]> = colors.iter().filter(|c| c.iter().all(|v| v.is_finite())).collect();
    if ok.is_empty() {
        return None;
    }
    let mut sum = [0f32; 4];
    for c in &ok {
        for i in 0..4 {
            sum[i] += c[i];
        }
    }
    Some(sum.map(|s| s / ok.len() as f32))
}

/// Bounding rect (min, max) of UV coordinates.
pub fn uv_rect(uv: &[[f32; 2]]) -> Option<([f32; 2], [f32; 2])> {
    let ok = uv.iter().filter(|p| p[0].is_finite() && p[1].is_finite());
    let mut r: Option<([f32; 2], [f32; 2])> = None;
    for p in ok {
        r = Some(match r {
            None => (*p, *p),
            Some((a, b)) => ([a[0].min(p[0]), a[1].min(p[1])], [b[0].max(p[0]), b[1].max(p[1])]),
        });
    }
    r
}

/// The vertex colour for the banner: the plate's times `brightness`, alpha kept.
pub fn tint(base: [f32; 4], brightness: f32) -> [f32; 4] {
    let b = brightness.max(0.0);
    [base[0] * b, base[1] * b, base[2] * b, base[3]]
}

/// Picks the gate among object paths ("Root/Course/Gate/Header"): the
/// top-most path element containing `wanted` (lowercase; default "gate",
/// never "goal"). Returns (path index, element index) of the candidate that
/// holds the most renderers.
pub fn pick_anchor(paths: &[Vec<String>], wanted: Option<&str>) -> Option<(usize, usize)> {
    let matches = |name: &str| {
        let n = name.to_lowercase();
        match wanted {
            Some(w) if !w.is_empty() => n.contains(w),
            _ => n.contains("gate") && !n.contains("goal"),
        }
    };
    let mut counts: Vec<(Vec<String>, usize, (usize, usize))> = Vec::new();
    for (pi, path) in paths.iter().enumerate() {
        let Some(ei) = path.iter().position(|n| matches(n)) else { continue };
        let key = &path[..=ei];
        match counts.iter_mut().find(|(k, _, _)| k.as_slice() == key) {
            Some(c) => c.1 += 1,
            None => counts.push((key.to_vec(), 1, (pi, ei))),
        }
    }
    counts.into_iter().max_by_key(|c| c.1).map(|c| c.2)
}

/// Whether a gate part (path below the gate) belongs to the header: anything
/// under one of `groups` (e.g. "pos_name_plate"), or named one of `parts`
/// (e.g. "gate_alpha_URA"). Case-insensitive.
pub fn is_header(path: &[String], groups: &[&str], parts: &[&str]) -> bool {
    let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    path.iter().any(|n| groups.iter().any(|g| eq(n, g)))
        || path.last().is_some_and(|n| parts.iter().any(|p| eq(n, p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Tokyo header seen in game: 12 long along Z, 0.3 high, plates 1.4 apart.
    fn header() -> (V3, V3) {
        (V3::new(-0.7, 2.15, -6.0), V3::new(0.7, 2.45, 6.0))
    }

    fn close(a: V3, b: V3) -> bool {
        (a.x - b.x).abs() < 1e-4 && (a.y - b.y).abs() < 1e-4 && (a.z - b.z).abs() < 1e-4
    }

    #[test]
    fn covers_the_header_on_both_faces() {
        let (min, max) = header();
        let p = place(min, max, 0.3, &Tuning::default()).unwrap();
        assert!(!p.along_x);
        assert_eq!(p.width, 12.0);
        // 2.5x the header, bottom edge on the header's (2.15): centre 2.15 + 0.375.
        assert!((p.height - 0.75).abs() < 1e-5);
        assert!(close(p.faces[0].center, V3::new(-0.72, 2.525, 0.0)));
        assert!(close(p.faces[1].center, V3::new(0.72, 2.525, 0.0)));
        assert_eq!((p.faces[0].yaw, p.faces[1].yaw), (90.0, -90.0));
    }

    #[test]
    fn corners_of_a_box() {
        let c = box_corners(V3::new(1.0, 2.0, 3.0), V3::new(0.5, 0.25, 6.0));
        assert_eq!(c[0], V3::new(0.5, 1.75, -3.0));
        assert_eq!(c[7], V3::new(1.5, 2.25, 9.0));
        // All 8 distinct.
        for i in 0..8 {
            for j in i + 1..8 {
                assert_ne!(c[i], c[j]);
            }
        }
    }

    #[test]
    fn taller_banners_keep_the_bottom_edge() {
        let (min, max) = header();
        for scale in [1.0, 1.5, 2.0, 3.0] {
            let t = Tuning { height: scale, ..Tuning::default() };
            let p = place(min, max, 0.3, &t).unwrap();
            let bottom = p.faces[0].center.y - p.height / 2.0;
            assert!((bottom - min.y).abs() < 1e-5, "scale {scale}: bottom {bottom}");
        }
    }

    #[test]
    fn adjustments() {
        let (min, max) = header();
        let t = Tuning { along: 1.0, up: -0.1, width: 0.5, height: 2.0, ..Tuning::default() };
        let p = place(min, max, 0.3, &t).unwrap();
        assert_eq!(p.width, 6.0);
        assert!((p.height - 0.6).abs() < 1e-5);
        assert!(close(p.faces[0].center, V3::new(-0.7 - STANDOFF, 2.35, 1.0)));
        assert!(close(p.faces[1].center, V3::new(0.7 + STANDOFF, 2.35, 1.0)));
    }

    #[test]
    fn gate_along_x() {
        let t = Tuning { height: 1.0, ..Tuning::default() };
        let p = place(V3::new(-6.0, 0.0, -0.5), V3::new(6.0, 0.3, 0.5), 0.3, &t).unwrap();
        assert!(p.along_x);
        assert_eq!((p.faces[0].yaw, p.faces[1].yaw), (0.0, 180.0));
        assert!(close(p.faces[0].center, V3::new(0.0, 0.15, -0.5 - STANDOFF)));
    }

    #[test]
    fn front_normal_points_away_from_the_gate() {
        // The quad's -Z normal turned by +90 degrees points to -X.
        let [_, y, _, w] = yaw_quat(90.0);
        let angle = 2.0 * y.atan2(w);
        assert!((-angle.sin() + 1.0).abs() < 1e-5);
        assert!(place(V3::default(), V3::default(), 0.3, &Tuning::default()).is_none());
    }

    #[test]
    fn one_sided_quad() {
        let m = quad_mesh();
        assert_eq!(m.vertices[3], V3::new(0.5, 0.5, 0.0));
        assert_eq!(m.uv[3], [1.0, 1.0]);
        assert!(m.triangles.iter().all(|&i| (0..4).contains(&i)));
        // Clockwise as seen from -Z (x right, y up): bottom-left, top-right, bottom-right.
        let [a, b, c] = [0, 1, 2].map(|i| m.vertices[m.triangles[i] as usize]);
        let cross_z = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        assert!(cross_z < 0.0);
    }

    #[test]
    fn plate_colour_and_lightmap_area() {
        assert_eq!(average_color(&[]), None);
        let avg = average_color(&[[0.2, 0.4, 0.6, 1.0], [0.4, 0.6, 0.8, 1.0]]).unwrap();
        let want = [0.3, 0.5, 0.7, 1.0];
        assert!(avg.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6), "{avg:?}");
        assert_eq!(uv_rect(&[[0.5, 0.2], [0.1, 0.9], [0.3, 0.3]]), Some(([0.1, 0.2], [0.5, 0.9])));
        assert_eq!(uv_rect(&[]), None);
        assert_eq!(tint([0.5, 0.5, 0.5, 0.8], 1.5), [0.75, 0.75, 0.75, 0.8]);
    }

    #[test]
    fn picks_the_gate_with_most_renderers() {
        let p = |s: &str| s.split('/').map(str::to_owned).collect::<Vec<_>>();
        let paths = vec![
            p("Course/GoalGate/Flower"),
            p("Course/StartGate/Frame"),
            p("Course/StartGate/Header"),
            p("Course/StartGate/GateDoor_01"),
            p("Crowd/Fan"),
        ];
        assert_eq!(pick_anchor(&paths, None), Some((1, 1)));
        assert_eq!(pick_anchor(&paths, Some("crowd")), Some((4, 0)));
        assert_eq!(pick_anchor(&paths[4..], None), None);
    }

    #[test]
    fn header_parts() {
        let p = |s: &str| s.split('/').map(str::to_owned).collect::<Vec<_>>();
        let groups = ["pos_name_plate"];
        let parts = ["gate_alpha_URA"];
        assert!(is_header(&p("pos_name_plate/pfb_env_race10006_1000_000(Clone)/gate_name_etc"), &groups, &parts));
        assert!(is_header(&p("gate_alpha_URA"), &groups, &parts));
        assert!(!is_header(&p("gate_alpha"), &groups, &parts));
        assert!(!is_header(&p("gate_number/number_01"), &groups, &parts));
    }

    #[test]
    fn looks_round_trip() {
        for l in Look::ALL {
            assert_eq!(Look::from_key(l.key()), Some(l));
        }
        assert_eq!(Look::from_key("nope"), None);
    }

    #[test]
    fn pictures_by_race_course_and_gate_size() {
        let files = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let key = RaceKey { instance: 800002, race: 10161, course: Some(10007) };
        let f = files(&["all.png", "all_9.png", "10161_18.png", "course_10007.png", "notes.txt"]);
        assert_eq!(pick_picture(&f, key, 18).as_deref(), Some("10161_18.png"));
        assert_eq!(pick_picture(&f, key, 9).as_deref(), Some("all_9.png"), "sized beats unsized");
        assert_eq!(pick_picture(&f, key, 12).as_deref(), Some("course_10007.png"));
        let other = RaceKey { instance: 1, race: 2, course: Some(10006) };
        assert_eq!(pick_picture(&f, other, 18).as_deref(), Some("all.png"));
        assert_eq!(pick_picture(&f, RaceKey { course: None, ..other }, 9).as_deref(), Some("all_9.png"));
        // Only sized pictures: some gate sizes have none.
        let sized = files(&["all_9.png"]);
        assert!(has_any_picture(&sized, 1, 2));
        assert_eq!(pick_picture(&sized, other, 18), None);
        assert!(has_any_picture(&files(&["10161.png"]), 800002, 10161));
        assert!(has_any_picture(&files(&["course_10006_18.png"]), 1, 2));
        assert!(has_any_picture(&files(&["course_10006.png"]), 1, 2));
        assert!(!has_any_picture(&files(&["all_x.png", "101610.png", "course_x.png", "notes.txt"]), 800002, 10161));
        assert!(!has_any_picture(&[], 1, 2));
    }

    #[test]
    fn course_from_the_name_plate() {
        let p = |s: &str| s.split('/').map(str::to_owned).collect::<Vec<_>>();
        let paths = [p("gate_base"), p("pos_name_plate/pfb_env_race10007_1000_000(Clone)/gate_name_etc")];
        let refs: Vec<&[String]> = paths.iter().map(|v| v.as_slice()).collect();
        assert_eq!(course_from_paths(&refs), Some(10007));
        assert_eq!(course_from_paths(&refs[..1]), None);
        let gate_only = [p("pfb_env_race00000_gate000_000(Clone)")];
        let refs: Vec<&[String]> = gate_only.iter().map(|v| v.as_slice()).collect();
        assert_eq!(course_from_paths(&refs), None, "the gate prefab is not a course");
    }

    #[test]
    fn counts_stalls() {
        let p = |s: &str| s.split('/').map(str::to_owned).collect::<Vec<_>>();
        let paths = [p("gate_number/number_09"), p("gate_number/number_12"), p("startgate_door_18/door_r/frame_r_00")];
        let refs: Vec<&[String]> = paths.iter().map(|v| v.as_slice()).collect();
        assert_eq!(stall_count(&refs), Some(12));
        assert_eq!(stall_count(&refs[2..]), Some(18), "doors when there are no numbers");
        assert_eq!(stall_count(&[]), None);
        assert_eq!(ratio_hint(12.0, 0.625), "19.2:1, e.g. 4800x250 px");
    }
}
