//! The plugin's section in the Hachimi menu.

use std::ffi::c_void;
use std::sync::atomic::Ordering;

use crate::api::{self, cstr};
use crate::tracer;

pub extern "C" fn menu_section(ui: *mut c_void, _userdata: *mut c_void) {
    let api = api::get();
    let text = |s: &str| cstr(s);
    unsafe {
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
