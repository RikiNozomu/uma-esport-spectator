//! The plugin's section in the Hachimi menu.

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use crate::api::{self, cstr};
use crate::{cutin, leaderboard, tracer};

pub extern "C" fn menu_section(ui: *mut c_void, _userdata: *mut c_void) {
    let api = api::get();
    let text = |s: &str| cstr(s);
    unsafe {
        (api.gui_ui_heading)(ui, text("Race").as_ptr());
        let mut no_cutins = cutin::is_disabled();
        if (api.gui_ui_checkbox)(ui, text("Disable skill cut-ins (all horses)").as_ptr(), &mut no_cutins) {
            cutin::set_disabled(no_cutins);
        }
        (api.gui_ui_small)(ui, text(cutin::status()).as_ptr());
        (api.gui_ui_separator)(ui);

        (api.gui_ui_heading)(ui, text("Leaderboard overlay").as_ptr());
        let mut on = leaderboard::is_enabled();
        if (api.gui_ui_checkbox)(ui, text("Show during races").as_ptr(), &mut on) {
            leaderboard::set_enabled(on);
        }
        let mut preview = leaderboard::is_preview();
        if (api.gui_ui_checkbox)(ui, text("Preview with sample data").as_ptr(), &mut preview) {
            leaderboard::set_preview(preview);
        }
        let mut view = leaderboard::view();
        let mut changed = (api.gui_ui_slider_f32)(ui, text("Size").as_ptr(), &mut view.scale, 0.4, 2.5);
        changed |= (api.gui_ui_slider_f32)(ui, text("Height").as_ptr(), &mut view.y, 0.1, 1.0);
        if changed {
            leaderboard::set_view(view);
        }
        (api.gui_ui_small)(ui, text(&leaderboard::status()).as_ptr());
        (api.gui_ui_separator)(ui);

        (api.gui_ui_heading)(ui, text("Function trace").as_ptr());

        if !tracer::GAME_READY.load(Ordering::Acquire) {
            (api.gui_ui_label)(ui, text("Waiting for the game to finish loading...").as_ptr());
            return;
        }

        let mut record = tracer::wants_recording();
        if (api.gui_ui_checkbox)(ui, text("Record function calls").as_ptr(), &mut record) {
            tracer::set_recording(record);
        }

        let mut every = tracer::EVERY_CALL.load(Ordering::Relaxed);
        if (api.gui_ui_checkbox)(ui, text("Log every call (heavy)").as_ptr(), &mut every) {
            tracer::EVERY_CALL.store(every, Ordering::Relaxed);
        }

        (api.gui_ui_small)(ui, text(&tracer::status()).as_ptr());
        (api.gui_ui_separator)(ui);
    }
}
