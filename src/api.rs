//! Bindings to the Hachimi Edge plugin API (v3, resolved by name through `get_api`).
//! Signatures mirror `src/core/plugin_api.rs` in kairusds/Hachimi-Edge.

use std::ffi::{c_char, c_void, CString};
use std::sync::OnceLock;

pub type GetApiFn = extern "C" fn(name: *const c_char) -> *mut c_void;
pub type UiCallback = extern "C" fn(ui: *mut c_void, userdata: *mut c_void);
pub type GameInitializedCallback = unsafe extern "C" fn(userdata: *mut c_void);

macro_rules! api_struct {
    ($( $name:ident : $ty:ty ),* $(,)?) => {
        pub struct Api { $( pub $name: $ty, )* }

        impl Api {
            /// Resolves every function we use. Returns None if the host lacks one.
            pub fn load(get_api: GetApiFn) -> Option<Api> {
                Some(Api { $(
                    $name: {
                        let p = get_api(concat!(stringify!($name), "\0").as_ptr() as *const c_char);
                        if p.is_null() { return None; }
                        unsafe { std::mem::transmute::<*mut c_void, $ty>(p) }
                    },
                )* })
            }
        }
    };
}

api_struct! {
    hachimi_instance: unsafe extern "C" fn() -> *const c_void,
    hachimi_get_interceptor: unsafe extern "C" fn(this: *const c_void) -> *const c_void,
    hachimi_register_on_game_initialized:
        unsafe extern "C" fn(cb: Option<GameInitializedCallback>, userdata: *mut c_void) -> bool,
    hachimi_get_base_dir: unsafe extern "C" fn() -> *const c_char,

    // Returns the trampoline (call it to run the original), or null on failure.
    interceptor_hook:
        unsafe extern "C" fn(this: *const c_void, orig: *mut c_void, hook: *mut c_void) -> *mut c_void,

    il2cpp_resolve_symbol: unsafe extern "C" fn(name: *const c_char) -> *mut c_void,
    il2cpp_get_assembly_image: unsafe extern "C" fn(name: *const c_char) -> *const c_void,
    il2cpp_get_class: unsafe extern "C" fn(
        image: *const c_void, namespace: *const c_char, name: *const c_char,
    ) -> *mut c_void,
    il2cpp_class_get_methods:
        unsafe extern "C" fn(klass: *mut c_void, iter: *mut *mut c_void) -> *const c_void,
    il2cpp_get_field_from_name:
        unsafe extern "C" fn(klass: *mut c_void, name: *const c_char) -> *mut c_void,
    il2cpp_resolve_icall: unsafe extern "C" fn(name: *const c_char) -> *mut c_void,
    il2cpp_get_singleton_like_instance: unsafe extern "C" fn(klass: *mut c_void) -> *mut c_void,
    il2cpp_string_new: unsafe extern "C" fn(text: *const c_char) -> *mut c_void,
    il2cpp_get_main_thread: unsafe extern "C" fn() -> *mut c_void,
    il2cpp_schedule_on_thread: unsafe extern "C" fn(thread: *mut c_void, cb: unsafe extern "C" fn()),

    log: unsafe extern "C" fn(level: i32, target: *const c_char, message: *const c_char),

    gui_register_menu_section:
        unsafe extern "C" fn(cb: Option<UiCallback>, userdata: *mut c_void) -> bool,
    gui_show_notification: unsafe extern "C" fn(message: *const c_char) -> bool,
    gui_ui_heading: unsafe extern "C" fn(ui: *mut c_void, text: *const c_char) -> bool,
    gui_ui_label: unsafe extern "C" fn(ui: *mut c_void, text: *const c_char) -> bool,
    gui_ui_small: unsafe extern "C" fn(ui: *mut c_void, text: *const c_char) -> bool,
    gui_ui_separator: unsafe extern "C" fn(ui: *mut c_void) -> bool,
    gui_ui_checkbox:
        unsafe extern "C" fn(ui: *mut c_void, text: *const c_char, value: *mut bool) -> bool,
}

static API: OnceLock<Api> = OnceLock::new();

pub fn set(api: Api) -> &'static Api {
    API.get_or_init(|| api)
}

/// Panics only if called before `hachimi_init_v3`, which cannot happen:
/// every caller is a callback registered from there.
pub fn get() -> &'static Api {
    API.get().expect("api not initialized")
}

/// Lossy CString: interior NULs are dropped instead of failing.
pub fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

impl Api {
    fn log_at(&self, level: i32, msg: &str) {
        let tag = cstr(crate::TAG);
        let msg = cstr(msg);
        unsafe { (self.log)(level, tag.as_ptr(), msg.as_ptr()) }
    }
    pub fn error(&self, msg: &str) { self.log_at(1, msg) }
    pub fn warn(&self, msg: &str) { self.log_at(2, msg) }
    pub fn info(&self, msg: &str) { self.log_at(3, msg) }

    pub fn notify(&self, msg: &str) {
        let msg = cstr(msg);
        unsafe { (self.gui_show_notification)(msg.as_ptr()); }
    }

    /// Looks up a raw il2cpp export (e.g. `il2cpp_class_get_name`) and casts it.
    /// Safety: `T` must be the correct fn pointer type for that export.
    pub unsafe fn il2cpp_fn<T: Copy>(&self, name: &str) -> Option<T> {
        debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<usize>());
        let name = cstr(name);
        let p = (self.il2cpp_resolve_symbol)(name.as_ptr());
        if p.is_null() { None } else { Some(std::mem::transmute_copy::<*mut c_void, T>(&p)) }
    }
}
