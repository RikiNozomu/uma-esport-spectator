//! uma-esport-spectator — Hachimi Edge plugin (API v3).
//!
//! Stage 1: function tracer. Hooks every method of the classes listed in
//! `trace_targets.txt` and logs which ones run, so the race flow can be studied.

mod api;
mod gui;
mod stub;
mod sys;
mod targets;
mod tracer;

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use api::{Api, GetApiFn};

pub const TAG: &str = "uma-spectator";
const REQUIRED_API_VERSION: i32 = 3;

/// Entry point. Hachimi looks this symbol up after LoadLibrary.
#[no_mangle]
pub extern "C" fn hachimi_init_v3(get_api: Option<GetApiFn>, version: i32) -> i32 {
    let Some(get_api) = get_api else { return 0 };
    if version < REQUIRED_API_VERSION {
        return 0;
    }
    let Some(api) = Api::load(get_api) else { return 0 };
    let api = api::set(api);

    unsafe {
        (api.hachimi_register_on_game_initialized)(Some(on_game_initialized), std::ptr::null_mut());
        (api.gui_register_menu_section)(Some(gui::menu_section), std::ptr::null_mut());
    }
    api.info("plugin loaded");
    1
}

/// Called once by Hachimi when IL2CPP and the game assemblies are ready.
unsafe extern "C" fn on_game_initialized(_userdata: *mut c_void) {
    tracer::GAME_READY.store(true, Ordering::Release);
    api::get().info("game initialized, tracer is ready");
}
