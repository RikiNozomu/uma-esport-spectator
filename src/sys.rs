//! The few OS calls we need, declared by hand so the crate has no dependencies.

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16, month: u16, day_of_week: u16, day: u16,
        hour: u16, minute: u16, second: u16, millis: u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn VirtualAlloc(addr: *mut c_void, size: usize, alloc_type: u32, protect: u32) -> *mut c_void;
        fn GetCurrentThreadId() -> u32;
        fn GetLocalTime(out: *mut SystemTime);
    }

    const MEM_COMMIT_RESERVE: u32 = 0x1000 | 0x2000;
    const PAGE_EXECUTE_READWRITE: u32 = 0x40;

    pub fn alloc_exec(size: usize) -> *mut u8 {
        unsafe { VirtualAlloc(std::ptr::null_mut(), size, MEM_COMMIT_RESERVE, PAGE_EXECUTE_READWRITE) as *mut u8 }
    }

    pub fn thread_id() -> u32 {
        unsafe { GetCurrentThreadId() }
    }

    /// Local time as `YYYYMMDD_HHMMSS`, for file names.
    pub fn timestamp() -> String {
        let mut t = SystemTime::default();
        unsafe { GetLocalTime(&mut t) };
        format!("{:04}{:02}{:02}_{:02}{:02}{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
    }

    // ------------------------------------------------------------ game window

    /// Window procedure (`WNDPROC`), handles as isize.
    pub type WndProc = unsafe extern "system" fn(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcessId() -> u32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(cb: unsafe extern "system" fn(hwnd: isize, lparam: isize) -> i32, lparam: isize) -> i32;
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        fn GetClassNameW(hwnd: isize, buf: *mut u16, max: i32) -> i32;
        fn IsWindowVisible(hwnd: isize) -> i32;
        fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
        fn CallWindowProcW(prev: isize, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn GetClientRect(hwnd: isize, rect: *mut [i32; 4]) -> i32;
        fn SetCapture(hwnd: isize) -> isize;
        fn ReleaseCapture() -> i32;
    }

    const GWLP_WNDPROC: i32 = -4;

    unsafe extern "system" fn find_cb(hwnd: isize, out: isize) -> i32 {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != GetCurrentProcessId() || IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let mut buf = [0u16; 64];
        let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        if String::from_utf16_lossy(&buf[..n]) == "UnityWndClass" {
            *(out as *mut isize) = hwnd;
            return 0;
        }
        1
    }

    /// The game's main window (Unity's, in this process), if it exists yet.
    pub fn game_window() -> Option<isize> {
        let mut found: isize = 0;
        unsafe { EnumWindows(find_cb, &mut found as *mut isize as isize) };
        (found != 0).then_some(found)
    }

    /// Installs `proc` as the window procedure; returns the previous one (0 = failed).
    pub fn subclass(hwnd: isize, proc: WndProc) -> isize {
        unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc as usize as isize) }
    }

    pub fn call_window_proc(prev: isize, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
        unsafe { CallWindowProcW(prev, hwnd, msg, wparam, lparam) }
    }

    pub fn def_window_proc(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// Client area size in pixels.
    pub fn client_size(hwnd: isize) -> Option<(i32, i32)> {
        let mut r = [0i32; 4];
        let ok = unsafe { GetClientRect(hwnd, &mut r) } != 0;
        (ok && r[2] > r[0] && r[3] > r[1]).then(|| (r[2] - r[0], r[3] - r[1]))
    }

    /// Keeps mouse messages coming while a drag leaves the window.
    pub fn capture_mouse(hwnd: isize, on: bool) {
        unsafe {
            if on {
                SetCapture(hwnd);
            } else {
                ReleaseCapture();
            }
        }
    }
}

// Non-Windows build exists only so `cargo test` can exercise the stub on Linux.
#[cfg(not(windows))]
mod imp {
    use std::ffi::c_void;

    extern "C" {
        fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut c_void;
    }

    pub fn alloc_exec(size: usize) -> *mut u8 {
        // PROT_READ|WRITE|EXEC, MAP_PRIVATE|MAP_ANONYMOUS
        let p = unsafe { mmap(std::ptr::null_mut(), size, 7, 0x22, -1, 0) };
        if p as isize == -1 { std::ptr::null_mut() } else { p as *mut u8 }
    }

    pub fn thread_id() -> u32 { 0 }

    pub fn timestamp() -> String {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        format!("{secs}")
    }
}

pub use imp::*;
