//! uma-esport-spectator — Hachimi Edge plugin (API v3).
//!
//! - Function tracer: hooks the methods listed in `trace_targets.txt` and logs
//!   which ones run, to study the race flow.
//! - Live leaderboard overlay during races (`leaderboard`).
//! - Race toggles: skip skill cut-ins / skill popups (`patches`), race clock
//!   (`clock`), hide the race UI while running (`race_ui`).

mod api;
mod clock;
mod gui;
pub mod leaderboard;
pub mod race;
mod il2cpp;
mod label;
mod patches;
mod race_ui;
mod settings;
pub mod seek;
mod sig;
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
        // On Windows, Hachimi runs plugin init *after* it has already fired the
        // game-initialized callbacks (see `on_hooking_finished` in Hachimi-Edge's
        // src/core/hachimi.rs), and only fires them again when ui_scale != 1.0.
        // So check whether IL2CPP is already usable instead of only waiting.
        if game_assembly_loaded(api) {
            mark_ready("game already initialized at plugin load, tracer is ready");
        } else {
            (api.hachimi_register_on_game_initialized)(Some(on_game_initialized), std::ptr::null_mut());
        }
        (api.gui_register_menu_section)(Some(gui::menu_section), std::ptr::null_mut());
        leaderboard::load_settings();
        #[cfg(windows)]
        if !(api.hachimi_register_present_callback)(Some(leaderboard::render_d3d11::on_present), std::ptr::null_mut()) {
            api.warn("present callback unavailable: leaderboard overlay disabled");
        }
    }
    api.info("plugin loaded");
    1
}

/// True once IL2CPP's domain can open the game assembly.
unsafe fn game_assembly_loaded(api: &Api) -> bool {
    !(api.il2cpp_get_assembly_image)(c"umamusume.dll".as_ptr()).is_null()
}

fn mark_ready(msg: &str) {
    if !tracer::GAME_READY.swap(true, Ordering::AcqRel) {
        api::get().info(msg);
        settings::load();
        patches::install();
        race_ui::install();
        clock::install();
        seek::game::install();
        #[cfg(windows)]
        seek::input::install();
    }
}

/// Called once by Hachimi when IL2CPP and the game assemblies are ready.
unsafe extern "C" fn on_game_initialized(_userdata: *mut c_void) {
    mark_ready("game initialized, tracer is ready");
}
