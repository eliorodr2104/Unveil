//! guard wraps every exported call so that no panic unwinds across the C boundary.

use std::any::Any;

use crate::last_error;
use crate::status::{Failure, UVStatus};

/// FromStatus is the value an export returns when it fails: the status itself for int32_t,
/// 0 for a generation, NULL for a pointer, nothing for void.
pub trait FromStatus {
    fn from_status(status: UVStatus) -> Self;
}

impl FromStatus for i32 {
    fn from_status(status: UVStatus) -> i32 {
        status as i32
    }
}

impl FromStatus for u64 {
    fn from_status(_: UVStatus) -> u64 {
        0
    }
}

impl FromStatus for () {
    fn from_status(_: UVStatus) {}
}

impl<T> FromStatus for *mut T {
    fn from_status(_: UVStatus) -> *mut T {
        std::ptr::null_mut()
    }
}

/// guard runs one exported call: a panic becomes UV_ERR_PANIC with its message, never an unwind
/// across the C boundary, which would abort the process. It clears the last error first, so the
/// error of a call is visible only until the next call on the same thread.
pub fn guard<T: FromStatus>(f: impl FnOnce() -> Result<T, Failure>) -> T {
    last_error::clear();
    let failure = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(v)) => return v,
        Ok(Err(failure)) => failure,
        Err(payload) => Failure::new(
            UVStatus::UV_ERR_PANIC,
            format!("panic: {}", panic_message(&*payload)),
        ),
    };
    last_error::set(&failure.message);
    T::from_status(failure.status)
}

/// panic_message extracts the text of a panic payload, for guard and the engine thread.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into())
}
