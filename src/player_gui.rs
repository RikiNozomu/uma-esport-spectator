//! "Race player" menu section and window. Render thread: only sends
//! commands; `player::tick` applies them on the main thread.

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::api::{self, cstr};
use crate::keybinds::{self, BINDINGS};
use crate::player::{self, Command, REWIND_RATES, SPEEDS, STEP};

static WINDOW_ID: OnceLock<i32> = OnceLock::new();
/// When the window was last drawn: Hachimi closes it (X button) without
/// telling us, so "drawn recently" is how we know it is open.
static LAST_DRAWN: Mutex<Option<Instant>> = Mutex::new(None);

#[cfg_attr(not(windows), allow(dead_code))]
fn window_open() -> bool {
    LAST_DRAWN.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|t| t.elapsed() < Duration::from_millis(300))
}

/// For the open/close keybind (render thread).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn toggle_window() {
    if window_open() {
        if let Some(id) = WINDOW_ID.get() {
            unsafe { (api::get().gui_close_window)(*id) };
        }
        *LAST_DRAWN.lock().unwrap_or_else(|e| e.into_inner()) = None;
    } else {
        open_window();
    }
}

/// In the plugin's menu section.
pub fn menu_section(ui: *mut c_void) {
    let api = api::get();
    unsafe {
        (api.gui_ui_heading)(ui, cstr("Race player").as_ptr());
        if (api.gui_ui_button)(ui, cstr("Open race player").as_ptr()) {
            open_window();
        }
        let mut keys = player::shortcuts_enabled();
        let label = format!(
            "Keybinds (change them in the player window); open/close: {}",
            keybinds::key_name(keybinds::key_of(BINDINGS.len() - 1))
        );
        if (api.gui_ui_checkbox)(ui, cstr(&label).as_ptr(), &mut keys) {
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
    *LAST_DRAWN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    let api = api::get();
    unsafe {
        (api.gui_ui_label)(ui, cstr(&player::status()).as_ptr());
        match player::position() {
            Some((t, end)) => controls(ui, t, end),
            None => {
                (api.gui_ui_small)(ui, cstr("Controls work from the start of a race until the result board.").as_ptr());
            }
        }
        (api.gui_ui_separator)(ui);
        keybind_list(ui);
    }
}

unsafe fn controls(ui: *mut c_void, t: f32, end: f32) {
    let api = api::get();
    {
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

/// One button per action showing its key; click it, then press the new key.
unsafe fn keybind_list(ui: *mut c_void) {
    let api = api::get();
    (api.gui_ui_label)(ui, cstr("Keybinds (only while the game window has focus)").as_ptr());
    let capturing = keybinds::capturing();
    for (i, b) in BINDINGS.iter().enumerate() {
        let text = if capturing == Some(i) {
            format!("{}: press a key... (Esc cancel, Backspace clear)", b.label)
        } else {
            format!("{}: {}", b.label, keybinds::key_name(keybinds::key_of(i)))
        };
        if (api.gui_ui_button)(ui, cstr(&text).as_ptr()) {
            keybinds::start_capture(i);
        }
    }
    if (api.gui_ui_button)(ui, cstr("Reset keybinds to defaults").as_ptr()) {
        keybinds::reset_defaults();
    }
    if !player::shortcuts_enabled() {
        (api.gui_ui_small)(ui, cstr("Keybinds are turned off in the plugin menu.").as_ptr());
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
