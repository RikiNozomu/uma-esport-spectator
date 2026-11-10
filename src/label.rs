//! An in-game text label drawn by the game itself (Unity UI), so it shows in
//! exclusive fullscreen and in OBS game capture.
//!
//! Same technique as Hachimi's captions (src/core/captions.rs in Hachimi-Edge):
//! clone the game's `UI/Parts/Notification` prefab under UIManager's notice
//! canvas and take its `Gallop.TextCommon` label, which carries the game's
//! font and outline. We move that label to the canvas root, anchor it to the
//! top-left corner and hide the rest of the notification.
//!
//! Main thread only (every call here touches Unity objects).

use std::ffi::c_void;

use crate::il2cpp::{self, Method, Obj, CORE, GAME, UI};

const PREFAB: &str = "UI/Parts/Notification";

// UnityEngine enums.
pub const TEXT_ANCHOR_UPPER_LEFT: i32 = 0;
pub const TEXT_ANCHOR_UPPER_RIGHT: i32 = 2;
const WRAP_OVERFLOW: i32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct Vector2 {
    x: f32,
    y: f32,
}

/// Where and how big the label is (canvas units of the notice canvas).
/// The box is anchored to a top corner of the screen; `margin_*` is from
/// that corner to the same corner of the box.
pub struct Layout {
    /// Top-right corner instead of top-left.
    pub right: bool,
    /// TEXT_ANCHOR_* inside the box.
    pub alignment: i32,
    pub font_size: i32,
    /// 1.0 = font default; smaller pulls lines together.
    pub line_spacing: f32,
    pub margin_x: f32,
    pub margin_y: f32,
    pub width: f32,
    pub height: f32,
    /// Gallop.OutlineColorType / OutlineSizeType member names, first match wins.
    pub outline_color: &'static [&'static str],
    pub outline_size: &'static [&'static str],
}

/// TextCommon's own outline (the game's text style), as Hachimi's captions set it.
struct Outline {
    set_size: Method,
    update: Method,
    set_color: Method,
    rebuild: Method,
    size_enum: usize,
    color_enum: usize,
}

impl Outline {
    unsafe fn resolve(text_common: *mut c_void) -> Result<Outline, String> {
        use il2cpp::{class, expect, method};
        let set_size = method(text_common, "set_OutlineSize", 1)?;
        expect(set_size, false, &["System.Void"], 1)?;
        let update = method(text_common, "UpdateOutline", 0)?;
        expect(update, false, &["System.Void"], 0)?;
        let set_color = method(text_common, "set_OutlineColor", 1)?;
        expect(set_color, false, &["System.Void"], 1)?;
        let rebuild = method(text_common, "RebuildOutline", 0)?;
        expect(rebuild, false, &["System.Void"], 0)?;
        Ok(Outline {
            set_size,
            update,
            set_color,
            rebuild,
            size_enum: class(GAME, "Gallop", "OutlineSizeType")? as usize,
            color_enum: class(GAME, "Gallop", "OutlineColorType")? as usize,
        })
    }

    /// Returns the member names used, for the log.
    unsafe fn apply(&self, u: &Unity, label: Obj, layout: &Layout) -> Result<String, String> {
        let (size, size_name) = il2cpp::enum_value(self.size_enum as *mut c_void, layout.outline_size)
            .map_err(|e| format!("OutlineSizeType: {e}"))?;
        let (color, color_name) = il2cpp::enum_value(self.color_enum as *mut c_void, layout.outline_color)
            .map_err(|e| format!("OutlineColorType: {e}"))?;
        let call0 = |m: Method| {
            let f: extern "C" fn(Obj, usize) = std::mem::transmute(m.code);
            f(label, m.info)
        };
        u.i32(self.set_size, label, size);
        call0(self.update);
        u.i32(self.set_color, label, color);
        call0(self.rebuild);
        Ok(format!("{color_name}, size {size_name}"))
    }
}

/// Unity methods the label needs, resolved once.
pub struct Unity {
    resources_load: Method,
    clone_with_parent: usize,
    get_component_in_children: Method,
    component_transform: usize,
    component_game_object: usize,
    set_active: usize,
    set_parent: Method,
    exists: Method,
    rt_anchor_min: Method,
    rt_anchor_max: Method,
    rt_pivot: Method,
    rt_anchored_position: Method,
    rt_size_delta: Method,
    text_set_text: Method,
    text_font_size: Method,
    text_alignment: Method,
    text_rich_text: Method,
    text_h_overflow: Method,
    text_v_overflow: Method,
    text_best_fit: Method,
    text_line_spacing: Method,
    /// Optional: without it the label keeps the prefab's outline.
    outline: Result<Outline, String>,
    game_object_type: usize,
    text_common_type: usize,
    ui_manager_class: usize,
    notice_canvas_offset: usize,
}

type ObjGetter = extern "C" fn(Obj) -> Obj;
type SetV2 = extern "C" fn(Obj, Vector2, usize);
type SetI32 = extern "C" fn(Obj, i32, usize);
type SetBool = extern "C" fn(Obj, bool, usize);

impl Unity {
    pub unsafe fn resolve() -> Result<Unity, String> {
        use il2cpp::{class, icall, method};
        let resources = class(CORE, "UnityEngine", "Resources")?;
        let game_object = class(CORE, "UnityEngine", "GameObject")?;
        let transform = class(CORE, "UnityEngine", "Transform")?;
        let rect = class(CORE, "UnityEngine", "RectTransform")?;
        let object = class(CORE, "UnityEngine", "Object")?;
        let text = class(UI, "UnityEngine.UI", "Text")?;
        let text_common = class(GAME, "Gallop", "TextCommon")?;
        let ui_manager = class(GAME, "Gallop", "UIManager")?;

        let resources_load = method(resources, "Load", 2)?;
        il2cpp::expect(resources_load, true, &["UnityEngine.Object"], 2)?;
        let exists = method(object, "op_Implicit", 1)?;
        il2cpp::expect(exists, true, &["System.Boolean"], 1)?;
        let get_component_in_children = method(game_object, "GetComponentInChildren", 2)?;
        il2cpp::expect(get_component_in_children, false, &["UnityEngine.Component"], 2)?;
        let set_parent = method(transform, "SetParent", 2)?;
        il2cpp::expect(set_parent, false, &["System.Void"], 2)?;

        Ok(Unity {
            resources_load,
            clone_with_parent: icall("UnityEngine.Object::Internal_CloneSingleWithParent()")?,
            get_component_in_children,
            component_transform: icall("UnityEngine.Component::get_transform()")?,
            component_game_object: icall("UnityEngine.Component::get_gameObject()")?,
            set_active: icall("UnityEngine.GameObject::SetActive(System.Boolean)")?,
            set_parent,
            exists,
            rt_anchor_min: method(rect, "set_anchorMin", 1)?,
            rt_anchor_max: method(rect, "set_anchorMax", 1)?,
            rt_pivot: method(rect, "set_pivot", 1)?,
            rt_anchored_position: method(rect, "set_anchoredPosition", 1)?,
            rt_size_delta: method(rect, "set_sizeDelta", 1)?,
            text_set_text: method(text, "set_text", 1)?,
            text_font_size: method(text, "set_fontSize", 1)?,
            text_alignment: method(text, "set_alignment", 1)?,
            text_rich_text: method(text, "set_supportRichText", 1)?,
            text_h_overflow: method(text, "set_horizontalOverflow", 1)?,
            text_v_overflow: method(text, "set_verticalOverflow", 1)?,
            text_best_fit: method(text, "set_resizeTextForBestFit", 1)?,
            text_line_spacing: {
                let m = method(text, "set_lineSpacing", 1)?;
                il2cpp::expect(m, false, &["System.Void"], 1)?;
                m
            },
            outline: Outline::resolve(text_common),
            game_object_type: il2cpp::type_object(game_object)? as usize,
            text_common_type: il2cpp::type_object(text_common)? as usize,
            ui_manager_class: ui_manager as usize,
            notice_canvas_offset: il2cpp::field_offset(ui_manager, "_noticeCanvas")?,
        })
    }

    unsafe fn alive(&self, obj: Obj) -> bool {
        if obj.is_null() {
            return false;
        }
        let f: extern "C" fn(Obj, usize) -> bool = std::mem::transmute(self.exists.code);
        f(obj, self.exists.info)
    }

    unsafe fn get(&self, icall: usize, obj: Obj) -> Obj {
        let f: ObjGetter = std::mem::transmute(icall);
        f(obj)
    }

    unsafe fn set_active(&self, go: Obj, on: bool) {
        let f: extern "C" fn(Obj, bool) = std::mem::transmute(self.set_active);
        f(go, on)
    }

    unsafe fn v2(&self, m: Method, obj: Obj, x: f32, y: f32) {
        let f: SetV2 = std::mem::transmute(m.code);
        f(obj, Vector2 { x, y }, m.info)
    }

    unsafe fn i32(&self, m: Method, obj: Obj, v: i32) {
        let f: SetI32 = std::mem::transmute(m.code);
        f(obj, v, m.info)
    }

    unsafe fn bool(&self, m: Method, obj: Obj, v: bool) {
        let f: SetBool = std::mem::transmute(m.code);
        f(obj, v, m.info)
    }
}

/// The label. Holds a GC handle so the managed object stays valid.
pub struct Label {
    handle: u32,
    shown: bool,
    text: String,
    /// Current (margin_x, margin_y); layouts sharing a label may differ only in these.
    margins: (f32, f32),
}

impl Label {
    pub const fn new() -> Label {
        Label { handle: 0, shown: false, text: String::new(), margins: (0.0, 0.0) }
    }

    /// The live TextCommon, creating it if missing or destroyed.
    unsafe fn ensure(&mut self, u: &Unity, layout: &Layout) -> Result<Obj, String> {
        let current = il2cpp::gc_target(self.handle);
        if u.alive(current) {
            return Ok(current);
        }
        let label = create(u, layout)?;
        self.handle = il2cpp::gc_handle(label);
        self.shown = false;
        self.text.clear();
        self.margins = (layout.margin_x, layout.margin_y);
        Ok(label)
    }

    /// Shows `text` (Unity rich text) at `layout`'s position. Only touches
    /// Unity when something changed. Font and size come from the layout the
    /// label was created with; later layouts only move it.
    pub unsafe fn show(&mut self, u: &Unity, layout: &Layout, text: &str) -> Result<(), String> {
        let label = self.ensure(u, layout)?;
        let margins = (layout.margin_x, layout.margin_y);
        if self.margins != margins {
            let tr = u.get(u.component_transform, label);
            let x = if layout.right { -margins.0 } else { margins.0 };
            u.v2(u.rt_anchored_position, tr, x, -margins.1);
            self.margins = margins;
        }
        if self.text != text {
            let f: extern "C" fn(Obj, Obj, usize) = std::mem::transmute(u.text_set_text.code);
            f(label, il2cpp::new_string(text), u.text_set_text.info);
            self.text.clear();
            self.text.push_str(text);
        }
        if !self.shown {
            u.set_active(u.get(u.component_game_object, label), true);
            self.shown = true;
        }
        Ok(())
    }

    pub unsafe fn hide(&mut self, u: &Unity) {
        if !self.shown {
            return;
        }
        self.shown = false;
        let label = il2cpp::gc_target(self.handle);
        if u.alive(label) {
            u.set_active(u.get(u.component_game_object, label), false);
        }
    }
}

unsafe fn create(u: &Unity, layout: &Layout) -> Result<Obj, String> {
    let api = crate::api::get();
    let ui_manager = (api.il2cpp_get_singleton_like_instance)(u.ui_manager_class as *mut c_void);
    if ui_manager.is_null() {
        return Err("UIManager not available".to_owned());
    }
    let canvas: Obj = il2cpp::read(ui_manager, u.notice_canvas_offset);
    if !u.alive(canvas) {
        return Err("notice canvas not available".to_owned());
    }
    let canvas_tr = u.get(u.component_transform, canvas);

    let load: extern "C" fn(Obj, Obj, usize) -> Obj = std::mem::transmute(u.resources_load.code);
    let prefab = load(il2cpp::new_string(PREFAB), u.game_object_type as Obj, u.resources_load.info);
    if prefab.is_null() {
        return Err(format!("prefab not found: {PREFAB}"));
    }
    let clone_fn: extern "C" fn(Obj, Obj, bool) -> Obj = std::mem::transmute(u.clone_with_parent);
    let clone = clone_fn(prefab, canvas_tr, false);
    if clone.is_null() {
        return Err("cannot instantiate the notification prefab".to_owned());
    }
    let find: extern "C" fn(Obj, Obj, bool, usize) -> Obj = std::mem::transmute(u.get_component_in_children.code);
    let label = find(clone, u.text_common_type as Obj, true, u.get_component_in_children.info);
    if label.is_null() {
        u.set_active(clone, false);
        return Err("notification prefab has no TextCommon".to_owned());
    }

    // Move the label to the canvas root; the rest of the notification stays hidden.
    let label_tr = u.get(u.component_transform, label);
    let set_parent: extern "C" fn(Obj, Obj, bool, usize) = std::mem::transmute(u.set_parent.code);
    set_parent(label_tr, canvas_tr, false, u.set_parent.info);
    u.set_active(clone, false);

    u.bool(u.text_rich_text, label, true);
    u.bool(u.text_best_fit, label, false);
    u.i32(u.text_h_overflow, label, WRAP_OVERFLOW);
    u.i32(u.text_v_overflow, label, WRAP_OVERFLOW);
    u.i32(u.text_alignment, label, layout.alignment);
    u.i32(u.text_font_size, label, layout.font_size);
    let spacing: extern "C" fn(Obj, f32, usize) = std::mem::transmute(u.text_line_spacing.code);
    spacing(label, layout.line_spacing, u.text_line_spacing.info);

    let api = crate::api::get();
    match u.outline.as_ref().map_err(Clone::clone).and_then(|o| o.apply(u, label, layout)) {
        Ok(used) => api.info(&format!("race clock outline: {used}")),
        Err(e) => api.warn(&format!("race clock outline not set: {e}")),
    }

    // Anchor and pivot at the chosen top corner; x goes inwards, y down.
    let (corner_x, x) = if layout.right { (1.0, -layout.margin_x) } else { (0.0, layout.margin_x) };
    u.v2(u.rt_anchor_min, label_tr, corner_x, 1.0);
    u.v2(u.rt_anchor_max, label_tr, corner_x, 1.0);
    u.v2(u.rt_pivot, label_tr, corner_x, 1.0);
    u.v2(u.rt_anchored_position, label_tr, x, -layout.margin_y);
    u.v2(u.rt_size_delta, label_tr, layout.width, layout.height);

    u.set_active(u.get(u.component_game_object, label), false);
    crate::api::get().info("race clock label created");
    Ok(label)
}
