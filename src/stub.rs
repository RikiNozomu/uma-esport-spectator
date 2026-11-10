//! Generic entry stubs (x86-64, Microsoft calling convention).
//!
//! We hook methods without knowing their signatures, so a normal Rust hook
//! function is not possible. Instead every hooked method gets a tiny piece of
//! machine code that:
//!   1. saves the argument registers (rcx, rdx, r8, r9, xmm0-3),
//!   2. calls `enter(index)`, which logs and returns the trampoline address,
//!   3. restores the registers and jumps to the trampoline (the original code).
//!
//! The stub is gone from the stack before the original method runs, so return
//! values, stack arguments and exceptions are untouched.

use std::sync::Mutex;

use crate::sys;

/// `enter(index) -> address to jump to`. Must never return 0.
pub type EnterFn = extern "win64" fn(index: u32) -> usize;

const CHUNK_SIZE: usize = 64 * 1024;

struct Arena {
    cur: *mut u8,
    left: usize,
}
unsafe impl Send for Arena {}

static ARENA: Mutex<Arena> = Mutex::new(Arena { cur: std::ptr::null_mut(), left: 0 });

/// Writes a stub for `index` and returns its address (never freed), or None
/// if executable memory could not be allocated.
pub fn build(index: u32, enter: EnterFn) -> Option<*mut u8> {
    let code = assemble(index, enter as usize);
    let mut arena = ARENA.lock().unwrap_or_else(|e| e.into_inner());
    if arena.left < code.len() {
        let p = sys::alloc_exec(CHUNK_SIZE);
        if p.is_null() {
            return None;
        }
        arena.cur = p;
        arena.left = CHUNK_SIZE;
    }
    let dst = arena.cur;
    unsafe {
        std::ptr::copy_nonoverlapping(code.as_ptr(), dst, code.len());
        // Keep stubs 16-byte aligned.
        let used = (code.len() + 15) & !15;
        arena.cur = dst.add(used);
        arena.left -= used;
    }
    Some(dst)
}

fn assemble(index: u32, enter: usize) -> Vec<u8> {
    let mut c: Vec<u8> = Vec::with_capacity(96);
    // On entry rsp % 16 == 8. Four pushes (32) + 0x68 makes it 16-aligned
    // for the call, with 0x20 bytes of shadow space at the bottom.
    c.extend_from_slice(&[0x51]);                               // push rcx
    c.extend_from_slice(&[0x52]);                               // push rdx
    c.extend_from_slice(&[0x41, 0x50]);                         // push r8
    c.extend_from_slice(&[0x41, 0x51]);                         // push r9
    c.extend_from_slice(&[0x48, 0x83, 0xEC, 0x68]);             // sub rsp, 0x68
    c.extend_from_slice(&[0xF3, 0x0F, 0x7F, 0x44, 0x24, 0x20]); // movdqu [rsp+0x20], xmm0
    c.extend_from_slice(&[0xF3, 0x0F, 0x7F, 0x4C, 0x24, 0x30]); // movdqu [rsp+0x30], xmm1
    c.extend_from_slice(&[0xF3, 0x0F, 0x7F, 0x54, 0x24, 0x40]); // movdqu [rsp+0x40], xmm2
    c.extend_from_slice(&[0xF3, 0x0F, 0x7F, 0x5C, 0x24, 0x50]); // movdqu [rsp+0x50], xmm3
    c.push(0xB9);                                               // mov ecx, index
    c.extend_from_slice(&index.to_le_bytes());
    c.extend_from_slice(&[0x48, 0xB8]);                         // mov rax, enter
    c.extend_from_slice(&(enter as u64).to_le_bytes());
    c.extend_from_slice(&[0xFF, 0xD0]);                         // call rax
    c.extend_from_slice(&[0xF3, 0x0F, 0x6F, 0x44, 0x24, 0x20]); // movdqu xmm0, [rsp+0x20]
    c.extend_from_slice(&[0xF3, 0x0F, 0x6F, 0x4C, 0x24, 0x30]); // movdqu xmm1, [rsp+0x30]
    c.extend_from_slice(&[0xF3, 0x0F, 0x6F, 0x54, 0x24, 0x40]); // movdqu xmm2, [rsp+0x40]
    c.extend_from_slice(&[0xF3, 0x0F, 0x6F, 0x5C, 0x24, 0x50]); // movdqu xmm3, [rsp+0x50]
    c.extend_from_slice(&[0x48, 0x83, 0xC4, 0x68]);             // add rsp, 0x68
    c.extend_from_slice(&[0x41, 0x59]);                         // pop r9
    c.extend_from_slice(&[0x41, 0x58]);                         // pop r8
    c.extend_from_slice(&[0x5A]);                               // pop rdx
    c.extend_from_slice(&[0x59]);                               // pop rcx
    c.extend_from_slice(&[0xFF, 0xE0]);                         // jmp rax
    c
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    static SEEN: AtomicU32 = AtomicU32::new(0);
    static TARGET: AtomicUsize = AtomicUsize::new(0);

    // Clobbers every volatile register the stub must protect.
    extern "win64" fn enter(index: u32) -> usize {
        SEEN.store(index, Ordering::SeqCst);
        let v: Vec<f64> = (0..64).map(|i| (i as f64).sqrt() * index as f64).collect();
        std::hint::black_box(v);
        TARGET.load(Ordering::SeqCst)
    }

    // Register ints, register floats and stack arguments all at once.
    extern "win64" fn original(a: u64, b: f64, c: u64, d: f32, e: u64, f: f64, g: u64) -> f64 {
        a as f64 + b + c as f64 + d as f64 + e as f64 + f + g as f64
    }

    #[test]
    fn stub_preserves_arguments_and_return_value() {
        TARGET.store(original as *const () as usize, Ordering::SeqCst);
        let stub = build(1234, enter).expect("exec memory");
        let via_stub: extern "win64" fn(u64, f64, u64, f32, u64, f64, u64) -> f64 =
            unsafe { std::mem::transmute(stub) };
        let got = via_stub(1, 2.5, 3, 4.25, 5, 6.125, 7);
        assert_eq!(got, original(1, 2.5, 3, 4.25, 5, 6.125, 7));
        assert_eq!(SEEN.load(Ordering::SeqCst), 1234);
    }

    #[test]
    fn many_stubs_span_chunks() {
        TARGET.store(original as *const () as usize, Ordering::SeqCst);
        let mut last = std::ptr::null_mut();
        for i in 0..2000 {
            last = build(i, enter).expect("exec memory");
            assert_eq!(last as usize % 16, 0);
        }
        let f: extern "win64" fn(u64, f64, u64, f32, u64, f64, u64) -> f64 =
            unsafe { std::mem::transmute(last) };
        assert_eq!(f(1, 1.0, 1, 1.0, 1, 1.0, 1), 7.0);
    }
}
