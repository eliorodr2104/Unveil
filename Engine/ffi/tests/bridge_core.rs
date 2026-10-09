mod common;
use common::*;
use std::ffi::CString;
use unveil_ffi::*;

#[test]
fn abi_version_matches_header() {
    assert_eq!(uv_abi_version(), 1);
}

#[test]
fn session_opens_and_frees() {
    let _s = TestSession::new();
}

#[test]
fn null_data_dir_is_an_invalid_argument() {
    // SAFETY: NULL data_dir is the case under test; the call rejects it before any read.
    let raw = unsafe { uv_session_new(std::ptr::null(), 0) };
    assert!(raw.is_null());
    assert!(last_error().contains("data_dir"), "got: {}", last_error());
}

#[test]
fn string_free_accepts_null() {
    // SAFETY: NULL is a documented no-op for uv_string_free.
    unsafe { uv_string_free(std::ptr::null_mut()) };
}

#[cfg(feature = "test-hooks")]
#[test]
fn panic_becomes_a_status_and_the_session_survives() {
    let s = TestSession::new();
    // SAFETY: uv_test_panic never dereferences the session; s.raw is live anyway.
    let status = unsafe { uv_test_panic(s.raw) };
    assert_eq!(status, UVStatus::UV_ERR_PANIC as i32);
    assert!(last_error().contains("test panic"), "got: {}", last_error());
    // The session still answers after a caught panic.
    let cmd = CString::new("library.info").unwrap();
    let mut out = std::ptr::null_mut();
    // SAFETY: s.raw is live; cmd and out outlive the call; NULL params means {}.
    let status = unsafe { uv_execute(s.raw, cmd.as_ptr(), std::ptr::null(), &mut out) };
    assert_eq!(status, 0, "{}", last_error());
    // SAFETY: out came from uv_execute (or is NULL) and is freed once.
    unsafe { uv_string_free(out) };
}
