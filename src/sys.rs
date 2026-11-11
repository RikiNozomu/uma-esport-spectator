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

    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(vk: i32) -> i16;
        fn GetForegroundWindow() -> *mut c_void;
        fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
    }

    /// The key is held down right now.
    pub fn key_down(vk: i32) -> bool {
        unsafe { GetAsyncKeyState(vk) as u16 & 0x8000 != 0 }
    }

    /// The foreground window belongs to this process (the game).
    pub fn game_has_focus() -> bool {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_null() {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            pid == GetCurrentProcessId()
        }
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcessId() -> u32;
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
