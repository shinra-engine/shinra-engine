//! On the wasm route a module allocates out of the engine's heap.
//!
//! The engine and every module it loads share one linear memory — that is what
//! `--import-memory` buys, and it is why a pointer a module returns is a
//! pointer the engine can follow. A shared address space can only have one
//! allocator: two `dlmalloc`s each believing they own everything above their
//! own `__heap_base` would hand out the same bytes twice.
//!
//! So the module does not bring one. It imports the engine's, which is the
//! same shape as importing the memory itself, and for the same reason.
//!
//! Behind a feature because a module built as a plain standalone `.wasm` — for
//! a compile check, say — has no engine to import from and should keep its own
//! allocator.

use core::alloc::{GlobalAlloc, Layout};

extern "C" {
    /// Provided by the engine, which owns the memory this carves from.
    fn se_host_alloc(size: usize, align: usize) -> *mut u8;
    fn se_host_dealloc(ptr: *mut u8, size: usize, align: usize);
    fn se_host_realloc(ptr: *mut u8, size: usize, align: usize, new_size: usize) -> *mut u8;
}

pub struct HostHeap;

unsafe impl GlobalAlloc for HostHeap {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        se_host_alloc(l.size(), l.align())
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        se_host_dealloc(p, l.size(), l.align())
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
        se_host_realloc(p, l.size(), l.align(), new_size)
    }
}

#[global_allocator]
static HEAP: HostHeap = HostHeap;
