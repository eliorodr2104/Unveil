//! The per-thread last error, like errno: it holds until the next uv_* call on the same thread.

use std::cell::RefCell;
use std::ffi::{CString, c_char};

thread_local! {
    static LAST: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// set records the message for this thread. Interior NUL bytes become '?' so C sees it whole.
pub fn set(message: &str) {
    let c = CString::new(message.replace('\0', "?")).expect("NUL bytes were replaced");
    LAST.with(|l| *l.borrow_mut() = Some(c));
}

pub fn clear() {
    LAST.with(|l| *l.borrow_mut() = None);
}

/// uv_last_error returns the message of the last failed uv_* call on this thread, or NULL.
/// The pointer is valid until the next uv_* call on the same thread. It skips the panic guard
/// on purpose: the guard clears the error, and reading it must not.
#[unsafe(no_mangle)]
pub extern "C" fn uv_last_error() -> *const c_char {
    LAST.with(|l| l.borrow().as_ref().map_or(std::ptr::null(), |c| c.as_ptr()))
}
