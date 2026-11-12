//! Mouse input for the seek bar (Windows): subclasses the game window.
//!
//! Hachimi hooks `SetWindowLongPtrW` on the game window and chains whatever
//! is installed behind its own window procedure, so Hachimi's GUI still sees
//! input first; without that hook we simply sit in front of it. Messages that
//! hit the bar (and every mouse message during a drag) are swallowed so the
//! game never sees them; everything else is passed on untouched.
//!
//! Runs on the window thread: no il2cpp calls, no panics (caught).

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use egui::Pos2;

use crate::{api, sys};

const WM_KILLFOCUS: u32 = 0x0008;
const WM_ACTIVATEAPP: u32 = 0x001C;
const WM_INPUT: u32 = 0x00FF;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_MOUSELAST: u32 = 0x020E;
const WM_CAPTURECHANGED: u32 = 0x0215;
const WM_POINTERFIRST: u32 = 0x0245;
const WM_POINTERLAST: u32 = 0x0257;

static HWND: AtomicIsize = AtomicIsize::new(0);
static PREV: AtomicIsize = AtomicIsize::new(0);
static ERROR: Mutex<Option<String>> = Mutex::new(None);

pub fn error() -> Option<String> {
    let e = ERROR.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if e.is_none() && PREV.load(Ordering::Acquire) == 0 {
        return Some("waiting for the game".to_owned());
    }
    e
}

fn set_error(e: Option<String>) {
    *ERROR.lock().unwrap_or_else(|e| e.into_inner()) = e;
}

/// At game ready. Safe to call again (does nothing once installed).
pub fn install() {
    if HWND.load(Ordering::Acquire) != 0 {
        return;
    }
    let Some(hwnd) = sys::game_window() else {
        set_error(Some("game window not found".to_owned()));
        api::get().warn("seek bar: game window not found, no mouse input");
        return;
    };
    HWND.store(hwnd, Ordering::Release);
    let prev = sys::subclass(hwnd, wnd_proc);
    if prev == 0 {
        HWND.store(0, Ordering::Release);
        set_error(Some("cannot subclass the game window".to_owned()));
        api::get().warn("seek bar: cannot subclass the game window, no mouse input");
        return;
    }
    PREV.store(prev, Ordering::Release);
    set_error(None);
    api::get().info("seek bar: mouse input ready");
}

/// Client coordinates in `lparam` -> back-buffer pixels.
fn to_buffer(hwnd: isize, lparam: isize) -> Option<Pos2> {
    let x = (lparam & 0xffff) as u16 as i16 as f32;
    let y = ((lparam >> 16) & 0xffff) as u16 as i16 as f32;
    let (bw, bh) = super::buffer_size()?;
    let (cw, ch) = sys::client_size(hwnd)?;
    Some(Pos2::new(x * bw as f32 / cw as f32, y * bh as f32 / ch as f32))
}

/// True = swallow the message.
fn handle(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> bool {
    match msg {
        WM_MOUSEMOVE => super::on_move(to_buffer(hwnd, lparam)),
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let Some(p) = to_buffer(hwnd, lparam) else { return false };
            let hit = super::on_down(p);
            if super::dragging() {
                sys::capture_mouse(hwnd, true);
            }
            hit
        }
        WM_LBUTTONUP => {
            let was_dragging = super::dragging();
            // Without a layout (the race ended mid-drag) the drag still ends.
            let swallow = super::on_up(to_buffer(hwnd, lparam).unwrap_or(Pos2::new(-1.0, -1.0)));
            if was_dragging {
                sys::capture_mouse(hwnd, false);
            }
            swallow
        }
        WM_CAPTURECHANGED | WM_KILLFOCUS => {
            super::on_cancel();
            false
        }
        WM_ACTIVATEAPP if wparam == 0 => {
            super::on_cancel();
            false
        }
        // Other buttons and the wheel: not for the game while on the bar.
        WM_MOUSEMOVE..=WM_MOUSELAST => {
            super::dragging() || to_buffer(hwnd, lparam).is_some_and(super::over_bar)
        }
        // Raw and pointer input describe the same mouse: hold them during a drag.
        WM_INPUT | WM_POINTERFIRST..=WM_POINTERLAST => super::dragging(),
        _ => false,
    }
}

unsafe extern "system" fn wnd_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    let swallow = catch_unwind(AssertUnwindSafe(|| handle(hwnd, msg, wparam, lparam))).unwrap_or(false);
    if swallow {
        return 0;
    }
    match PREV.load(Ordering::Acquire) {
        0 => sys::def_window_proc(hwnd, msg, wparam, lparam),
        prev => sys::call_window_proc(prev, hwnd, msg, wparam, lparam),
    }
}
