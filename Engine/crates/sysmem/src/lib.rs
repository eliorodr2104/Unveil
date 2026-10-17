//! Return disposable storage and free allocator pages to the operating system.
//!
//! `Returnable` gives selected large buffers anonymous mappings on Apple platforms and unmaps
//! them when their owner drops. Other layouts and platforms use the system allocator. This is
//! a per-buffer choice, not a replacement for the process allocator.
//!
//! `release_free_memory` separately asks the macOS allocator to return already-free pages.
//! That best-effort request is a no-op on other platforms, including iOS. Unsafe OS operations
//! stay in this crate behind safe storage and reclamation APIs.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

// Modified for Unveil (2026-10-17): selected decode buffers have their own returnable backing.
mod returnable;
pub use returnable::Returnable;
pub type ReturnableVec<T> = allocator_api2::vec::Vec<T, Returnable>;

/// Return free allocator pages to the system. Returns the number of bytes released (always 0 on
/// platforms where freed large blocks are already unmapped). Safe to call at any time, from any
/// thread.
pub fn release_free_memory() -> usize {
    #[cfg(target_os = "macos")]
    {
        macos::release()
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    unsafe extern "C" {
        /// libSystem malloc: release free memory of `zone` (all zones when null) back to the
        /// system, up to `goal` bytes (0 = as much as possible). Returns the bytes released.
        fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
    }

    pub fn release() -> usize {
        // SAFETY: `malloc_zone_pressure_relief` is a documented, thread-safe libSystem function
        // (malloc/malloc.h). A null zone means "every zone" and a goal of 0 means "as much as
        // possible". It only returns already-free pages to the system: it reads and writes no
        // memory we own, takes no pointers we must keep alive, and has no failure mode beyond
        // releasing 0 bytes.
        unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) }
    }
}

#[cfg(test)]
mod tests {
    use super::release_free_memory;

    #[test]
    fn releasing_with_nothing_freed_is_harmless() {
        // Twice in a row: the second call usually has nothing left to release.
        let _ = release_free_memory();
        let _ = release_free_memory();
    }

    #[test]
    fn freed_large_blocks_can_be_released_and_memory_stays_usable() {
        // Allocate and free well over the allocator's large-block threshold, then release.
        let blocks: Vec<Vec<u8>> = (0..8).map(|i| vec![i as u8; 16 << 20]).collect();
        let sum: u64 = blocks.iter().map(|b| u64::from(b[b.len() - 1])).sum();
        assert_eq!(sum, (0..8u64).sum());
        drop(blocks);
        let released = release_free_memory();
        if cfg!(not(target_os = "macos")) {
            assert_eq!(released, 0);
        }
        // The allocator must still work normally afterwards.
        let again = vec![7u8; 16 << 20];
        assert_eq!(again[again.len() - 1], 7);
    }

    #[test]
    fn concurrent_calls_are_safe() {
        let handles: Vec<_> = (0..4).map(|_| std::thread::spawn(release_free_memory)).collect();
        for h in handles {
            assert!(h.join().is_ok());
        }
    }
}
