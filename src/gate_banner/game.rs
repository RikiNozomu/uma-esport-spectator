//! Main-thread side of the gate banner: find the starting gate and its
//! header, add our quads over the header, keep them placed as the menu says.
//!
//! Called from clock's `RaceManagerReplayBase::AlterUpdate` hook (every race
//! frame). The gate is found by name among the scene's renderers
//! (`FindObjectsOfType(Renderer)`): the top-most object whose name has "gate"
//! (not "goal"), or `gate_banner_anchor` from settings.txt. The quads are
//! children of the gate: they go away with the race scene.
//!
//! The header = gate parts under `pos_name_plate` (the course's name plate)
//! plus `gate_alpha_URA` (and `gate_banner_hide` names from settings.txt).
//! The banner covers the header's box; its material is a copy of the name
//! plate's (same shader and lighting as the plate it replaces) with our
//! picture as main texture. Disabled / inactive renderers are ignored: their
//! bounds are stale (the lamp effects' are ~3x the gate's length).

use std::ffi::c_void;
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use super::{average_color, is_header, place, quad_mesh, tint, uv_rect, yaw_quat, Look, Tuning, V3};
use crate::il2cpp::{self, Method, Obj, CORE, GAME};
use crate::{api, clock, race_ui, settings, tracer};

const IMAGE_CONVERSION: &str = "UnityEngine.ImageConversionModule.dll";
const MSCORLIB: &str = "mscorlib.dll";
const OBJECT_NAME: &str = "UmaSpectatorGateBanner";
/// Gate children whose whole subtree is the header.
const HEADER_GROUPS: [&str; 1] = ["pos_name_plate"];
/// Gate parts that are also part of the header (the URA logo strip).
const HEADER_PARTS: [&str; 1] = ["gate_alpha_URA"];
/// Frames between two looks for the gate / the picture file.
const RETRY_FRAMES: u32 = 30;
/// Looks for the gate per race before giving up (~20 s at 60 fps).
const MAX_ATTEMPTS: u32 = 40;
/// Vertex colour when the plate's mesh cannot be read: the background
/// shaders double the colour, so mid grey is neutral.
const FALLBACK_TINT: [f32; 4] = [0.5, 0.5, 0.5, 1.0];
/// UnityEngine.Rendering.CullMode.Back, for shaders exposing a cull property.
const CULL_BACK: f32 = 2.0;
const MAX_RENDERERS: usize = 20_000;
const MAX_DUMPED: usize = 6_000;
const WRAP_CLAMP: i32 = 1;
const ANISO: i32 = 8;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Bounds {
    center: V3,
    extents: V3,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Quat([f32; 4]);

#[repr(C)]
#[derive(Clone, Copy)]
struct V2 {
    x: f32,
    y: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct V4([f32; 4]);

/// Lightmap slot of a renderer (index, scale/offset).
struct Lightmap {
    get_index: Method,
    set_index: Method,
    get_scale_offset: Method,
    set_scale_offset: Method,
}

/// Shader property listing, for the dump.
struct ShaderProps {
    count: Method,
    name: Method,
}

struct Unity {
    find_objects: usize,
    renderer_type: usize,
    get_name: Method,
    exists: Method,
    destroy: Method,
    component_game_object: usize,
    component_transform: usize,
    get_parent: Method,
    set_parent: Method,
    inverse_transform_point: Method,
    transform_point: Method,
    mesh_bounds: Method,
    get_lossy_scale: Method,
    set_local_position: Method,
    set_local_rotation: Method,
    set_local_scale: Method,
    go_class: usize,
    go_ctor: Method,
    go_transform: Method,
    go_get_layer: Method,
    go_set_layer: Method,
    go_active_in_hierarchy: Method,
    add_component: Method,
    get_component: Method,
    get_bounds: Method,
    get_shared_material: Method,
    set_shared_material: Method,
    renderer_get_enabled: Method,
    renderer_set_enabled: Method,
    lightmap: Result<Lightmap, String>,
    material_class: usize,
    material_ctor: Method,
    get_shader: Method,
    get_main_texture: Method,
    set_main_texture: Method,
    set_texture_scale: Method,
    set_texture_offset: Method,
    set_float: Option<Method>,
    shader_props: Result<ShaderProps, String>,
    mesh_class: usize,
    mesh_ctor: Method,
    set_vertices: Method,
    set_normals: Method,
    set_uv: Method,
    set_uv2: Method,
    set_colors: Method,
    set_triangles: Method,
    mesh_readable: Method,
    get_colors: Method,
    get_uv2: Method,
    mesh_filter_type: usize,
    mesh_renderer_type: usize,
    set_shared_mesh: Method,
    get_shared_mesh: Method,
    texture2d_class: usize,
    texture2d_ctor: Method,
    load_image: Method,
    texture_width: usize,
    texture_height: usize,
    texture_wrap: Option<Method>,
    texture_aniso: Option<usize>,
    byte_class: usize,
    int_class: usize,
    vector3_class: usize,
    vector2_class: usize,
    color_class: usize,
    race_info: Method,
    race_id: Method,
    race_instance_id: Method,
}

unsafe impl Send for Unity {}
unsafe impl Sync for Unity {}

static UNITY: OnceLock<Result<Unity, String>> = OnceLock::new();

unsafe fn call0<R>(m: Method, this: Obj) -> R {
    let f: extern "C" fn(Obj, usize) -> R = std::mem::transmute(m.code);
    f(this, m.info)
}

unsafe fn call1<A, R>(m: Method, this: Obj, a: A) -> R {
    let f: extern "C" fn(Obj, A, usize) -> R = std::mem::transmute(m.code);
    f(this, a, m.info)
}

unsafe fn call2<A, B, R>(m: Method, this: Obj, a: A, b: B) -> R {
    let f: extern "C" fn(Obj, A, B, usize) -> R = std::mem::transmute(m.code);
    f(this, a, b, m.info)
}

unsafe fn icall1<R>(code: usize, this: Obj) -> R {
    let f: extern "C" fn(Obj) -> R = std::mem::transmute(code);
    f(this)
}

/// Method `name` with `argc` params, checked against the expected signature.
unsafe fn checked(k: *mut c_void, name: &str, argc: u32, is_static: bool, ret: &[&str]) -> Result<Method, String> {
    let m = il2cpp::method(k, name, argc).map_err(|e| format!("{}: {e}", il2cpp::class_full_name(k)))?;
    il2cpp::expect(m, is_static, ret, argc).map_err(|e| format!("{}::{name}: {e}", il2cpp::class_full_name(k)))?;
    Ok(m)
}

const VOID: &[&str] = &["System.Void"];

impl Unity {
    unsafe fn resolve() -> Result<Unity, String> {
        use il2cpp::{class, icall, method_params, type_object};
        let m = |k, name, argc, is_static, ret: &[&str]| checked(k, name, argc, is_static, ret);
        let object = class(CORE, "UnityEngine", "Object")?;
        let go = class(CORE, "UnityEngine", "GameObject")?;
        let tr = class(CORE, "UnityEngine", "Transform")?;
        let renderer = class(CORE, "UnityEngine", "Renderer")?;
        let material = class(CORE, "UnityEngine", "Material")?;
        let shader = class(CORE, "UnityEngine", "Shader")?;
        let texture = class(CORE, "UnityEngine", "Texture")?;
        let mesh = class(CORE, "UnityEngine", "Mesh")?;
        let mesh_filter = class(CORE, "UnityEngine", "MeshFilter")?;
        let mesh_renderer = class(CORE, "UnityEngine", "MeshRenderer")?;
        let texture2d = class(CORE, "UnityEngine", "Texture2D")?;
        let conversion = class(IMAGE_CONVERSION, "UnityEngine", "ImageConversion")?;
        let rm = class(GAME, "Gallop", "RaceManager")?;
        let race_info = class(GAME, "Gallop", "RaceInfo")?;

        let lightmap = (|| {
            Ok(Lightmap {
                get_index: m(renderer, "get_lightmapIndex", 0, false, &["System.Int32"])?,
                set_index: m(renderer, "set_lightmapIndex", 1, false, VOID)?,
                get_scale_offset: m(renderer, "get_lightmapScaleOffset", 0, false, &["UnityEngine.Vector4"])?,
                set_scale_offset: m(renderer, "set_lightmapScaleOffset", 1, false, VOID)?,
            })
        })();
        let shader_props = (|| {
            Ok(ShaderProps {
                count: m(shader, "GetPropertyCount", 0, false, &["System.Int32"])?,
                name: m(shader, "GetPropertyName", 1, false, &["System.String"])?,
            })
        })();

        Ok(Unity {
            find_objects: icall("UnityEngine.Object::FindObjectsOfType(System.Type,System.Boolean)")?,
            renderer_type: type_object(renderer)? as usize,
            get_name: m(object, "get_name", 0, false, &["System.String"])?,
            exists: m(object, "op_Implicit", 1, true, &["System.Boolean"])?,
            destroy: m(object, "Destroy", 1, true, VOID)?,
            component_game_object: icall("UnityEngine.Component::get_gameObject()")?,
            component_transform: icall("UnityEngine.Component::get_transform()")?,
            get_parent: m(tr, "get_parent", 0, false, &["UnityEngine.Transform"])?,
            set_parent: m(tr, "SetParent", 2, false, VOID)?,
            inverse_transform_point: method_params(tr, "InverseTransformPoint", &["UnityEngine.Vector3"])?,
            transform_point: method_params(tr, "TransformPoint", &["UnityEngine.Vector3"])?,
            mesh_bounds: m(mesh, "get_bounds", 0, false, &["UnityEngine.Bounds"])?,
            get_lossy_scale: m(tr, "get_lossyScale", 0, false, &["UnityEngine.Vector3"])?,
            set_local_position: m(tr, "set_localPosition", 1, false, VOID)?,
            set_local_rotation: m(tr, "set_localRotation", 1, false, VOID)?,
            set_local_scale: m(tr, "set_localScale", 1, false, VOID)?,
            go_class: go as usize,
            go_ctor: method_params(go, ".ctor", &["System.String"])?,
            go_transform: m(go, "get_transform", 0, false, &["UnityEngine.Transform"])?,
            go_get_layer: m(go, "get_layer", 0, false, &["System.Int32"])?,
            go_set_layer: m(go, "set_layer", 1, false, VOID)?,
            go_active_in_hierarchy: m(go, "get_activeInHierarchy", 0, false, &["System.Boolean"])?,
            add_component: method_params(go, "AddComponent", &["System.Type"])?,
            get_component: method_params(go, "GetComponent", &["System.Type"])?,
            get_bounds: m(renderer, "get_bounds", 0, false, &["UnityEngine.Bounds"])?,
            get_shared_material: m(renderer, "get_sharedMaterial", 0, false, &["UnityEngine.Material"])?,
            set_shared_material: m(renderer, "set_sharedMaterial", 1, false, VOID)?,
            renderer_get_enabled: m(renderer, "get_enabled", 0, false, &["System.Boolean"])?,
            renderer_set_enabled: m(renderer, "set_enabled", 1, false, VOID)?,
            lightmap,
            material_class: material as usize,
            material_ctor: method_params(material, ".ctor", &["UnityEngine.Material"])?,
            get_shader: m(material, "get_shader", 0, false, &["UnityEngine.Shader"])?,
            get_main_texture: m(material, "get_mainTexture", 0, false, &["UnityEngine.Texture"])?,
            set_main_texture: m(material, "set_mainTexture", 1, false, VOID)?,
            set_texture_scale: m(material, "set_mainTextureScale", 1, false, VOID)?,
            set_texture_offset: m(material, "set_mainTextureOffset", 1, false, VOID)?,
            set_float: method_params(material, "SetFloat", &["System.String", "System.Single"]).ok(),
            shader_props,
            mesh_class: mesh as usize,
            mesh_ctor: m(mesh, ".ctor", 0, false, VOID)?,
            set_vertices: m(mesh, "set_vertices", 1, false, VOID)?,
            set_normals: m(mesh, "set_normals", 1, false, VOID)?,
            set_uv: m(mesh, "set_uv", 1, false, VOID)?,
            set_uv2: m(mesh, "set_uv2", 1, false, VOID)?,
            set_colors: method_params(mesh, "set_colors", &["UnityEngine.Color[]"])?,
            set_triangles: m(mesh, "set_triangles", 1, false, VOID)?,
            mesh_readable: m(mesh, "get_isReadable", 0, false, &["System.Boolean"])?,
            get_colors: m(mesh, "get_colors", 0, false, &["UnityEngine.Color[]"])?,
            get_uv2: m(mesh, "get_uv2", 0, false, &["UnityEngine.Vector2[]"])?,
            mesh_filter_type: type_object(mesh_filter)? as usize,
            mesh_renderer_type: type_object(mesh_renderer)? as usize,
            set_shared_mesh: m(mesh_filter, "set_sharedMesh", 1, false, VOID)?,
            get_shared_mesh: m(mesh_filter, "get_sharedMesh", 0, false, &["UnityEngine.Mesh"])?,
            texture2d_class: texture2d as usize,
            texture2d_ctor: method_params(texture2d, ".ctor", &["System.Int32", "System.Int32"])?,
            load_image: m(conversion, "LoadImage", 3, true, &["System.Boolean"])?,
            texture_width: icall("UnityEngine.Texture::GetDataWidth()")?,
            texture_height: icall("UnityEngine.Texture::GetDataHeight()")?,
            texture_wrap: m(texture, "set_wrapMode", 1, false, VOID).ok(),
            texture_aniso: icall("UnityEngine.Texture::set_anisoLevel(System.Int32)").ok(),
            byte_class: class(MSCORLIB, "System", "Byte")? as usize,
            int_class: class(MSCORLIB, "System", "Int32")? as usize,
            vector3_class: class(CORE, "UnityEngine", "Vector3")? as usize,
            vector2_class: class(CORE, "UnityEngine", "Vector2")? as usize,
            color_class: class(CORE, "UnityEngine", "Color")? as usize,
            race_info: m(rm, "get_RaceInfo", 0, true, &["Gallop.RaceInfo"])?,
            race_id: m(race_info, "get_RaceId", 0, false, &["System.Int32"])?,
            race_instance_id: m(race_info, "get_RaceInstanceId", 0, false, &["System.Int32"])?,
        })
    }

    unsafe fn alive(&self, obj: Obj) -> bool {
        if obj.is_null() {
            return false;
        }
        let f: extern "C" fn(Obj, usize) -> bool = std::mem::transmute(self.exists.code);
        f(obj, self.exists.info)
    }

    unsafe fn destroy(&self, obj: Obj) {
        if self.alive(obj) {
            let f: extern "C" fn(Obj, usize) = std::mem::transmute(self.destroy.code);
            f(obj, self.destroy.info)
        }
    }

    unsafe fn name(&self, obj: Obj) -> String {
        if obj.is_null() {
            return String::new();
        }
        api::get().string(call0(self.get_name, obj))
    }

    /// (race instance id, race id) of the race on screen.
    unsafe fn race_ids(&self) -> Option<(i32, i32)> {
        let f: extern "C" fn(usize) -> Obj = std::mem::transmute(self.race_info.code);
        let info = f(self.race_info.info);
        if info.is_null() {
            return None;
        }
        Some((call0(self.race_instance_id, info), call0(self.race_id, info)))
    }

    /// Every renderer in the loaded scenes, inactive ones included.
    unsafe fn renderers(&self) -> Vec<Obj> {
        let f: extern "C" fn(Obj, bool) -> Obj = std::mem::transmute(self.find_objects);
        let arr = f(self.renderer_type as Obj, true);
        if arr.is_null() {
            return Vec::new();
        }
        let len = il2cpp::read::<usize>(arr, 0x18).min(MAX_RENDERERS);
        std::slice::from_raw_parts((arr as *const u8).add(0x20) as *const Obj, len).to_vec()
    }

    /// Transforms from the scene root down to `tr`, with their names.
    unsafe fn path(&self, tr: Obj) -> Vec<(String, Obj)> {
        let mut out = Vec::new();
        let mut t = tr;
        while !t.is_null() && out.len() < 32 {
            out.push((self.name(t), t));
            t = call0(self.get_parent, t);
        }
        out.reverse();
        out
    }

    /// Drawn: enabled and its object active.
    unsafe fn shown(&self, renderer: Obj) -> bool {
        let go: Obj = icall1(self.component_game_object, renderer);
        !go.is_null() && call0::<bool>(self.renderer_get_enabled, renderer) && call0::<bool>(self.go_active_in_hierarchy, go)
    }

    unsafe fn new_object(&self, klass: usize) -> Result<Obj, String> {
        il2cpp::object_new(klass as *mut c_void)
    }

    /// Our picture as a Unity texture, and its width / height.
    unsafe fn load_texture(&self, path: &Path) -> Result<(Obj, f32), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let tex = self.new_object(self.texture2d_class)?;
        call2::<i32, i32, ()>(self.texture2d_ctor, tex, 2, 2);
        let data = il2cpp::array_of(self.byte_class as *mut c_void, &bytes)?;
        let load: extern "C" fn(Obj, Obj, bool, usize) -> bool = std::mem::transmute(self.load_image.code);
        if !load(tex, data, false, self.load_image.info) {
            self.destroy(tex);
            return Err(format!("{} is not a picture Unity can load (PNG/JPG)", path.display()));
        }
        let w: i32 = icall1(self.texture_width, tex);
        let h: i32 = icall1(self.texture_height, tex);
        if w <= 0 || h <= 0 {
            self.destroy(tex);
            return Err(format!("{}: empty picture", path.display()));
        }
        // No bleeding of the opposite edge at the borders; sharp at a slant.
        if let Some(wrap) = self.texture_wrap {
            call1::<i32, ()>(wrap, tex, WRAP_CLAMP);
        }
        if let Some(aniso) = self.texture_aniso {
            let f: extern "C" fn(Obj, i32) = std::mem::transmute(aniso);
            f(tex, ANISO);
        }
        Ok((tex, w as f32 / h as f32))
    }

    /// The banner quad. `uv2` = the area of the plate's lightmap coordinates
    /// it spans (the background shaders read baked light through uv2).
    unsafe fn quad(&self, uv2: ([f32; 2], [f32; 2])) -> Result<Obj, String> {
        let q = quad_mesh();
        let mesh = self.new_object(self.mesh_class)?;
        call0::<()>(self.mesh_ctor, mesh);
        let v2 = self.vector2_class as *mut c_void;
        let v3 = self.vector3_class as *mut c_void;
        let (lo, hi) = uv2;
        let uv2: Vec<[f32; 2]> =
            q.uv.iter().map(|p| [lo[0] + p[0] * (hi[0] - lo[0]), lo[1] + p[1] * (hi[1] - lo[1])]).collect();
        call1::<Obj, ()>(self.set_vertices, mesh, il2cpp::array_of(v3, &q.vertices)?);
        call1::<Obj, ()>(self.set_normals, mesh, il2cpp::array_of(v3, &q.normals)?);
        call1::<Obj, ()>(self.set_uv, mesh, il2cpp::array_of(v2, &q.uv)?);
        call1::<Obj, ()>(self.set_uv2, mesh, il2cpp::array_of(v2, &uv2)?);
        call1::<Obj, ()>(self.set_triangles, mesh, il2cpp::array_of(self.int_class as *mut c_void, &q.triangles)?);
        Ok(mesh)
    }

    unsafe fn set_tint(&self, mesh: Obj, color: [f32; 4]) -> Result<(), String> {
        let colors = il2cpp::array_of(self.color_class as *mut c_void, &[color; 4])?;
        call1::<Obj, ()>(self.set_colors, mesh, colors);
        Ok(())
    }

    /// Vertex colour (average) and uv2 area of a renderer's mesh, when the
    /// game lets us read it.
    unsafe fn mesh_look(&self, renderer: Obj) -> (Option<[f32; 4]>, Option<([f32; 2], [f32; 2])>, String) {
        let go: Obj = icall1(self.component_game_object, renderer);
        let filter: Obj = if go.is_null() { std::ptr::null_mut() } else { call1(self.get_component, go, self.mesh_filter_type as Obj) };
        if filter.is_null() {
            return (None, None, "no mesh filter".to_owned());
        }
        let mesh: Obj = call0(self.get_shared_mesh, filter);
        if !self.alive(mesh) {
            return (None, None, "no mesh".to_owned());
        }
        if !call0::<bool>(self.mesh_readable, mesh) {
            return (None, None, format!("mesh {} not readable", self.name(mesh)));
        }
        let colors: Vec<[f32; 4]> = il2cpp::array_values(call0(self.get_colors, mesh), 65_536);
        let uv2: Vec<[f32; 2]> = il2cpp::array_values(call0(self.get_uv2, mesh), 65_536);
        let color = average_color(&colors);
        let rect = uv_rect(&uv2);
        let note = format!(
            "colour {}, uv2 {}",
            color.map_or("none".to_owned(), |c| format!("{:.2} {:.2} {:.2} {:.2}", c[0], c[1], c[2], c[3])),
            rect.map_or("none".to_owned(), |(a, b)| format!("{:.2},{:.2}..{:.2},{:.2}", a[0], a[1], b[0], b[1]))
        );
        (color, rect, note)
    }
}

// ---------------------------------------------------------------- state

struct Built {
    /// The two quads (front, back).
    gos: [u32; 2],
    material: u32,
    texture: u32,
    mesh: u32,
    /// The game's header renderers (shown when we built).
    header: Vec<u32>,
    header_hidden: bool,
    picture: PathBuf,
    modified: Option<SystemTime>,
    /// Stalls of the gate the banner was made for, and its racecourse.
    stalls: u32,
    course: Option<i32>,
    anchor: String,
    /// The header's box, units in the gate's axes, and its height.
    fit: (V3, V3),
    default_height: f32,
    lossy: V3,
    /// Vertex colour before the brightness slider (the plate's own).
    base_tint: [f32; 4],
    /// The look asked for, and the shader actually copied (for the menu).
    look: Look,
    note: String,
    applied: Option<Tuning>,
}

struct State {
    /// (race manager, race_gen): a new one = a new race.
    key: (usize, u32),
    frame: u32,
    attempts: u32,
    next_look: u32,
    /// GC handle of the gate's transform, once found this race.
    gate: u32,
    dumped: bool,
    /// Lowercase file names in gate_banner/ (refreshed every RETRY_FRAMES).
    files: Vec<String>,
    built: Option<Built>,
    error_logged: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    key: (0, 0),
    frame: 0,
    attempts: 0,
    next_look: 0,
    gate: 0,
    dumped: false,
    files: Vec::new(),
    built: None,
    error_logged: false,
});

/// For the menu: what the banner is doing, and the current race's ids.
static STATUS: Mutex<(String, Option<(i32, i32)>)> = Mutex::new((String::new(), None));

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn status() -> (String, Option<(i32, i32)>) {
    let s = lock(&STATUS);
    let text = if s.0.is_empty() { "Waiting for a race.".to_owned() } else { s.0.clone() };
    (text, s.1)
}

fn set_status(text: String, ids: Option<(i32, i32)>) {
    *lock(&STATUS) = (text, ids);
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn built_status(b: &Built) -> String {
    let size = V3::new(b.fit.1.x - b.fit.0.x, b.fit.1.y - b.fit.0.y, b.fit.1.z - b.fit.0.z);
    let t = b.applied.unwrap_or_default();
    let length = size.x.max(size.z) * t.width;
    let height = super::banner_height(b.default_height, &t);
    let course = b.course.map(|c| format!(", course {c}")).unwrap_or_default();
    format!(
        "Showing {} ({} stalls{course}).\nBest picture shape: {}.\n{}",
        file_name(&b.picture),
        b.stalls,
        super::ratio_hint(length, height),
        b.note
    )
}

// ---------------------------------------------------------------- per frame

/// Once per race frame (main thread), from clock's AlterUpdate hook.
pub fn tick(rm: Obj) {
    if rm.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe { tick_inner(rm) }));
}

unsafe fn tick_inner(rm: Obj) {
    let u = match UNITY.get_or_init(|| {
        let r = Unity::resolve();
        if let Err(e) = &r {
            api::get().error(&format!("gate banner not available: {e}"));
        }
        r
    }) {
        Ok(u) => u,
        Err(e) => {
            set_status(format!("Not available: {e}"), None);
            return;
        }
    };
    let mut st = lock(&STATE);
    st.frame = st.frame.wrapping_add(1);
    let key = (rm as usize, race_ui::race_gen());
    if st.key != key {
        st.key = key;
        st.attempts = 0;
        st.next_look = st.frame;
        st.dumped = false;
        st.error_logged = false;
        il2cpp::gc_free(st.gate);
        st.gate = 0;
    }
    let ids = u.race_ids();

    // The gate surely exists when the gates open: dump the scene then.
    if super::dump_enabled() && !st.dumped && clock::race_live() {
        st.dumped = true;
        write_dump(u, ids);
    }

    let look = st.frame >= st.next_look;
    if look {
        st.next_look = st.frame + RETRY_FRAMES;
        st.files = if super::is_enabled() { super::picture_files(&super::folder()) } else { Vec::new() };
    }
    let pick = |files: &[String], stalls: u32, course: Option<i32>| {
        let (instance, race) = ids?;
        let key = super::RaceKey { instance, race, course };
        super::pick_picture(files, key, stalls).map(|f| super::folder().join(f))
    };

    // A banner already up: keep it while it lives and still is the right picture.
    if let Some(b) = &st.built {
        let want = pick(&st.files, b.stalls, b.course);
        let stale = want.as_ref() != Some(&b.picture)
            || (look && modified(&b.picture) != b.modified)
            || b.look != super::look();
        if stale || !u.alive(il2cpp::gc_target(b.gos[0])) {
            let b = st.built.take();
            teardown(u, b);
        }
    }

    if let Some(b) = st.built.as_mut() {
        let t = super::tuning();
        if b.applied != Some(t) {
            apply(u, b, &t);
        }
        keep_header_hidden(u, b);
        set_status(built_status(b), ids);
        return;
    }

    let any = ids.is_some_and(|(inst, id)| super::has_any_picture(&st.files, inst, id));
    if !any {
        let text = if !super::is_enabled() {
            "Off.".to_owned()
        } else if let Some((inst, id)) = ids {
            format!("No picture for this race: the game's own name plate shows.\nAdd all.png, {id}.png or {inst}.png (_18 / _9 for one gate size).")
        } else {
            "Waiting for a race.".to_owned()
        };
        set_status(text, ids);
        return;
    }
    if !look {
        return;
    }
    // Find the gate (a scene scan) once per race, then wait for it to show:
    // during the gate-in cutscene the game shows its own gate model and keeps
    // the course's gate hidden until just before the start.
    if !u.alive(il2cpp::gc_target(st.gate)) {
        if st.attempts >= MAX_ATTEMPTS {
            return;
        }
        st.attempts += 1;
        match find_gate(u) {
            Ok(g) => {
                il2cpp::gc_free(st.gate);
                st.gate = il2cpp::gc_handle(g.transform);
            }
            Err(e) => {
                if !st.error_logged && st.attempts >= MAX_ATTEMPTS {
                    st.error_logged = true;
                    api::get().warn(&format!("gate banner: {e}"));
                }
                set_status(format!("Not placed: {e}"), ids);
                return;
            }
        }
    }
    let gate_go: Obj = icall1(u.component_game_object, il2cpp::gc_target(st.gate));
    if gate_go.is_null() || !call0::<bool>(u.go_active_in_hierarchy, gate_go) {
        set_status("Waiting for the gate to show.".to_owned(), ids);
        return;
    }
    // Shown: its size (stalls) picks the picture.
    let gate = match find_gate(u) {
        Ok(g) => g,
        Err(e) => {
            set_status(format!("Not placed: {e}"), ids);
            return;
        }
    };
    // The model's stalls (18 or 9 on JRA courses), shown or not: the header's
    // length follows the model, not the number of runners.
    let parts: Vec<&[String]> = gate.parts.iter().map(|p| p.path.as_slice()).collect();
    let stalls = super::stall_count(&parts).unwrap_or(0);
    let course = super::course_from_paths(&parts);
    let Some(picture) = pick(&st.files, stalls, course) else {
        let (inst, id) = ids.unwrap_or_default();
        let course_file = course.map(|c| format!(", course_{c}_{stalls}.png")).unwrap_or_default();
        set_status(
            format!(
                "No picture for a {stalls}-stall gate: the game's own name plate shows.\nAdd all_{stalls}.png{course_file}, {id}_{stalls}.png or {inst}_{stalls}.png."
            ),
            ids,
        );
        // Checked again later, in case the picture is added.
        st.next_look = st.frame + RETRY_FRAMES * 10;
        return;
    };
    match build(u, gate, &picture, stalls, course, super::look()) {
        Ok(mut b) => {
            apply(u, &mut b, &super::tuning());
            api::get().info(&format!("gate banner: {} on {} ({stalls} gates)", picture.display(), b.anchor));
            set_status(built_status(&b), ids);
            st.built = Some(b);
        }
        Err(e) => {
            if !st.error_logged {
                st.error_logged = true;
                api::get().warn(&format!("gate banner: {e}"));
            }
            set_status(format!("Not placed yet: {e}"), ids);
        }
    }
}

unsafe fn teardown(u: &Unity, b: Option<Built>) {
    let Some(mut b) = b else { return };
    show_header(u, &mut b, true);
    let owned = [b.gos[0], b.gos[1], b.material, b.texture, b.mesh];
    for h in owned {
        u.destroy(il2cpp::gc_target(h));
    }
    for h in b.header.iter().copied().chain(owned) {
        il2cpp::gc_free(h);
    }
}

/// Turns the game's header plates on or off (only those that were shown).
unsafe fn show_header(u: &Unity, b: &mut Built, show: bool) {
    if b.header_hidden == !show {
        return;
    }
    b.header_hidden = !show;
    for &h in &b.header {
        let r = il2cpp::gc_target(h);
        if u.alive(r) {
            call1::<bool, ()>(u.renderer_set_enabled, r, show);
        }
    }
}

/// Keeps the header plates off while hiding: the game can turn them back on
/// (they showed again before the start on Kyoto). A few renderers, every frame.
unsafe fn keep_header_hidden(u: &Unity, b: &Built) {
    if !b.header_hidden {
        return;
    }
    for &h in &b.header {
        let r = il2cpp::gc_target(h);
        if u.alive(r) && call0::<bool>(u.renderer_get_enabled, r) {
            call1::<bool, ()>(u.renderer_set_enabled, r, false);
        }
    }
}

struct Part {
    renderer: Obj,
    /// Names below the gate object ("pos_name_plate/.../gate_name_etc").
    path: Vec<String>,
    shown: bool,
}

/// The gate: its transform, its scene path, and its renderers.
struct Gate {
    transform: Obj,
    name: String,
    parts: Vec<Part>,
    /// Every renderer of the scene (to borrow another object's material).
    all: Vec<Obj>,
}

unsafe fn find_gate(u: &Unity) -> Result<Gate, String> {
    let renderers = u.renderers();
    let paths: Vec<Vec<(String, Obj)>> =
        renderers.iter().map(|&r| u.path(icall1(u.component_transform, r))).collect();
    let names: Vec<Vec<String>> = paths.iter().map(|p| p.iter().map(|(n, _)| n.clone()).collect()).collect();
    let wanted = settings::get_string("gate_banner_anchor").map(|s| s.trim().to_lowercase());
    let (pi, ei) = super::pick_anchor(&names, wanted.as_deref()).ok_or_else(|| {
        format!("no gate object among {} renderers (see gate_dump.txt; set gate_banner_anchor)", renderers.len())
    })?;
    let prefix = &names[pi][..=ei];
    let parts = renderers
        .iter()
        .zip(&names)
        .filter(|(_, n)| n.len() > ei && &n[..=ei] == prefix)
        .filter(|(_, n)| !n.iter().any(|s| s == OBJECT_NAME))
        .map(|(&r, n)| Part { renderer: r, path: n[ei + 1..].to_vec(), shown: u.shown(r) })
        .collect();
    Ok(Gate { transform: paths[pi][ei].1, name: prefix.join("/"), parts, all: renderers.clone() })
}

/// Box of `renderers` in the axes of `gate`, in units (the gate's local
/// coordinates times its scale). Each part's mesh box is taken in the part's
/// own axes and carried to the gate's: exact whatever the gate's rotation. A
/// renderer's world box (`Renderer.bounds`) is axis-aligned in world space: on
/// a gate standing at an angle it is much deeper than the part (the Kyoto
/// header measured 2.79 deep instead of 1.0, which put the banners 4.5 apart).
/// The world box is used only for a part without a mesh filter.
unsafe fn local_box(u: &Unity, gate: Obj, lossy: V3, renderers: &[Obj]) -> Option<(V3, V3)> {
    let mut min = V3::new(f32::MAX, f32::MAX, f32::MAX);
    let mut max = V3::new(f32::MIN, f32::MIN, f32::MIN);
    for &r in renderers {
        let go: Obj = icall1(u.component_game_object, r);
        let filter: Obj = if go.is_null() { std::ptr::null_mut() } else { call1(u.get_component, go, u.mesh_filter_type as Obj) };
        let mesh: Obj = if filter.is_null() { std::ptr::null_mut() } else { call0(u.get_shared_mesh, filter) };
        let (b, part): (Bounds, Option<Obj>) = if u.alive(mesh) {
            (call0(u.mesh_bounds, mesh), Some(icall1(u.component_transform, r)))
        } else {
            (call0(u.get_bounds, r), None)
        };
        for corner in super::box_corners(b.center, b.extents) {
            let world = match part {
                Some(tr) => call1(u.transform_point, tr, corner),
                None => corner,
            };
            let l: V3 = call1(u.inverse_transform_point, gate, world);
            let m = V3::new(l.x * lossy.x, l.y * lossy.y, l.z * lossy.z);
            min = V3::new(min.x.min(m.x), min.y.min(m.y), min.z.min(m.z));
            max = V3::new(max.x.max(m.x), max.y.max(m.y), max.z.max(m.z));
        }
    }
    (min.x <= max.x).then_some((min, max))
}

/// The first material in the scene whose shader is `shader`.
unsafe fn material_with_shader(u: &Unity, renderers: &[Obj], shader: &str) -> Option<(Obj, Obj)> {
    renderers.iter().find_map(|&r| {
        let m: Obj = call0(u.get_shared_material, r);
        (u.alive(m) && u.name(call0(u.get_shader, m)) == shader).then_some((r, m))
    })
}

unsafe fn build(
    u: &Unity,
    gate: Gate,
    picture: &Path,
    stalls: u32,
    course: Option<i32>,
    look: Look,
) -> Result<Built, String> {
    let extra = settings::get_string("gate_banner_hide").unwrap_or_default();
    let extra: Vec<&str> = extra.split(',').map(str::trim).filter(|t| !t.is_empty()).collect();
    let parts: Vec<&str> = HEADER_PARTS.iter().copied().chain(extra).collect();
    let shown: Vec<&Part> = gate.parts.iter().filter(|p| p.shown).collect();
    let header: Vec<&Part> = shown.iter().copied().filter(|p| is_header(&p.path, &HEADER_GROUPS, &parts)).collect();
    // The name plate first: its material is the one to copy.
    let plate: Vec<&Part> = header.iter().copied().filter(|p| is_header(&p.path, &HEADER_GROUPS, &[])).collect();

    let lossy: V3 = call0(u.get_lossy_scale, gate.transform);
    let lossy = V3::new(nonzero(lossy.x), nonzero(lossy.y), nonzero(lossy.z));
    let (fit, default_height) = if !header.is_empty() {
        let r: Vec<Obj> = header.iter().map(|p| p.renderer).collect();
        let b = local_box(u, gate.transform, lossy, &r).ok_or("the gate's header has no size")?;
        (b, b.1.y - b.0.y)
    } else {
        // No known header: a strip along the top of the gate.
        let r: Vec<Obj> = shown.iter().map(|p| p.renderer).collect();
        let (min, max) = local_box(u, gate.transform, lossy, &r).ok_or("the gate has no visible renderers")?;
        let h = (max.y - min.y) * 0.12;
        ((V3::new(min.x, max.y - h, min.z), max), h)
    };

    // The chosen look's material, else the name plate's.
    let borrowed = look.shader().and_then(|shader| material_with_shader(u, &gate.all, shader));
    let from_plate = borrowed.is_none();
    let source = borrowed
        .or_else(|| {
            plate
                .iter()
                .chain(header.iter())
                .chain(shown.iter())
                .map(|p| (p.renderer, call0::<Obj>(u.get_shared_material, p.renderer)))
                .find(|&(_, m)| u.alive(m))
        })
        .ok_or_else(|| format!("gate {} has no material to copy", gate.name))?;
    let shader_name = u.name(call0(u.get_shader, source.1));

    let (texture, _aspect) = u.load_texture(picture)?;
    let mut handles = vec![il2cpp::gc_handle(texture)];
    let result = (|| -> Result<Built, String> {
        let material = u.new_object(u.material_class)?;
        call1::<Obj, ()>(u.material_ctor, material, source.1);
        handles.push(il2cpp::gc_handle(material));
        call1::<Obj, ()>(u.set_main_texture, material, texture);
        call1::<V2, ()>(u.set_texture_scale, material, V2 { x: 1.0, y: 1.0 });
        call1::<V2, ()>(u.set_texture_offset, material, V2 { x: 0.0, y: 0.0 });

        if let Some(set_float) = u.set_float {
            for name in ["_Cull", "_CullMode"] {
                call2::<Obj, f32, ()>(set_float, material, il2cpp::new_string(name), CULL_BACK);
            }
        }

        // The plate's shader reads baked light from vertex colours and uv2:
        // copy the plate mesh's. Other looks show the picture as is (white).
        let (color, uv2, plate_note) =
            if from_plate { u.mesh_look(source.0) } else { (None, None, String::new()) };
        let mesh = u.quad(uv2.unwrap_or(([0.0, 0.0], [1.0, 1.0])))?;
        handles.push(il2cpp::gc_handle(mesh));

        // Same layer as the gate, so the race cameras draw it.
        let gate_go: Obj = icall1(u.component_game_object, gate.transform);
        let layer: i32 = call0(u.go_get_layer, gate_go);
        let mut gos = [0u32; 2];
        for slot in &mut gos {
            let go = u.new_object(u.go_class)?;
            call1::<Obj, ()>(u.go_ctor, go, il2cpp::new_string(OBJECT_NAME));
            *slot = il2cpp::gc_handle(go);
            handles.push(*slot);
            call1::<i32, ()>(u.go_set_layer, go, layer);
            let tr: Obj = call0(u.go_transform, go);
            call2::<Obj, bool, ()>(u.set_parent, tr, gate.transform, false);
            let filter: Obj = call1(u.add_component, go, u.mesh_filter_type as Obj);
            let renderer: Obj = call1(u.add_component, go, u.mesh_renderer_type as Obj);
            if filter.is_null() || renderer.is_null() {
                return Err("cannot add the mesh components".to_owned());
            }
            call1::<Obj, ()>(u.set_shared_mesh, filter, mesh);
            call1::<Obj, ()>(u.set_shared_material, renderer, material);
            // Lit like the plate it replaces.
            if let Ok(l) = &u.lightmap {
                let index: i32 = call0(l.get_index, source.0);
                let scale_offset: V4 = call0(l.get_scale_offset, source.0);
                call1::<i32, ()>(l.set_index, renderer, index);
                call1::<V4, ()>(l.set_scale_offset, renderer, scale_offset);
            }
        }

        Ok(Built {
            gos,
            material: handles[1],
            texture: handles[0],
            mesh: handles[2],
            header: header.iter().map(|p| il2cpp::gc_handle(p.renderer)).collect(),
            header_hidden: false,
            picture: picture.to_owned(),
            modified: modified(picture),
            stalls,
            course,
            anchor: gate.name.clone(),
            fit,
            default_height,
            lossy,
            base_tint: if from_plate { color.unwrap_or(FALLBACK_TINT) } else { [1.0; 4] },
            look,
            note: if from_plate {
                format!("Shader: {shader_name} (plate {plate_note})")
            } else {
                format!("Shader: {shader_name}")
            },
            applied: None,
        })
    })();
    if result.is_err() {
        for &h in &handles {
            u.destroy(il2cpp::gc_target(h));
            il2cpp::gc_free(h);
        }
    }
    result
}

fn nonzero(v: f32) -> f32 {
    if v.is_finite() && v.abs() > 1e-6 { v } else { 1.0 }
}

unsafe fn apply(u: &Unity, b: &mut Built, t: &Tuning) {
    if b.applied.is_none_or(|a| a.brightness != t.brightness) {
        let mesh = il2cpp::gc_target(b.mesh);
        if u.alive(mesh) {
            let _ = u.set_tint(mesh, tint(b.base_tint, t.brightness));
        }
    }
    b.applied = Some(*t);
    show_header(u, b, !t.hide_header);
    let Some(p) = place(b.fit.0, b.fit.1, b.default_height, t) else { return };
    let ls = b.lossy;
    // The quad's X runs along the gate's long axis; undo the gate's scale on it.
    let scale = if p.along_x {
        V3::new(p.width / ls.x, p.height / ls.y, 1.0 / ls.z)
    } else {
        V3::new(p.width / ls.z, p.height / ls.y, 1.0 / ls.x)
    };
    for (handle, face) in b.gos.iter().zip(p.faces) {
        let go = il2cpp::gc_target(*handle);
        if !u.alive(go) {
            continue;
        }
        let tr: Obj = call0(u.go_transform, go);
        let pos = V3::new(face.center.x / ls.x, face.center.y / ls.y, face.center.z / ls.z);
        call1::<V3, ()>(u.set_local_position, tr, pos);
        call1::<Quat, ()>(u.set_local_rotation, tr, Quat(yaw_quat(face.yaw)));
        call1::<V3, ()>(u.set_local_scale, tr, scale);
    }
}

// ---------------------------------------------------------------- dump

/// gate_dump.txt: every renderer of the scene (path, layer, material,
/// texture, lightmap, world box), the gate's shaders' properties, and the
/// game's classes named "*Gate*".
unsafe fn write_dump(u: &Unity, ids: Option<(i32, i32)>) {
    let mut out = String::from("# Scene renderers at the gates opening (written by the gate banner)\n");
    if let Some((inst, id)) = ids {
        let _ = writeln!(out, "# race instance id {inst}, race id {id}");
    }
    let renderers = u.renderers();
    let _ = writeln!(
        out,
        "# {} renderers; path | layer | on | material | shader | texture | lightmap | center | size\n",
        renderers.len()
    );
    let mut gate_shaders: Vec<(String, Obj)> = Vec::new();
    for &r in renderers.iter().take(MAX_DUMPED) {
        let tr: Obj = icall1(u.component_transform, r);
        let path: Vec<String> = u.path(tr).into_iter().map(|(n, _)| n).collect();
        let path = path.join("/");
        let go: Obj = icall1(u.component_game_object, r);
        let layer: i32 = if go.is_null() { -1 } else { call0(u.go_get_layer, go) };
        let mat: Obj = call0(u.get_shared_material, r);
        let (mat_name, shader_name, tex) = if u.alive(mat) {
            let shader: Obj = call0(u.get_shader, mat);
            let shader_name = u.name(shader);
            if path.to_lowercase().contains("gate") && !gate_shaders.iter().any(|(n, _)| *n == shader_name) {
                gate_shaders.push((shader_name.clone(), shader));
            }
            let tex: Obj = call0(u.get_main_texture, mat);
            let tex = if u.alive(tex) {
                let w: i32 = icall1(u.texture_width, tex);
                let h: i32 = icall1(u.texture_height, tex);
                format!("{} {w}x{h}", u.name(tex))
            } else {
                "-".to_owned()
            };
            (u.name(mat), shader_name, tex)
        } else {
            ("-".to_owned(), "-".to_owned(), "-".to_owned())
        };
        let lightmap = match &u.lightmap {
            Ok(l) => {
                let i: i32 = call0(l.get_index, r);
                let so: V4 = call0(l.get_scale_offset, r);
                format!("{i} [{:.3} {:.3} {:.3} {:.3}]", so.0[0], so.0[1], so.0[2], so.0[3])
            }
            Err(_) => "?".to_owned(),
        };
        let b: Bounds = call0(u.get_bounds, r);
        let _ = writeln!(
            out,
            "{path} | {layer} | {} | {mat_name} | {shader_name} | {tex} | {lightmap} | ({:.2}, {:.2}, {:.2}) | ({:.2}, {:.2}, {:.2})",
            if u.shown(r) { "on" } else { "off" },
            b.center.x,
            b.center.y,
            b.center.z,
            b.extents.x * 2.0,
            b.extents.y * 2.0,
            b.extents.z * 2.0
        );
    }
    if renderers.len() > MAX_DUMPED {
        let _ = writeln!(out, "... {} more", renderers.len() - MAX_DUMPED);
    }

    out.push_str("\n# Properties of the gate's shaders\n\n");
    match &u.shader_props {
        Ok(sp) => {
            for (name, shader) in &gate_shaders {
                if !u.alive(*shader) {
                    continue;
                }
                let n: i32 = call0(sp.count, *shader);
                let props: Vec<String> =
                    (0..n.clamp(0, 200)).map(|i| api::get().string(call1::<i32, Obj>(sp.name, *shader, i))).collect();
                let _ = writeln!(out, "{name}: {}", props.join(", "));
            }
        }
        Err(e) => {
            let _ = writeln!(out, "not available: {e}");
        }
    }

    out.push_str("\n# Gallop classes named *Gate*\n\n");
    let gate_classes: Vec<_> = il2cpp::classes(GAME)
        .into_iter()
        .filter(|&k| il2cpp::class_full_name(k).rsplit('.').next().is_some_and(|n| n.contains("Gate")))
        .collect();
    for &k in gate_classes.iter().take(40) {
        out.push_str(&il2cpp::describe_class(k));
        out.push('\n');
    }

    let path = tracer::plugin_dir().join("gate_dump.txt");
    match std::fs::write(&path, out) {
        Ok(()) => api::get().info(&format!("gate banner: wrote {}", path.display())),
        Err(e) => api::get().warn(&format!("cannot write {}: {e}", path.display())),
    }
}
