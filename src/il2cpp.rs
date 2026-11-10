//! Helpers over the Hachimi IL2CPP API: class/method/field lookups, hooks,
//! object checks. Lookups are slow and some allocate: do them at install time
//! (or once), cache the results, and keep hot paths to cached pointers.
//!
//! Calling convention reminder (IL2CPP, Win64): a compiled method is
//! `extern "C" fn([this,] args..., method: *const MethodInfo)`. Icalls
//! (`il2cpp_resolve_icall`) take no MethodInfo.

use std::ffi::{c_char, c_void, CStr};
use std::sync::OnceLock;

use crate::api::{self, cstr};
use crate::sig;

pub const GAME: &str = "umamusume.dll";
pub const CORE: &str = "UnityEngine.CoreModule.dll";
pub const UI: &str = "UnityEngine.UI.dll";
pub const UI_MODULE: &str = "UnityEngine.UIModule.dll";

pub type Obj = *mut c_void;

/// A resolved method. `info` is the MethodInfo passed as the hidden last
/// argument; `code` is the compiled function.
#[derive(Clone, Copy)]
pub struct Method {
    pub info: usize,
    pub code: usize,
}

struct Fns {
    method_name: unsafe extern "C" fn(m: *const c_void) -> *const c_char,
    method_param_count: unsafe extern "C" fn(m: *const c_void) -> u32,
    class_parent: unsafe extern "C" fn(k: *mut c_void) -> *mut c_void,
    class_name: unsafe extern "C" fn(k: *mut c_void) -> *const c_char,
    class_namespace: unsafe extern "C" fn(k: *mut c_void) -> *const c_char,
    class_fields: unsafe extern "C" fn(k: *mut c_void, iter: *mut *mut c_void) -> *mut c_void,
    class_type: unsafe extern "C" fn(k: *mut c_void) -> *const c_void,
    type_object: unsafe extern "C" fn(t: *const c_void) -> Obj,
    class_assignable: unsafe extern "C" fn(k: *mut c_void, from: *mut c_void) -> bool,
    object_class: unsafe extern "C" fn(o: Obj) -> *mut c_void,
    field_name: unsafe extern "C" fn(f: *mut c_void) -> *const c_char,
    field_type: unsafe extern "C" fn(f: *mut c_void) -> *const c_void,
    field_offset: unsafe extern "C" fn(f: *mut c_void) -> usize,
    field_flags: unsafe extern "C" fn(f: *mut c_void) -> i32,
    field_static_value: unsafe extern "C" fn(f: *mut c_void, out: *mut c_void),
    gchandle_new: unsafe extern "C" fn(o: Obj, pinned: bool) -> u32,
    gchandle_target: unsafe extern "C" fn(h: u32) -> Obj,
}

fn fns() -> Result<&'static Fns, String> {
    static FNS: OnceLock<Result<Fns, String>> = OnceLock::new();
    FNS.get_or_init(|| unsafe {
        let api = api::get();
        macro_rules! sym {
            ($n:literal) => {
                api.il2cpp_fn($n).ok_or_else(|| format!("il2cpp symbol not found: {}", $n))?
            };
        }
        Ok(Fns {
            method_name: sym!("il2cpp_method_get_name"),
            method_param_count: sym!("il2cpp_method_get_param_count"),
            class_parent: sym!("il2cpp_class_get_parent"),
            class_name: sym!("il2cpp_class_get_name"),
            class_namespace: sym!("il2cpp_class_get_namespace"),
            class_fields: sym!("il2cpp_class_get_fields"),
            class_type: sym!("il2cpp_class_get_type"),
            type_object: sym!("il2cpp_type_get_object"),
            class_assignable: sym!("il2cpp_class_is_assignable_from"),
            object_class: sym!("il2cpp_object_get_class"),
            field_name: sym!("il2cpp_field_get_name"),
            field_type: sym!("il2cpp_field_get_type"),
            field_offset: sym!("il2cpp_field_get_offset"),
            field_flags: sym!("il2cpp_field_get_flags"),
            field_static_value: sym!("il2cpp_field_static_get_value"),
            gchandle_new: sym!("il2cpp_gchandle_new"),
            gchandle_target: sym!("il2cpp_gchandle_get_target"),
        })
    })
    .as_ref()
    .map_err(Clone::clone)
}

pub unsafe fn lossy(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
}

pub unsafe fn class(assembly: &str, ns: &str, name: &str) -> Result<*mut c_void, String> {
    let api = api::get();
    let image = (api.il2cpp_get_assembly_image)(cstr(assembly).as_ptr());
    if image.is_null() {
        return Err(format!("assembly not found: {assembly}"));
    }
    let k = (api.il2cpp_get_class)(image, cstr(ns).as_ptr(), cstr(name).as_ptr());
    if k.is_null() { Err(format!("class not found: {ns}.{name}")) } else { Ok(k) }
}

/// Finds `name` with `argc` parameters in `klass` or its parents, without
/// logging when it is missing (some lookups are optional).
pub unsafe fn method(klass: *mut c_void, name: &str, argc: u32) -> Result<Method, String> {
    let f = fns()?;
    let api = api::get();
    let mut k = klass;
    for _ in 0..32 {
        if k.is_null() {
            break;
        }
        let mut iter: *mut c_void = std::ptr::null_mut();
        loop {
            let m = (api.il2cpp_class_get_methods)(k, &mut iter);
            if m.is_null() {
                break;
            }
            if (f.method_param_count)(m) == argc && lossy((f.method_name)(m)) == name {
                let code = *(m as *const usize);
                if code == 0 {
                    return Err(format!("{name}({argc}) has no code"));
                }
                return Ok(Method { info: m as usize, code });
            }
        }
        k = (f.class_parent)(k);
    }
    Err(format!("method not found: {name}({argc})"))
}

/// Like `method`, for overloads with the same parameter count: picks the one
/// whose parameter types are exactly `params` (e.g. `["System.Type"]`).
pub unsafe fn method_params(klass: *mut c_void, name: &str, params: &[&str]) -> Result<Method, String> {
    let f = fns()?;
    let api = api::get();
    let argc = params.len() as u32;
    let mut k = klass;
    for _ in 0..32 {
        if k.is_null() {
            break;
        }
        let mut iter: *mut c_void = std::ptr::null_mut();
        loop {
            let m = (api.il2cpp_class_get_methods)(k, &mut iter);
            if m.is_null() {
                break;
            }
            if (f.method_param_count)(m) != argc || lossy((f.method_name)(m)) != name {
                continue;
            }
            let matches = params
                .iter()
                .enumerate()
                .all(|(i, p)| sig::param_type(m, i as u32).as_deref() == Some(*p));
            let code = *(m as *const usize);
            if matches && code != 0 {
                return Ok(Method { info: m as usize, code });
            }
        }
        k = (f.class_parent)(k);
    }
    Err(format!("method not found: {name}({})", params.join(", ")))
}

/// Checks a method against the signature we are about to call it with.
pub unsafe fn expect(m: Method, is_static: bool, returns: &[&str], params: u32) -> Result<(), String> {
    let s = sig::shape(m.info as *const c_void).ok_or("cannot read method metadata")?;
    if s.is_static != is_static || s.params != params || !returns.contains(&s.ret.as_str()) {
        return Err(format!(
            "unexpected signature {}{} ({} params)",
            if s.is_static { "static " } else { "" },
            s.ret,
            s.params
        ));
    }
    Ok(())
}

pub unsafe fn return_type(m: Method) -> String {
    sig::return_type(m.info as *const c_void).unwrap_or_default()
}

pub unsafe fn icall(name: &str) -> Result<usize, String> {
    let p = (api::get().il2cpp_resolve_icall)(cstr(name).as_ptr());
    if p.is_null() { Err(format!("icall not found: {name}")) } else { Ok(p as usize) }
}

/// Byte offset of an instance field from the start of the object.
pub unsafe fn field_offset(klass: *mut c_void, name: &str) -> Result<usize, String> {
    let f = fns()?;
    let field = (api::get().il2cpp_get_field_from_name)(klass, cstr(name).as_ptr());
    if field.is_null() {
        return Err(format!("field not found: {name}"));
    }
    Ok((f.field_offset)(field))
}

pub unsafe fn read<T: Copy>(obj: Obj, offset: usize) -> T {
    std::ptr::read_unaligned((obj as *const u8).add(offset) as *const T)
}

/// Value of the first enum member found in `names` (int-backed enums).
/// The error lists the members the enum does have.
pub unsafe fn enum_value(enum_class: *mut c_void, names: &[&str]) -> Result<(i32, String), String> {
    let f = fns()?;
    let mut members = Vec::new();
    let mut iter: *mut c_void = std::ptr::null_mut();
    loop {
        let field = (f.class_fields)(enum_class, &mut iter);
        if field.is_null() {
            break;
        }
        // Enum members are the static fields; `value__` is the instance one.
        if (f.field_flags)(field) & 0x10 != 0 {
            members.push((lossy((f.field_name)(field)), field));
        }
    }
    for want in names {
        if let Some((name, field)) = members.iter().find(|(n, _)| n == want) {
            let mut v: i32 = 0;
            (f.field_static_value)(*field, &mut v as *mut i32 as *mut c_void);
            return Ok((v, name.clone()));
        }
    }
    let have: Vec<&str> = members.iter().map(|(n, _)| n.as_str()).collect();
    Err(format!("none of {names:?} in enum (has {})", have.join(", ")))
}

/// `System.Type` object for a class (for GetComponent, Resources.Load, ...).
pub unsafe fn type_object(klass: *mut c_void) -> Result<Obj, String> {
    let f = fns()?;
    let t = (f.class_type)(klass);
    let o = if t.is_null() { std::ptr::null_mut() } else { (f.type_object)(t) };
    if o.is_null() { Err("cannot get System.Type".to_owned()) } else { Ok(o) }
}

/// True if `obj` is an instance of `klass` (or a subclass).
pub unsafe fn is_instance(obj: Obj, klass: *mut c_void) -> bool {
    let Ok(f) = fns() else { return false };
    if obj.is_null() {
        return false;
    }
    let oc = (f.object_class)(obj);
    !oc.is_null() && (f.class_assignable)(klass, oc)
}

pub unsafe fn new_string(s: &str) -> Obj {
    (api::get().il2cpp_string_new)(cstr(s).as_ptr())
}

/// Keeps a managed object alive and lets us find it again.
pub unsafe fn gc_handle(obj: Obj) -> u32 {
    fns().map(|f| (f.gchandle_new)(obj, false)).unwrap_or(0)
}

pub unsafe fn gc_target(handle: u32) -> Obj {
    match fns() {
        Ok(f) if handle != 0 => (f.gchandle_target)(handle),
        _ => std::ptr::null_mut(),
    }
}

/// Elements of a managed reference-type array (`T[]`).
pub unsafe fn array_items<'a>(arr: Obj) -> &'a [Obj] {
    if arr.is_null() {
        return &[];
    }
    // Il2CppArray: klass, monitor, bounds, max_length, then the elements.
    let len = read::<usize>(arr, 0x18);
    if len > 4096 {
        return &[];
    }
    std::slice::from_raw_parts((arr as *const u8).add(0x20) as *const Obj, len)
}

/// Installs a typed hook; returns the trampoline (call it to run the original).
pub unsafe fn hook(code: usize, detour: usize) -> Result<usize, String> {
    let api = api::get();
    let interceptor = (api.hachimi_get_interceptor)((api.hachimi_instance)());
    let t = (api.interceptor_hook)(interceptor, code as *mut c_void, detour as *mut c_void) as usize;
    if t == 0 { Err("MinHook refused (already hooked?)".to_owned()) } else { Ok(t) }
}

/// Lists a class's fields and methods (own members, then each parent's).
pub unsafe fn describe_class(klass: *mut c_void) -> String {
    use std::fmt::Write as _;
    let Ok(f) = fns() else { return "?\n".to_owned() };
    let api = api::get();
    let mut out = String::new();
    let mut k = klass;
    for depth in 0..8 {
        if k.is_null() {
            break;
        }
        let ns = lossy((f.class_namespace)(k));
        let name = lossy((f.class_name)(k));
        if ns == "UnityEngine" || ns == "System" {
            let _ = writeln!(out, "  (inherits {ns}.{name})");
            break;
        }
        let _ = writeln!(out, "{}class {ns}.{name}", if depth == 0 { "" } else { "  inherits " });
        let mut iter: *mut c_void = std::ptr::null_mut();
        loop {
            let field = (f.class_fields)(k, &mut iter);
            if field.is_null() {
                break;
            }
            let is_static = (f.field_flags)(field) & 0x10 != 0;
            let _ = writeln!(
                out,
                "    field {}{} {}{}",
                if is_static { "static " } else { "" },
                sig::type_name_of((f.field_type)(field)),
                lossy((f.field_name)(field)),
                if is_static { String::new() } else { format!("  @0x{:x}", (f.field_offset)(field)) }
            );
        }
        let mut iter: *mut c_void = std::ptr::null_mut();
        loop {
            let m = (api.il2cpp_class_get_methods)(k, &mut iter);
            if m.is_null() {
                break;
            }
            let _ = writeln!(out, "    method {}  {}", lossy((f.method_name)(m)), sig::signature(m));
        }
        k = (f.class_parent)(k);
    }
    out
}
