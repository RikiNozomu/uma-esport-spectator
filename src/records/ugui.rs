//! The record cards as the game's own UI (Unity UI), on a canvas of ours.
//!
//! - A `Canvas` (screen-space overlay, `SORTING_ORDER`) with a `CanvasScaler`
//!   (reference 1920x1080, matched on height: canvas units = 1080p points) and
//!   a `CanvasGroup` (the fade; never blocks clicks), kept across scenes.
//! - Children: the dark gradient across the bottom, then a container (scaled
//!   by `layout::fit_scale` on narrow screens, moved for the rise / sink)
//!   holding two card slots: `Image` strips and box, `RawImage` picture, `Text`
//!   in the game's font (from the `UI/Parts/Notification` prefab's TextCommon).
//! - `tick` (main thread) lays the cards out when a show starts and moves /
//!   fades them every frame. It is scheduled from the present callback
//!   (`request_tick`) while cards show, so it also runs outside races (the
//!   menu's preview).
//!
//! Every object is created by us and checked alive before use.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

use super::layout::{self, Box2};
use super::{Card, Frame};
use crate::il2cpp::{self, Method, Obj, CORE, GAME, UI, UI_MODULE};
use crate::{api, tracer};

const IMAGE_CONVERSION: &str = "UnityEngine.ImageConversionModule.dll";
const MSCORLIB: &str = "mscorlib.dll";
const FONT_PREFAB: &str = "UI/Parts/Notification";
const OBJECT_NAME: &str = "UmaSpectatorRecordCards";
/// Above the game's UI; Hachimi's menu and our egui overlays draw later anyway.
const SORTING_ORDER: i32 = 900;
const MAX_CARDS: usize = 2;

// Unity enums.
const RENDER_MODE_OVERLAY: i32 = 0;
const SCALE_WITH_SCREEN_SIZE: i32 = 1;
const MATCH_WIDTH_OR_HEIGHT: i32 = 0;
const ANCHOR_MIDDLE_LEFT: i32 = 3;
const ANCHOR_MIDDLE_CENTER: i32 = 4;
const WRAP: i32 = 0;
const TRUNCATE: i32 = 0;
const STYLE_NORMAL: i32 = 0;
const STYLE_BOLD: i32 = 1;
const WRAP_CLAMP: i32 = 1;

const YELLOW: Color = Color::rgb(210, 200, 0);
const WHITE: Color = Color::rgb(255, 255, 255);
const INK: Color = Color::rgb(10, 10, 10);
/// The gradient's darkest alpha (at the bottom of the screen).
const SHADE_ALPHA: u8 = 170;

#[repr(C)]
#[derive(Clone, Copy)]
struct V2 {
    x: f32,
    y: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct V3 {
    x: f32,
    y: f32,
    z: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Color {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

impl Color {
    const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
    }
}

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

unsafe fn static0<R>(m: Method) -> R {
    let f: extern "C" fn(usize) -> R = std::mem::transmute(m.code);
    f(m.info)
}

unsafe fn static1<A, R>(m: Method, a: A) -> R {
    let f: extern "C" fn(A, usize) -> R = std::mem::transmute(m.code);
    f(a, m.info)
}

unsafe fn static2<A, B, R>(m: Method, a: A, b: B) -> R {
    let f: extern "C" fn(A, B, usize) -> R = std::mem::transmute(m.code);
    f(a, b, m.info)
}

unsafe fn static3<A, B, C, R>(m: Method, a: A, b: B, c: C) -> R {
    let f: extern "C" fn(A, B, C, usize) -> R = std::mem::transmute(m.code);
    f(a, b, c, m.info)
}

struct Unity {
    go_class: usize,
    go_ctor: Method,
    add_component: Method,
    set_active: usize,
    go_transform: Method,
    component_in_children: Method,
    dont_destroy: Method,
    exists: Method,
    destroy: Method,
    set_parent: Method,
    set_local_scale: Method,
    anchor_min: Method,
    anchor_max: Method,
    pivot: Method,
    anchored_position: Method,
    size_delta: Method,
    screen_width: Method,
    screen_height: Method,
    resources_load: Method,
    render_mode: Method,
    sorting_order: Method,
    scale_mode: Method,
    reference_resolution: Method,
    match_mode: Method,
    match_value: Method,
    group_alpha: Method,
    group_blocks: Method,
    group_interactable: Method,
    graphic_color: Method,
    raw_texture: Method,
    get_font: Method,
    set_font: Method,
    set_text: Method,
    font_size: Method,
    font_style: Method,
    alignment: Method,
    h_overflow: Method,
    v_overflow: Method,
    best_fit: Method,
    best_min: Method,
    best_max: Method,
    texture2d_class: usize,
    texture2d_ctor: Method,
    load_image: Method,
    texture_width: usize,
    texture_height: usize,
    texture_wrap: Option<Method>,
    byte_class: usize,
    t_game_object: usize,
    t_rect: usize,
    t_canvas: usize,
    t_scaler: usize,
    t_group: usize,
    t_image: usize,
    t_raw: usize,
    t_text: usize,
    t_text_common: usize,
}

unsafe impl Send for Unity {}
unsafe impl Sync for Unity {}

impl Unity {
    unsafe fn resolve() -> Result<Unity, String> {
        use il2cpp::{class, expect, icall, method, method_params, type_object};
        let m = |k: *mut c_void, name: &str, argc: u32, is_static: bool, ret: &[&str]| -> Result<Method, String> {
            let found = method(k, name, argc).map_err(|e| format!("{}: {e}", il2cpp::class_full_name(k)))?;
            expect(found, is_static, ret, argc).map_err(|e| format!("{}::{name}: {e}", il2cpp::class_full_name(k)))?;
            Ok(found)
        };
        let void = &["System.Void"][..];
        let object = class(CORE, "UnityEngine", "Object")?;
        let go = class(CORE, "UnityEngine", "GameObject")?;
        let tr = class(CORE, "UnityEngine", "Transform")?;
        let rect = class(CORE, "UnityEngine", "RectTransform")?;
        let screen = class(CORE, "UnityEngine", "Screen")?;
        let resources = class(CORE, "UnityEngine", "Resources")?;
        let texture = class(CORE, "UnityEngine", "Texture")?;
        let texture2d = class(CORE, "UnityEngine", "Texture2D")?;
        let canvas = class(UI_MODULE, "UnityEngine", "Canvas")?;
        let group = class(UI_MODULE, "UnityEngine", "CanvasGroup")?;
        let scaler = class(UI, "UnityEngine.UI", "CanvasScaler")?;
        let graphic = class(UI, "UnityEngine.UI", "Graphic")?;
        let image = class(UI, "UnityEngine.UI", "Image")?;
        let raw = class(UI, "UnityEngine.UI", "RawImage")?;
        let text = class(UI, "UnityEngine.UI", "Text")?;
        let text_common = class(GAME, "Gallop", "TextCommon")?;
        let conversion = class(IMAGE_CONVERSION, "UnityEngine", "ImageConversion")?;
        Ok(Unity {
            go_class: go as usize,
            go_ctor: method_params(go, ".ctor", &["System.String"])?,
            add_component: method_params(go, "AddComponent", &["System.Type"])?,
            set_active: icall("UnityEngine.GameObject::SetActive(System.Boolean)")?,
            go_transform: m(go, "get_transform", 0, false, &["UnityEngine.Transform"])?,
            component_in_children: method(go, "GetComponentInChildren", 2)?,
            dont_destroy: m(object, "DontDestroyOnLoad", 1, true, void)?,
            exists: m(object, "op_Implicit", 1, true, &["System.Boolean"])?,
            destroy: m(object, "Destroy", 1, true, void)?,
            set_parent: m(tr, "SetParent", 2, false, void)?,
            set_local_scale: m(tr, "set_localScale", 1, false, void)?,
            anchor_min: m(rect, "set_anchorMin", 1, false, void)?,
            anchor_max: m(rect, "set_anchorMax", 1, false, void)?,
            pivot: m(rect, "set_pivot", 1, false, void)?,
            anchored_position: m(rect, "set_anchoredPosition", 1, false, void)?,
            size_delta: m(rect, "set_sizeDelta", 1, false, void)?,
            screen_width: m(screen, "get_width", 0, true, &["System.Int32"])?,
            screen_height: m(screen, "get_height", 0, true, &["System.Int32"])?,
            resources_load: m(resources, "Load", 2, true, &["UnityEngine.Object"])?,
            render_mode: m(canvas, "set_renderMode", 1, false, void)?,
            sorting_order: m(canvas, "set_sortingOrder", 1, false, void)?,
            scale_mode: m(scaler, "set_uiScaleMode", 1, false, void)?,
            reference_resolution: m(scaler, "set_referenceResolution", 1, false, void)?,
            match_mode: m(scaler, "set_screenMatchMode", 1, false, void)?,
            match_value: m(scaler, "set_matchWidthOrHeight", 1, false, void)?,
            group_alpha: m(group, "set_alpha", 1, false, void)?,
            group_blocks: m(group, "set_blocksRaycasts", 1, false, void)?,
            group_interactable: m(group, "set_interactable", 1, false, void)?,
            graphic_color: m(graphic, "set_color", 1, false, void)?,
            raw_texture: m(raw, "set_texture", 1, false, void)?,
            get_font: m(text, "get_font", 0, false, &["UnityEngine.Font"])?,
            set_font: m(text, "set_font", 1, false, void)?,
            set_text: m(text, "set_text", 1, false, void)?,
            font_size: m(text, "set_fontSize", 1, false, void)?,
            font_style: m(text, "set_fontStyle", 1, false, void)?,
            alignment: m(text, "set_alignment", 1, false, void)?,
            h_overflow: m(text, "set_horizontalOverflow", 1, false, void)?,
            v_overflow: m(text, "set_verticalOverflow", 1, false, void)?,
            best_fit: m(text, "set_resizeTextForBestFit", 1, false, void)?,
            best_min: m(text, "set_resizeTextMinSize", 1, false, void)?,
            best_max: m(text, "set_resizeTextMaxSize", 1, false, void)?,
            texture2d_class: texture2d as usize,
            texture2d_ctor: method_params(texture2d, ".ctor", &["System.Int32", "System.Int32"])?,
            load_image: m(conversion, "LoadImage", 3, true, &["System.Boolean"])?,
            texture_width: icall("UnityEngine.Texture::GetDataWidth()")?,
            texture_height: icall("UnityEngine.Texture::GetDataHeight()")?,
            texture_wrap: m(texture, "set_wrapMode", 1, false, void).ok(),
            byte_class: class(MSCORLIB, "System", "Byte")? as usize,
            t_game_object: type_object(go)? as usize,
            t_rect: type_object(rect)? as usize,
            t_canvas: type_object(canvas)? as usize,
            t_scaler: type_object(scaler)? as usize,
            t_group: type_object(group)? as usize,
            t_image: type_object(image)? as usize,
            t_raw: type_object(raw)? as usize,
            t_text: type_object(text)? as usize,
            t_text_common: type_object(text_common)? as usize,
        })
    }

    unsafe fn alive(&self, obj: Obj) -> bool {
        !obj.is_null() && static1::<Obj, bool>(self.exists, obj)
    }

    unsafe fn destroy(&self, obj: Obj) {
        if self.alive(obj) {
            static1::<Obj, ()>(self.destroy, obj);
        }
    }

    unsafe fn activate(&self, go: Obj, on: bool) {
        let f: extern "C" fn(Obj, bool) = std::mem::transmute(self.set_active);
        f(go, on)
    }

    unsafe fn add(&self, go: Obj, ty: usize) -> Result<Obj, String> {
        let c: Obj = call1(self.add_component, go, ty as Obj);
        if c.is_null() { Err("AddComponent failed".to_owned()) } else { Ok(c) }
    }

    unsafe fn new_go(&self, name: &str) -> Result<Obj, String> {
        let go = il2cpp::object_new(self.go_class as *mut c_void)?;
        call1::<Obj, ()>(self.go_ctor, go, il2cpp::new_string(name));
        Ok(go)
    }

    /// A child of `parent` with a RectTransform anchored at the parent's
    /// bottom-left; returns (game object, rect transform).
    unsafe fn child(&self, parent: Obj, name: &str) -> Result<(Obj, Obj), String> {
        let go = self.new_go(name)?;
        let rt = self.add(go, self.t_rect)?;
        call2::<Obj, bool, ()>(self.set_parent, rt, parent, false);
        self.corner(rt);
        Ok((go, rt))
    }

    unsafe fn corner(&self, rt: Obj) {
        call1::<V2, ()>(self.anchor_min, rt, V2 { x: 0.0, y: 0.0 });
        call1::<V2, ()>(self.anchor_max, rt, V2 { x: 0.0, y: 0.0 });
        call1::<V2, ()>(self.pivot, rt, V2 { x: 0.0, y: 0.0 });
    }

    unsafe fn place(&self, rt: Obj, b: Box2) {
        call1::<V2, ()>(self.anchored_position, rt, V2 { x: b.x, y: b.y });
        call1::<V2, ()>(self.size_delta, rt, V2 { x: b.w, y: b.h });
    }

    unsafe fn color(&self, graphic: Obj, c: Color) {
        call1::<Color, ()>(self.graphic_color, graphic, c);
    }

    /// A PNG as a texture: (texture, width, height).
    unsafe fn texture_from_png(&self, bytes: &[u8]) -> Result<(Obj, f32, f32), String> {
        let tex = il2cpp::object_new(self.texture2d_class as *mut c_void)?;
        call2::<i32, i32, ()>(self.texture2d_ctor, tex, 2, 2);
        let data = il2cpp::array_of(self.byte_class as *mut c_void, bytes)?;
        if !static3::<Obj, Obj, bool, bool>(self.load_image, tex, data, false) {
            self.destroy(tex);
            return Err("not a PNG Unity can load".to_owned());
        }
        if let Some(wrap) = self.texture_wrap {
            call1::<i32, ()>(wrap, tex, WRAP_CLAMP);
        }
        let w: i32 = (std::mem::transmute::<usize, extern "C" fn(Obj) -> i32>(self.texture_width))(tex);
        let h: i32 = (std::mem::transmute::<usize, extern "C" fn(Obj) -> i32>(self.texture_height))(tex);
        Ok((tex, w as f32, h as f32))
    }

    /// The game's UI font, from the notification prefab's TextCommon.
    unsafe fn game_font(&self) -> Result<Obj, String> {
        let prefab: Obj = static2(self.resources_load, il2cpp::new_string(FONT_PREFAB), self.t_game_object as Obj);
        if prefab.is_null() {
            return Err(format!("prefab not found: {FONT_PREFAB}"));
        }
        let label: Obj = call2(self.component_in_children, prefab, self.t_text_common as Obj, true);
        if label.is_null() {
            return Err("no TextCommon in the notification prefab".to_owned());
        }
        let font: Obj = call0(self.get_font, label);
        if font.is_null() { Err("the game's font is not loaded".to_owned()) } else { Ok(font) }
    }

    unsafe fn text(&self, parent: Obj, name: &str, font: Obj, style: i32, align: i32) -> Result<(Obj, Obj), String> {
        let (go, rt) = self.child(parent, name)?;
        let t = self.add(go, self.t_text)?;
        call1::<Obj, ()>(self.set_font, t, font);
        call1::<i32, ()>(self.font_style, t, style);
        call1::<i32, ()>(self.alignment, t, align);
        call1::<i32, ()>(self.h_overflow, t, WRAP);
        call1::<i32, ()>(self.v_overflow, t, TRUNCATE);
        call1::<bool, ()>(self.best_fit, t, true);
        call1::<i32, ()>(self.best_min, t, 10);
        self.color(t, INK);
        Ok((rt, t))
    }
}

// ---------------------------------------------------------------- the UI

struct TextUi {
    rt: u32,
    text: u32,
}

struct CardUi {
    go: u32,
    head: u32,
    body: u32,
    date: u32,
    picture_go: u32,
    picture_rt: u32,
    picture: u32,
    heading: TextUi,
    holder: TextUi,
    time: TextUi,
    date_text: TextUi,
}

struct Ui {
    root: u32,
    group: u32,
    container: u32,
    cards: Vec<CardUi>,
}

fn h(obj: Obj) -> u32 {
    unsafe { il2cpp::gc_handle(obj) }
}

fn o(handle: u32) -> Obj {
    unsafe { il2cpp::gc_target(handle) }
}

/// The dark gradient: a column, transparent at the top, `SHADE_ALPHA` at the bottom.
fn shade_png() -> Vec<u8> {
    use image::ImageEncoder;
    let height = 128u32;
    let img = image::RgbaImage::from_fn(2, height, |_, y| {
        let t = y as f32 / (height - 1) as f32; // 0 at the top row
        image::Rgba([0, 0, 0, (t * t * SHADE_ALPHA as f32) as u8])
    });
    let mut out = Vec::new();
    let _ = image::codecs::png::PngEncoder::new(&mut out).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
    );
    out
}

unsafe fn build(u: &Unity) -> Result<Ui, String> {
    let font = u.game_font()?;
    let root = u.new_go(OBJECT_NAME)?;
    let canvas = u.add(root, u.t_canvas)?;
    call1::<i32, ()>(u.render_mode, canvas, RENDER_MODE_OVERLAY);
    call1::<i32, ()>(u.sorting_order, canvas, SORTING_ORDER);
    let scaler = u.add(root, u.t_scaler)?;
    call1::<i32, ()>(u.scale_mode, scaler, SCALE_WITH_SCREEN_SIZE);
    call1::<V2, ()>(u.reference_resolution, scaler, V2 { x: 1920.0, y: 1080.0 });
    call1::<i32, ()>(u.match_mode, scaler, MATCH_WIDTH_OR_HEIGHT);
    call1::<f32, ()>(u.match_value, scaler, 1.0);
    let group = u.add(root, u.t_group)?;
    call1::<bool, ()>(u.group_blocks, group, false);
    call1::<bool, ()>(u.group_interactable, group, false);
    call1::<f32, ()>(u.group_alpha, group, 0.0);
    static1::<Obj, ()>(u.dont_destroy, root);
    let root_tr: Obj = call0(u.go_transform, root);

    // The gradient across the bottom of the screen (not scaled, not moved).
    let (shade_go, shade_rt) = u.child(root_tr, "Shade")?;
    call1::<V2, ()>(u.anchor_max, shade_rt, V2 { x: 1.0, y: 0.0 });
    call1::<V2, ()>(u.anchored_position, shade_rt, V2 { x: 0.0, y: 0.0 });
    call1::<V2, ()>(u.size_delta, shade_rt, V2 { x: 0.0, y: layout::SHADE_H });
    let shade = u.add(shade_go, u.t_raw)?;
    match u.texture_from_png(&shade_png()) {
        Ok((tex, _, _)) => {
            h(tex);
            call1::<Obj, ()>(u.raw_texture, shade, tex);
        }
        Err(_) => u.activate(shade_go, false),
    }

    let (_, container) = u.child(root_tr, "Cards")?;
    let mut cards = Vec::new();
    for i in 0..MAX_CARDS {
        let (go, card_rt) = u.child(container, &format!("Card{i}"))?;
        let strip = |name: &str, c: Color| -> Result<u32, String> {
            let (sgo, srt) = u.child(card_rt, name)?;
            let img = u.add(sgo, u.t_image)?;
            u.color(img, c);
            Ok(h(srt))
        };
        let head = strip("Heading", YELLOW)?;
        let body = strip("Body", WHITE)?;
        let date = strip("Date", YELLOW)?;
        let (pgo, prt) = u.child(card_rt, "Picture")?;
        let picture = u.add(pgo, u.t_raw)?;
        let text = |name: &str, style: i32, align: i32| -> Result<TextUi, String> {
            let (rt, t) = u.text(card_rt, name, font, style, align)?;
            Ok(TextUi { rt: h(rt), text: h(t) })
        };
        cards.push(CardUi {
            go: h(go),
            head,
            body,
            date,
            picture_go: h(pgo),
            picture_rt: h(prt),
            picture: h(picture),
            heading: text("HeadingText", STYLE_NORMAL, ANCHOR_MIDDLE_LEFT)?,
            holder: text("Holder", STYLE_BOLD, ANCHOR_MIDDLE_LEFT)?,
            time: text("Time", STYLE_BOLD, ANCHOR_MIDDLE_CENTER)?,
            date_text: text("DateText", STYLE_BOLD, ANCHOR_MIDDLE_CENTER)?,
        });
    }
    u.activate(root, false);
    api::get().info("record cards: UI created");
    Ok(Ui { root: h(root), group: h(group), container: h(container), cards })
}

// ---------------------------------------------------------------- state

struct Picture {
    texture: u32,
    modified: Option<SystemTime>,
    size: (f32, f32),
}

struct State {
    ui: Option<Ui>,
    unity: Option<Result<Unity, String>>,
    /// The show laid out, and the screen it was laid out for.
    laid_out: Option<(u32, i32, i32)>,
    pictures: BTreeMap<PathBuf, Option<Picture>>,
    error: Option<String>,
}

static STATE: Mutex<State> =
    Mutex::new(State { ui: None, unity: None, laid_out: None, pictures: BTreeMap::new(), error: None });
/// The root is active (cards on screen or fading).
static ACTIVE: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicBool = AtomicBool::new(false);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The UI's problem, if any (for the menu).
pub fn error() -> Option<String> {
    lock(&STATE).error.clone()
}

/// Asks the main thread to update the cards; every frame while they show.
/// Any thread (called from the present callback).
pub fn request_tick() {
    if !tracer::GAME_READY.load(Ordering::Acquire) {
        return;
    }
    if !super::is_showing() && !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    if PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    let api = api::get();
    unsafe { (api.il2cpp_schedule_on_thread)((api.il2cpp_get_main_thread)(), on_main_thread) };
}

unsafe extern "C" fn on_main_thread() {
    let result = catch_unwind(AssertUnwindSafe(|| tick()));
    PENDING.store(false, Ordering::Release);
    if result.is_err() {
        api::get().error("record cards: update panicked");
        super::hide();
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Main thread.
fn tick() {
    let mut st = lock(&STATE);
    let st = &mut *st;
    let u = match st.unity.get_or_insert_with(|| unsafe { Unity::resolve() }) {
        Ok(u) => u as *const Unity,
        Err(e) => {
            let msg = format!("Unity UI not available: {e}");
            if st.error.as_deref() != Some(&msg) {
                api::get().error(&format!("record cards: {msg}"));
                st.error = Some(msg);
            }
            super::hide();
            return;
        }
    };
    // Safety: `unity` is never replaced once set.
    let u = unsafe { &*u };
    let frame = super::next_frame();

    let alive = st.ui.as_ref().is_some_and(|ui| unsafe { u.alive(o(ui.root)) });
    let Some(f) = frame else {
        if alive {
            unsafe { u.activate(o(st.ui.as_ref().expect("alive").root), false) };
        }
        ACTIVE.store(false, Ordering::Relaxed);
        return;
    };
    if !alive {
        match unsafe { build(u) } {
            Ok(ui) => {
                st.ui = Some(ui);
                st.laid_out = None;
                st.error = None;
            }
            Err(e) => {
                api::get().error(&format!("record cards: cannot create the cards: {e}"));
                st.error = Some(format!("cannot create the cards: {e}"));
                super::hide();
                return;
            }
        }
    }
    unsafe {
        let (sw, sh): (i32, i32) = (static0(u.screen_width), static0(u.screen_height));
        if st.laid_out != Some((f.serial, sw, sh)) {
            st.laid_out = Some((f.serial, sw, sh));
            lay_out(u, st, &f, sw, sh);
        }
        let ui = st.ui.as_ref().expect("built");
        call1::<f32, ()>(u.group_alpha, o(ui.group), f.alpha);
        let (scale, _) = fit(sw, sh, f.cards.len());
        call1::<V2, ()>(u.anchored_position, o(ui.container), V2 { x: 0.0, y: -f.offset * scale });
        u.activate(o(ui.root), true);
    }
    ACTIVE.store(true, Ordering::Relaxed);
}

/// (scale, width in points of the scaled container) for the screen.
fn fit(sw: i32, sh: i32, count: usize) -> (f32, f32) {
    let width = if sh > 0 { 1080.0 * sw as f32 / sh as f32 } else { 1920.0 };
    let scale = layout::fit_scale(width, count);
    (scale, width / scale)
}

/// The picture's texture (cached, reloaded when the file changes).
unsafe fn picture(u: &Unity, st: &mut State, path: &Path) -> Option<(Obj, f32, f32)> {
    let m = modified(path);
    let stale = match st.pictures.get(path) {
        Some(Some(p)) => p.modified != m,
        Some(None) => m.is_some(),
        None => true,
    };
    if stale {
        if let Some(Some(old)) = st.pictures.remove(path) {
            u.destroy(o(old.texture));
            il2cpp::gc_free(old.texture);
        }
        let loaded = std::fs::read(path).ok().and_then(|bytes| u.texture_from_png(&bytes).ok());
        let entry = loaded.map(|(tex, pw, ph)| Picture { texture: h(tex), modified: m, size: (pw, ph) });
        st.pictures.insert(path.to_owned(), entry);
    }
    match st.pictures.get(path) {
        Some(Some(p)) if u.alive(o(p.texture)) => Some((o(p.texture), p.size.0, p.size.1)),
        _ => None,
    }
}

unsafe fn set_text(u: &Unity, t: &TextUi, b: Box2, size: f32, s: &str) {
    u.place(o(t.rt), b);
    let text = o(t.text);
    call1::<i32, ()>(u.font_size, text, size.round() as i32);
    call1::<i32, ()>(u.best_max, text, size.round() as i32);
    call1::<Obj, ()>(u.set_text, text, il2cpp::new_string(s));
}

unsafe fn lay_out(u: &Unity, st: &mut State, f: &Frame, sw: i32, sh: i32) {
    let (scale, width) = fit(sw, sh, f.cards.len());
    let lefts = layout::group_lefts(width, f.cards.len());
    let textures: Vec<Option<(Obj, f32, f32)>> = f
        .cards
        .iter()
        .map(|c: &Card| c.picture.as_deref().and_then(|p| picture(u, st, &super::folder().join(p))))
        .collect();
    let ui = st.ui.as_ref().expect("built");
    call1::<V3, ()>(u.set_local_scale, o(ui.container), V3 { x: scale, y: scale, z: 1.0 });
    for (i, slot) in ui.cards.iter().enumerate() {
        let (Some(card), Some(left)) = (f.cards.get(i), lefts.get(i)) else {
            u.activate(o(slot.go), false);
            continue;
        };
        u.activate(o(slot.go), true);
        let tex = textures[i];
        let r = layout::card_rects(*left, tex.is_some());
        u.place(o(slot.head), r.head);
        u.place(o(slot.body), r.body);
        u.place(o(slot.date), r.date);
        match (tex, r.picture) {
            (Some((t, w, h)), Some(area)) => {
                call1::<Obj, ()>(u.raw_texture, o(slot.picture), t);
                u.place(o(slot.picture_rt), layout::fit_picture(area, w, h));
                u.activate(o(slot.picture_go), true);
            }
            _ => u.activate(o(slot.picture_go), false),
        }
        set_text(u, &slot.heading, r.heading_text, layout::HEADING_SIZE, &card.heading);
        let holder_align = if r.holder_centred { ANCHOR_MIDDLE_CENTER } else { ANCHOR_MIDDLE_LEFT };
        call1::<i32, ()>(u.alignment, o(slot.holder.text), holder_align);
        set_text(u, &slot.holder, r.holder_text, layout::HOLDER_SIZE, &card.holder.to_uppercase());
        set_text(u, &slot.time, r.time_text, layout::TIME_SIZE, &card.time);
        set_text(u, &slot.date_text, r.date_text, layout::DATE_SIZE, &card.date);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shade_is_a_png_dark_at_the_bottom() {
        let png = shade_png();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(img.get_pixel(0, 0)[3], 0);
        assert_eq!(img.get_pixel(0, img.height() - 1)[3], SHADE_ALPHA);
    }

    #[test]
    fn fits_the_screen() {
        assert_eq!(fit(1920, 1080, 2), (1.0, 1920.0));
        let (s, w) = fit(1440, 1080, 2);
        assert!(s < 1.0 && (w * s - 1440.0).abs() < 0.01);
    }
}
