//! UVSession owns the engine thread: one lightcraft_engine::Session, driven by messages.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use lightcraft_engine::Session;
use serde_json::Value;

use crate::panic_guard::{guard, panic_message};
use crate::status::{Failure, UVStatus};

const EXECUTE_TIMEOUT: Duration = Duration::from_secs(30);
const CAMERA_PROFILES: &str = "LIGHTCRAFT_CAMERA_PROFILES";
const QOS_CLASS_USER_INITIATED: u32 = 0x19;

unsafe extern "C" {
    fn pthread_set_qos_class_self_np(qos: u32, rel: i32) -> i32;
}

/// uv_frame_cb of uv.h. Called on the engine thread; `rgba` is valid only during the call.
pub type UvFrameCb = unsafe extern "C" fn(
    ctx: *mut c_void,
    rgba: *const u8,
    width: u32,
    height: u32,
    stride: u32,
    generation: u64,
    draft: bool,
);

/// Msg is what the engine thread receives. Each request carries its own reply channel.
pub(crate) enum Msg {
    Execute {
        command: String,
        params: Value,
        reply: Sender<Result<Value, Failure>>,
    },
    Shutdown,
}

/// UVSession is the opaque handle of uv.h. The engine Session never leaves its thread: callers
/// only hold the sending side of its channel, and dropping the handle stops and joins the thread.
pub struct UVSession {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

impl UVSession {
    /// spawn starts the engine thread and waits until the library at data_dir is open, so a
    /// session that exists is a session that works.
    fn spawn(data_dir: PathBuf) -> Result<UVSession, Failure> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("unveil-engine".into())
            .stack_size(8 << 20)
            .spawn(move || engine_main(data_dir, rx, ready_tx))
            .map_err(|e| {
                Failure::new(
                    UVStatus::UV_ERR_ENGINE,
                    format!("cannot start the engine thread: {e}"),
                )
            })?;
        let failure = match ready_rx.recv() {
            Ok(Ok(())) => {
                return Ok(UVSession {
                    tx,
                    thread: Some(thread),
                });
            }
            Ok(Err(failure)) => failure,
            Err(_) => stopped(),
        };
        let _ = thread.join();
        Err(failure)
    }

    fn execute(&self, command: &str, params: Value) -> Result<Value, Failure> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(Msg::Execute {
                command: command.to_owned(),
                params,
                reply,
            })
            .map_err(|_| stopped())?;
        match rx.recv_timeout(EXECUTE_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(Failure::new(
                UVStatus::UV_ERR_TIMEOUT,
                format!("`{command}` took longer than 30 s"),
            )),
            Err(RecvTimeoutError::Disconnected) => Err(stopped()),
        }
    }
}

impl Drop for UVSession {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn stopped() -> Failure {
    Failure::new(UVStatus::UV_ERR_ENGINE, "engine thread stopped")
}

/// set_user_initiated_qos gives the calling thread the QoS of work the user is waiting for.
pub(crate) fn set_user_initiated_qos() {
    // SAFETY: plain libc call on the current thread; it takes no pointers.
    unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INITIATED, 0) };
}

/// engine_main is the body of the engine thread. It reports the library open over `ready`, then
/// serves messages until Shutdown or until every sender is gone, and closes the library on exit.
/// A panic while handling one message becomes that message's UV_ERR_PANIC, so the thread lives on.
fn engine_main(data_dir: PathBuf, rx: Receiver<Msg>, ready: Sender<Result<(), Failure>>) {
    set_user_initiated_qos();
    let mut session = Session::new().with_fs();
    if let Err(e) = session.open_library(&data_dir, false) {
        let _ = ready.send(Err(Failure::engine(e)));
        return;
    }
    // Spec 4.2: no XMP sidecars, whatever the library's prefs.json says.
    session.xmp.auto_write = false;
    let _ = ready.send(Ok(()));
    for msg in rx {
        match msg {
            Msg::Shutdown => break,
            Msg::Execute {
                command,
                params,
                reply,
            } => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    session.execute(&command, &params).map_err(Failure::engine)
                }))
                .unwrap_or_else(|p| {
                    Err(Failure::new(
                        UVStatus::UV_ERR_PANIC,
                        format!("panic: {}", panic_message(&*p)),
                    ))
                });
                let _ = reply.send(result);
            }
        }
    }
    if let Err(e) = session.close_library() {
        log::warn!("unveil: closing the library failed: {e}");
    }
}

/// c_str borrows a C string argument as UTF-8, naming the argument in the error.
///
/// # Safety
/// `ptr` is NULL or a NUL-terminated string that stays valid for 'a.
unsafe fn c_str<'a>(ptr: *const c_char, name: &str) -> Result<&'a str, Failure> {
    if ptr.is_null() {
        return Err(Failure::invalid(format!("{name} is NULL")));
    }
    // SAFETY: non-NULL and NUL-terminated per the caller's contract; the borrow ends with the call.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| Failure::invalid(format!("{name} is not UTF-8")))
}

/// uv_session_new opens the library at data_dir on a new engine thread, or returns NULL with
/// uv_last_error set. It also points camera profiles at data_dir/camera-profiles, unless
/// LIGHTCRAFT_CAMERA_PROFILES is already set, so nothing is read from $HOME (spec 4.2).
/// memory_budget is ignored until the memory ticket (T6) applies it.
///
/// # Safety
/// `data_dir` is NULL or a NUL-terminated UTF-8 path, valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_session_new(data_dir: *const c_char, _memory_budget: u64) -> *mut UVSession {
    guard(|| {
        // SAFETY: the caller's contract above; the path is copied before returning.
        let dir = PathBuf::from(unsafe { c_str(data_dir, "data_dir") }?);
        if std::env::var_os(CAMERA_PROFILES).is_none() {
            // SAFETY: setenv is not thread-safe; the app creates the session at launch,
            // before any other thread reads the environment.
            unsafe { std::env::set_var(CAMERA_PROFILES, dir.join("camera-profiles")) };
        }
        Ok(Box::into_raw(Box::new(UVSession::spawn(dir)?)))
    })
}

/// uv_session_free stops the engine thread, closes the library and frees the session.
/// NULL is a no-op.
///
/// # Safety
/// `session` is NULL or a pointer from uv_session_new not yet freed; no call may use it after.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_session_free(session: *mut UVSession) {
    guard(|| {
        if !session.is_null() {
            // SAFETY: the pointer came from Box::into_raw in uv_session_new and is freed once.
            drop(unsafe { Box::from_raw(session) });
        }
        Ok(())
    })
}

/// uv_execute runs one engine command with JSON params (NULL means {}) and waits for it up to
/// 30 s. On UV_OK *result_json is a Rust-allocated JSON string the caller frees with
/// uv_string_free; on any failure it is NULL and uv_last_error says why.
///
/// # Safety
/// `session` is NULL or a live pointer from uv_session_new. `command` and `params_json` are NULL
/// or NUL-terminated strings valid for the call. `result_json` is NULL or points to writable storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_execute(
    session: *mut UVSession,
    command: *const c_char,
    params_json: *const c_char,
    result_json: *mut *mut c_char,
) -> i32 {
    guard(|| {
        if result_json.is_null() {
            return Err(Failure::invalid("result_json is NULL"));
        }
        // SAFETY: result_json is non-NULL and writable per the contract; cleared before any failure.
        unsafe { *result_json = std::ptr::null_mut() };
        // SAFETY: a live session per the contract; it is only borrowed for this call.
        let session = unsafe { session.as_ref() }.ok_or_else(|| Failure::invalid("session is NULL"))?;
        // SAFETY: the caller's contract; both strings are copied before returning.
        let command = unsafe { c_str(command, "command") }?;
        let params = if params_json.is_null() {
            Value::Object(Default::default())
        } else {
            // SAFETY: as above.
            let text = unsafe { c_str(params_json, "params_json") }?;
            serde_json::from_str(text).map_err(|e| Failure::invalid(format!("params_json: {e}")))?
        };
        let value = session.execute(command, params)?;
        // serde_json escapes NUL inside strings, so the output never contains one.
        let json = CString::new(value.to_string()).expect("JSON text has no NUL bytes");
        // SAFETY: as above; ownership of the string passes to the caller.
        unsafe { *result_json = json.into_raw() };
        Ok(UVStatus::UV_OK as i32)
    })
}

/// uv_string_free frees a string returned by uv_execute. NULL is a no-op.
///
/// # Safety
/// `string` is NULL or a pointer from uv_execute's result_json, freed only once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_string_free(string: *mut c_char) {
    guard(|| {
        if !string.is_null() {
            // SAFETY: the string came from CString::into_raw in uv_execute and is freed once.
            drop(unsafe { CString::from_raw(string) });
        }
        Ok(())
    })
}

fn not_implemented(name: &str) -> Failure {
    Failure::new(UVStatus::UV_ERR_ENGINE, format!("{name}: not implemented yet"))
}

/// uv_request_preview is a stub until the render ticket (T5): it returns 0 and sets
/// uv_last_error to "not implemented yet".
///
/// # Safety
/// Same contract as the final version: `session` is NULL or live, `ctx` is passed back untouched.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_request_preview(
    _session: *mut UVSession,
    _max_pixels: u32,
    _draft: bool,
    _callback: Option<UvFrameCb>,
    _ctx: *mut c_void,
) -> u64 {
    guard(|| Err(not_implemented("uv_request_preview")))
}

/// uv_suspend is a stub until the lifecycle ticket (T6): UV_ERR_ENGINE, "not implemented yet".
///
/// # Safety
/// `session` is NULL or a live pointer from uv_session_new.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_suspend(_session: *mut UVSession) -> i32 {
    guard(|| Err(not_implemented("uv_suspend")))
}

/// uv_resume is a stub until the lifecycle ticket (T6): UV_ERR_ENGINE, "not implemented yet".
///
/// # Safety
/// `session` is NULL or a live pointer from uv_session_new.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_resume(_session: *mut UVSession) -> i32 {
    guard(|| Err(not_implemented("uv_resume")))
}

/// uv_set_memory_budget is a no-op stub until the lifecycle ticket (T6).
///
/// # Safety
/// `session` is NULL or a live pointer from uv_session_new.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_set_memory_budget(_session: *mut UVSession, _bytes: u64) {
    guard(|| Ok(()))
}

/// uv_test_panic panics on the calling thread inside the guard, to test that a panic becomes
/// UV_ERR_PANIC. Only built with the test-hooks feature, never in the app.
///
/// # Safety
/// Never dereferences `session`.
#[cfg(feature = "test-hooks")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_test_panic(_session: *mut UVSession) -> i32 {
    guard(|| panic!("test panic"))
}
