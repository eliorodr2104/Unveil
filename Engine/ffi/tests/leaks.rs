mod common;
use common::*;
use std::ffi::CString;
use unveil_ffi::*;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[test]
fn execute_results_and_errors_do_not_leak() {
    let s = TestSession::new();
    let warm = |n: usize| {
        for _ in 0..n {
            let cmd = CString::new("library.info").unwrap();
            let mut out = std::ptr::null_mut();
            // SAFETY: s.raw is live; cmd and out outlive the call; NULL params means {}.
            unsafe { uv_execute(s.raw, cmd.as_ptr(), std::ptr::null(), &mut out) };
            // SAFETY: out came from the uv_execute above (or is NULL) and is freed once.
            unsafe { uv_string_free(out) };
            let bad = CString::new("no.such.command").unwrap();
            // SAFETY: s.raw is live; bad and out outlive the call; on failure out is set to NULL.
            unsafe { uv_execute(s.raw, bad.as_ptr(), std::ptr::null(), &mut out) };
            let _ = last_error();
        }
    };
    warm(10);
    let _profiler = dhat::Profiler::builder().testing().build();
    let before = dhat::HeapStats::get();
    warm(1000);
    let after = dhat::HeapStats::get();
    // A few blocks of slack for lazily grown caches; a leak would add >= 1000.
    assert!(
        after.curr_blocks <= before.curr_blocks + 16,
        "{before:?} -> {after:?}"
    );
}
