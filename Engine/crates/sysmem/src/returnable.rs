//! Modified for Unveil (2026-10-17): per-buffer mapping, never a global allocator replacement.
//! The vector owns initialization and element destruction. This allocator owns only its storage.
#![allow(unsafe_code)]

use allocator_api2::alloc::{AllocError, Allocator, Global, Layout};
use std::ptr::NonNull;

/// Large buffers on Apple platforms bypass malloc's deferred reclamation. Small, over-aligned
/// and non-Apple allocations retain the standard allocator. The routing depends only on layout,
/// so a failed mapping returns an error instead of silently changing the deallocation route.
#[derive(Clone, Copy, Debug, Default)]
pub struct Returnable;

// SAFETY: allocations satisfy Layout, return exactly its size and pair with their original allocator.
// Copies are interchangeable; inherited grow/shrink allocate, copy and release through these methods.

unsafe impl Allocator for Returnable {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        #[cfg(target_vendor = "apple")]
        if mapped(layout) {
            // SAFETY: a fresh anonymous private read/write mapping owns no file or caller memory.
            // Layout is nonzero and at most isize::MAX; mmap rounds its extent to whole pages.
            let raw = unsafe {
                libc::mmap(std::ptr::null_mut(), layout.size(), libc::PROT_READ | libc::PROT_WRITE, libc::MAP_ANON | libc::MAP_PRIVATE, -1, 0)
            };
            if raw == libc::MAP_FAILED {
                return Err(AllocError);
            }
            let Some(ptr) = NonNull::new(raw.cast::<u8>()) else {
                // SAFETY: even a zero-address successful mapping must be released before rejection.
                unsafe { libc::munmap(raw, layout.size()) };
                return Err(AllocError);
            };
            return Ok(NonNull::slice_from_raw_parts(ptr, layout.size()));
        }
        Global.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        #[cfg(target_vendor = "apple")]
        if mapped(layout) {
            // SAFETY: the caller supplies a live allocation and fitting layout after all consumers end.
            // We report no excess capacity, so size and alignment select the original freeing route.

            unsafe { libc::munmap(ptr.as_ptr().cast(), layout.size()) };
            return;
        }
        // SAFETY: this layout was routed to Global at allocation and still belongs to it.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

#[cfg(target_vendor = "apple")]
fn mapped(layout: Layout) -> bool {
    static PAGE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    // SAFETY: sysconf reads a process constant. A failed query disables the mapped route.
    let page = *PAGE.get_or_init(|| usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(0));
    layout.size() >= 1 << 20 && layout.align() <= page
}
