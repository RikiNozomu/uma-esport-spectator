//! "Race player" menu section and window. Render thread: only sends
//! commands; `player::tick` applies them on the main thread.

use std::ffi::c_void;
use std::sync::OnceLock;

use crate::api::{self, cstr};
use crate::player::{self, Command, REWIND_RATES, SPEEDS, STEP};

static WINDOW_ID: OnceLock<i32> = OnceLock::new();

/// In the plugin's menu section.
pub fn menu_section(ui: *mut c_void) {
    let api = api::get();
    unsafe {
        (api.gui_ui_heading)(ui, cstr("Race player").as_ptr());
        if (api.gui_ui_button)(ui, cstr("Open race player").as_ptr()) {
            open_window();
        }
        let mut keys = player::shortcuts_enabled();
        let label = cstr("Keyboard shortcuts: Space play/pause, Left/Right -/+5 s, Down/Up slower/faster, R rewind");
        if (api.gui_ui_checkbox)(ui, label.as_ptr(), &mut keys) {
            player::set_shortcuts(keys);
        }
        (api.gui_ui_small)(ui, cstr(&player::status()).as_ptr());
        (api.gui_ui_separator)(ui);
    }
}

pub fn open_window() {
    let api = api::get();
    unsafe {
        let id = *WINDOW_ID.get_or_init(|| (api.gui_new_window_id)());
        (api.gui_show_window)(id, cstr("Race player").as_ptr(), Some(window), None, std::ptr::null_mut());
    }
}

extern "C" fn window(ui: *mut c_void, _userdata: *mut c_void) {
    let api = api::get();
    unsafe {
        (api.gui_ui_label)(ui, cstr(&player::status()).as_ptr());
        let Some((t, end)) = player::position() else {
            (api.gui_ui_small)(ui, cstr("Controls work from the start of a race until the result board.").as_ptr());
            return;
        };
        // Seek bar: dragging it seeks.
        let mut pos = t;
        if (api.gui_ui_slider_f32)(ui, cstr("s").as_ptr(), &mut pos, 0.0, end) && (pos - t).abs() > 0.01 {
            player::send(Command::SeekTo(pos));
        }
        (api.gui_ui_horizontal)(ui, Some(transport_row), std::ptr::null_mut());
        (api.gui_ui_horizontal)(ui, Some(speed_row), std::ptr::null_mut());
        (api.gui_ui_small)(ui, cstr("Tip: turn on \"Disable skill cut-ins\" for smoother seeking.").as_ptr());
    }
}

/// -5 s, rewind, play/pause, faster, +5 s.
extern "C" fn transport_row(ui: *mut c_void, _userdata: *mut c_void) {
    let api = api::get();
    let c = player::current();
    let rewind = match c.rewind {
        Some(i) if i + 1 < REWIND_RATES.len() => format!("<< {}x", REWIND_RATES[i + 1]),
        Some(_) => "<< stop".to_owned(),
        None => format!("<< {}x", REWIND_RATES[0]),
    };
    let play = if c.paused || c.rewind.is_some() { "Play" } else { "Pause" };
    let buttons: [(String, Command); 5] = [
        (format!("-{STEP:.0} s"), Command::SeekBy(-STEP)),
        (rewind, Command::Rewind),
        (play.to_owned(), Command::TogglePause),
        (">> faster".to_owned(), Command::Faster),
        (format!("+{STEP:.0} s"), Command::SeekBy(STEP)),
    ];
    for (label, cmd) in buttons {
        if unsafe { (api.gui_ui_button)(ui, cstr(&label).as_ptr()) } {
            player::send(cmd);
        }
    }
}

/// One button per speed; the current one in brackets.
extern "C" fn speed_row(ui: *mut c_void, _userdata: *mut c_void) {
    let api = api::get();
    let c = player::current();
    for (i, speed) in SPEEDS.iter().enumerate() {
        let label = if i == c.speed && !c.paused && c.rewind.is_none() { format!("[{speed}x]") } else { format!("{speed}x") };
        if unsafe { (api.gui_ui_button)(ui, cstr(&label).as_ptr()) } {
            player::send(Command::Speed(i));
        }
    }
}
