//! Readable method signatures from IL2CPP metadata, e.g.
//! `static System.Boolean (Gallop.HorseRaceInfo horse, System.Int32 skillId)`.
//!
//! Must run on a thread attached to IL2CPP (the main thread or plugin init),
//! never on the hook hot path.

use std::ffi::{c_char, c_void, CStr};
use std::sync::OnceLock;

use crate::api;

const METHOD_ATTRIBUTE_STATIC: u32 = 0x0010;

struct Fns {
    return_type: unsafe extern "C" fn(method: *const c_void) -> *const c_void,
    param_count: unsafe extern "C" fn(method: *const c_void) -> u32,
    param: unsafe extern "C" fn(method: *const c_void, index: u32) -> *const c_void,
    param_name: unsafe extern "C" fn(method: *const c_void, index: u32) -> *const c_char,
    flags: unsafe extern "C" fn(method: *const c_void, iflags: *mut u32) -> u32,
    type_name: unsafe extern "C" fn(ty: *const c_void) -> *mut c_char,
    free: Option<unsafe extern "C" fn(ptr: *mut c_void)>,
}

fn fns() -> Option<&'static Fns> {
    static FNS: OnceLock<Option<Fns>> = OnceLock::new();
    FNS.get_or_init(|| unsafe {
        let api = api::get();
        Some(Fns {
            return_type: api.il2cpp_fn("il2cpp_method_get_return_type")?,
            param_count: api.il2cpp_fn("il2cpp_method_get_param_count")?,
            param: api.il2cpp_fn("il2cpp_method_get_param")?,
            param_name: api.il2cpp_fn("il2cpp_method_get_param_name")?,
            flags: api.il2cpp_fn("il2cpp_method_get_flags")?,
            type_name: api.il2cpp_fn("il2cpp_type_get_name")?,
            free: api.il2cpp_fn("il2cpp_free"),
        })
    })
    .as_ref()
}

/// Type name of an `Il2CppType*`; the string il2cpp allocates is freed here.
unsafe fn type_name(f: &Fns, ty: *const c_void) -> String {
    if ty.is_null() {
        return "?".to_owned();
    }
    let p = (f.type_name)(ty);
    if p.is_null() {
        return "?".to_owned();
    }
    let s = CStr::from_ptr(p).to_string_lossy().into_owned();
    if let Some(free) = f.free {
        free(p as *mut c_void);
    }
    s
}

/// Type name of an `Il2CppType*` (e.g. a field's type); "?" if unavailable.
pub unsafe fn type_name_of(ty: *const c_void) -> String {
    match fns() {
        Some(f) => type_name(f, ty),
        None => "?".to_owned(),
    }
}

/// Fully-qualified return type name, or None if the metadata API is missing.
pub unsafe fn return_type(method: *const c_void) -> Option<String> {
    let f = fns()?;
    Some(type_name(f, (f.return_type)(method)))
}

/// `[static ]Ret (Type name, ...)`; "?" if the metadata API is missing.
pub unsafe fn signature(method: *const c_void) -> String {
    let Some(f) = fns() else { return "?".to_owned() };
    let mut out = String::new();
    if (f.flags)(method, std::ptr::null_mut()) & METHOD_ATTRIBUTE_STATIC != 0 {
        out.push_str("static ");
    }
    out.push_str(&type_name(f, (f.return_type)(method)));
    out.push_str(" (");
    for i in 0..(f.param_count)(method) {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&type_name(f, (f.param)(method, i)));
        let name = (f.param_name)(method, i);
        if !name.is_null() {
            out.push(' ');
            out.push_str(&CStr::from_ptr(name).to_string_lossy());
        }
    }
    out.push(')');
    out
}
