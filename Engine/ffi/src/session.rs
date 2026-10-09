//! UVSession owns the engine thread: one lightcraft_engine::Session, driven by messages.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use lightcraft_engine::Session;
use lightcraft_engine::pipeline::Rendered;
use serde_json::Value;

use crate::memory_budget::install_release_hook;
use crate::panic_guard::{guard, panic_message};
use crate::preview_scheduler::{PreviewScheduler, Request};
use crate::render_worker::{RenderDone, RenderWorker, Work};
use crate::status::{Failure, UVStatus};

const EXECUTE_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_RENDER_WAIT: Duration = Duration::from_secs(10);
const SUSPEND_TIMEOUT: Duration = Duration::from_secs(2);
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

/// FrameCallback is a uv_frame_cb with its ctx, carried from uv_request_preview to the engine
/// thread, which is the only thread that ever calls it.
#[derive(Clone, Copy)]
pub(crate) struct FrameCallback {
    f: UvFrameCb,
    ctx: *mut c_void,
}

// SAFETY: uv_request_preview's caller guarantees ctx stays valid and usable from another
// thread for as long as the session lives; f is a plain function pointer.
unsafe impl Send for FrameCallback {}

impl FrameCallback {
    /// call hands the frame's pixels to the callback without a copy: RGBA8, rows of width * 4.
    fn call(&self, rendered: &Rendered, generation: u64, draft: bool) {
        let image = &rendered.image;
        let rgba = image.data.as_flattened();
        let (width, height) = (image.width as u32, image.height as u32);
        // SAFETY: rgba is owned by the RenderResult and outlives the call, which is all uv_frame_cb
        // promises; ctx is valid per uv_request_preview's contract.
        unsafe {
            (self.f)(
                self.ctx,
                rgba.as_ptr(),
                width,
                height,
                width * 4,
                generation,
                draft,
            )
        };
    }
}

/// PreviewRequest is one uv_request_preview. Its job is built only when it starts, so a request
/// that waited behind a render picks up the settings of the moment.
pub(crate) struct PreviewRequest {
    generation: u64,
    max_pixels: u32,
    draft: bool,
    callback: FrameCallback,
}

impl Request for PreviewRequest {
    fn generation(&self) -> u64 {
        self.generation
    }
}

/// Msg is what the engine thread receives. Each request carries its own reply channel;
/// RenderDone comes from unveil-render.
pub(crate) enum Msg {
    Execute {
        command: String,
        params: Value,
        reply: Sender<Result<Value, Failure>>,
    },
    Preview(PreviewRequest),
    SetBudget(u64),
    /// Answered once no render is in flight.
    Suspend {
        reply: Sender<()>,
    },
    // Boxed: a RenderResult is far larger than the other variants.
    RenderDone(Box<RenderDone>),
    Shutdown,
}

/// UVSession is the opaque handle of uv.h. The engine Session never leaves its thread: callers
/// only hold the sending side of its channel, and dropping the handle stops and joins the thread.
/// `has_active` mirrors session.active().is_some() after every command, so uv_request_preview
/// can refuse at once without waiting behind a long uv_execute. `suspended` is shared with the
/// engine thread, which drops any preview that reaches it while the flag is set.
pub struct UVSession {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    generation: AtomicU64,
    has_active: Arc<AtomicBool>,
    suspended: Arc<AtomicBool>,
}

impl UVSession {
    /// spawn starts the engine thread and waits until the library at data_dir is open, so a
    /// session that exists is a session that works.
    fn spawn(data_dir: PathBuf, memory_budget: u64) -> Result<UVSession, Failure> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let has_active = Arc::new(AtomicBool::new(false));
        let suspended = Arc::new(AtomicBool::new(false));
        let flags = Flags {
            has_active: has_active.clone(),
            suspended: suspended.clone(),
        };
        let done = tx.clone();
        let thread = thread::Builder::new()
            .name("unveil-engine".into())
            .stack_size(8 << 20)
            .spawn(move || engine_main(data_dir, memory_budget, rx, done, flags, ready_tx))
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
                    generation: AtomicU64::new(0),
                    has_active,
                    suspended,
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

    /// suspend refuses new previews at once, then waits up to 2 s for the render in flight (the
    /// pending one is dropped). On UV_OK nothing renders until resume.
    fn suspend(&self) -> Result<(), Failure> {
        self.suspended.store(true, Ordering::Release);
        let (reply, rx) = mpsc::channel();
        self.tx.send(Msg::Suspend { reply }).map_err(|_| stopped())?;
        match rx.recv_timeout(SUSPEND_TIMEOUT) {
            Ok(()) => Ok(()),
            Err(RecvTimeoutError::Timeout) => Err(Failure::new(
                UVStatus::UV_ERR_TIMEOUT,
                "a render is still in flight after 2 s",
            )),
            Err(RecvTimeoutError::Disconnected) => Err(stopped()),
        }
    }

    /// request_preview checks only atomics, then queues the request: it never waits on the engine.
    fn request_preview(
        &self,
        max_pixels: u32,
        draft: bool,
        callback: Option<UvFrameCb>,
        ctx: *mut c_void,
    ) -> Result<u64, Failure> {
        if self.suspended.load(Ordering::Acquire) {
            return Err(Failure::new(
                UVStatus::UV_ERR_SUSPENDED,
                "the session is suspended",
            ));
        }
        if !self.has_active.load(Ordering::Acquire) {
            return Err(Failure::new(UVStatus::UV_ERR_ENGINE, "no active photo"));
        }
        let f = callback.ok_or_else(|| Failure::invalid("callback is NULL"))?;
        if max_pixels == 0 {
            return Err(Failure::invalid("max_pixels is 0"));
        }
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let request = PreviewRequest {
            generation,
            max_pixels,
            draft,
            callback: FrameCallback { f, ctx },
        };
        self.tx.send(Msg::Preview(request)).map_err(|_| stopped())?;
        Ok(generation)
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

/// caught runs f, turning a panic into None and a log line: engine work outside a command has
/// no caller to report to, and the thread must live on.
pub(crate) fn caught<T>(what: &str, f: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .map_err(|p| log::error!("unveil: {what} panicked: {}", panic_message(&*p)))
        .ok()
}

/// Flags are the atomics the engine thread shares with UVSession.
struct Flags {
    has_active: Arc<AtomicBool>,
    suspended: Arc<AtomicBool>,
}

/// engine_main is the body of the engine thread. It reports the library open over `ready`, then
/// serves messages until Shutdown, and closes the library on exit. `done` is handed to
/// unveil-render for its RenderDone messages. A panic while handling one command becomes that
/// command's UV_ERR_PANIC, so the thread lives on.
fn engine_main(
    data_dir: PathBuf,
    memory_budget: u64,
    rx: Receiver<Msg>,
    done: Sender<Msg>,
    flags: Flags,
    ready: Sender<Result<(), Failure>>,
) {
    set_user_initiated_qos();
    let worker = match RenderWorker::spawn(done) {
        Ok(worker) => worker,
        Err(e) => {
            let message = format!("cannot start the render thread: {e}");
            let _ = ready.send(Err(Failure::new(UVStatus::UV_ERR_ENGINE, message)));
            return;
        }
    };
    let mut session = Session::new().with_fs();
    if let Err(e) = session.open_library(&data_dir, false) {
        let _ = ready.send(Err(Failure::engine(e)));
        return;
    }
    // Spec 4.2: no XMP sidecars, whatever the library's prefs.json says.
    session.xmp.auto_write = false;
    // 0 keeps the engine default.
    if memory_budget != 0 {
        session.set_memory_budget(memory_budget as usize);
    }
    let mut engine = Engine {
        session,
        scheduler: PreviewScheduler::new(),
        worker,
        flags,
        suspend_waiter: None,
    };
    engine.publish_active();
    let _ = ready.send(Ok(()));
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Shutdown => break,
            Msg::Execute {
                command,
                params,
                reply,
            } => {
                let result = engine.execute(&command, &params);
                engine.publish_active();
                let _ = reply.send(result);
            }
            Msg::Preview(request) => {
                // A request that raced uv_suspend: the caller was told "suspended" or will be.
                if engine.flags.suspended.load(Ordering::Acquire) {
                    continue;
                }
                if let Some(request) = engine.scheduler.submit(request) {
                    engine.start(request);
                }
            }
            Msg::SetBudget(bytes) => {
                caught("set_memory_budget", || {
                    engine.session.set_memory_budget(bytes as usize)
                });
            }
            Msg::Suspend { reply } => {
                engine.scheduler.clear_pending();
                engine.suspend_waiter = Some(reply);
                engine.answer_suspend();
            }
            Msg::RenderDone(done) => {
                engine.finish(*done);
                engine.answer_suspend();
            }
        }
    }
    engine.shutdown(&rx);
}

/// Engine is the state of the engine thread: the Session, the preview scheduler and the render
/// worker it feeds.
struct Engine {
    session: Session,
    scheduler: PreviewScheduler<PreviewRequest>,
    worker: RenderWorker,
    flags: Flags,
    suspend_waiter: Option<Sender<()>>,
}

impl Engine {
    fn execute(&mut self, command: &str, params: &Value) -> Result<Value, Failure> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.session.execute(command, params).map_err(Failure::engine)
        }))
        .unwrap_or_else(|p| {
            Err(Failure::new(
                UVStatus::UV_ERR_PANIC,
                format!("panic: {}", panic_message(&*p)),
            ))
        })
    }

    /// answer_suspend replies to a waiting uv_suspend once no render is in flight.
    fn answer_suspend(&mut self) {
        if self.scheduler.is_idle()
            && let Some(reply) = self.suspend_waiter.take()
        {
            let _ = reply.send(());
        }
    }

    fn publish_active(&self) {
        let active = self.session.active().is_some();
        self.flags.has_active.store(active, Ordering::Release);
    }

    /// start sends request's job to the worker. When there is nothing to render (the photo is
    /// gone) it counts as finished, and the next pending request gets its turn.
    fn start(&mut self, mut request: PreviewRequest) {
        loop {
            if let Some(job) = self.job_for(&request) {
                let work = Work {
                    job,
                    draft: request.draft,
                    callback: request.callback,
                };
                if self.worker.run(work) {
                    return;
                }
            }
            match self.scheduler.finished(request.generation) {
                Some(next) => request = next,
                None => return,
            }
        }
    }

    fn job_for(&mut self, request: &PreviewRequest) -> Option<lightcraft_engine::RenderJob> {
        let active = self.session.active()?;
        let max = request.max_pixels as usize;
        let mut job = caught("render_job", || {
            self.session.render_job(active, max, max, false, true)
        })??;
        if request.draft {
            job = job.draft();
        }
        job.request_id = request.generation;
        Some(job)
    }

    /// finish caches what the render decoded, delivers the frame if it is still the newest for
    /// the active photo, then starts the pending request, if any.
    fn finish(&mut self, done: RenderDone) {
        if let Some(result) = &done.result {
            caught("accept", || self.session.accept(result));
            match &result.rendered {
                Ok(rendered) => {
                    let active = self.session.active().map(|p| p.0);
                    if self
                        .scheduler
                        .should_deliver(done.generation, done.photo.0, active)
                    {
                        done.callback.call(rendered, done.generation, done.draft);
                    }
                }
                Err(e) => log::warn!("unveil: preview {} failed: {e}", done.generation),
            }
        }
        if let Some(next) = self.scheduler.finished(done.generation) {
            self.start(next);
        }
    }

    /// shutdown drops the pending request and waits up to 10 s for the one in flight, without
    /// delivering it: after uv_session_free returns, no callback may run.
    fn shutdown(mut self, rx: &Receiver<Msg>) {
        self.scheduler.clear_pending();
        let idle = self.scheduler.is_idle() || wait_for_render_done(rx);
        self.worker.stop(idle);
        if let Err(e) = self.session.close_library() {
            log::warn!("unveil: closing the library failed: {e}");
        }
    }
}

/// wait_for_render_done drains rx until the in-flight RenderDone arrives (true) or 10 s pass.
/// Anything else is dropped: the session is going away.
fn wait_for_render_done(rx: &Receiver<Msg>) -> bool {
    let deadline = Instant::now() + SHUTDOWN_RENDER_WAIT;
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Msg::RenderDone(_)) => return true,
            Ok(_) => continue,
            Err(_) => return false,
        }
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
/// A non-zero memory_budget (bytes) is applied before it returns; 0 keeps the engine default.
///
/// # Safety
/// `data_dir` is NULL or a NUL-terminated UTF-8 path, valid for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_session_new(data_dir: *const c_char, memory_budget: u64) -> *mut UVSession {
    guard(|| {
        // SAFETY: the caller's contract above; the path is copied before returning.
        let dir = PathBuf::from(unsafe { c_str(data_dir, "data_dir") }?);
        if std::env::var_os(CAMERA_PROFILES).is_none() {
            // SAFETY: setenv is not thread-safe; the app creates the session at launch,
            // before any other thread reads the environment.
            unsafe { std::env::set_var(CAMERA_PROFILES, dir.join("camera-profiles")) };
        }
        install_release_hook();
        Ok(Box::into_raw(Box::new(UVSession::spawn(dir, memory_budget)?)))
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

/// uv_request_preview asks for a preview of the active photo fitting max_pixels on its long
/// edge, at draft quality when `draft`, and returns its generation (the first is 1) at once.
/// The frame reaches `callback` later, on the engine thread, unless a newer request overtakes
/// it or the active photo changes first. On failure it returns 0 with uv_last_error set: "no
/// active photo" (UV_ERR_ENGINE) comes before a NULL callback (UV_ERR_INVALID_ARGUMENT).
/// It may be called from any thread: requests that reach the engine out of order never let an
/// older generation replace or follow a newer one, so the newest generation always wins.
///
/// # Safety
/// `session` is NULL or a live pointer from uv_session_new. `ctx` is passed back untouched and
/// must stay valid, and usable from the engine thread, until uv_session_free returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_request_preview(
    session: *mut UVSession,
    max_pixels: u32,
    draft: bool,
    callback: Option<UvFrameCb>,
    ctx: *mut c_void,
) -> u64 {
    guard(|| {
        // SAFETY: a live session per the contract; it is only borrowed for this call.
        let session = unsafe { session.as_ref() }.ok_or_else(|| Failure::invalid("session is NULL"))?;
        session.request_preview(max_pixels, draft, callback, ctx)
    })
}

/// uv_suspend stops GPU work for the background: new previews fail with UV_ERR_SUSPENDED, the
/// pending one is dropped, and GPU rendering is switched off, so a uv_execute that renders (an
/// export) runs on the CPU. It returns UV_OK once the render in flight is done (its frame is
/// still delivered, before the return). If that takes over 2 s it returns UV_ERR_TIMEOUT, and
/// the session stays suspended: the late frame is still delivered and uv_resume is still needed.
/// After UV_OK no frame is delivered until uv_resume; uv_execute keeps working.
///
/// # Safety
/// `session` is NULL or a live pointer from uv_session_new.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_suspend(session: *mut UVSession) -> i32 {
    guard(|| {
        // SAFETY: a live session per the contract; it is only borrowed for this call.
        let session = unsafe { session.as_ref() }.ok_or_else(|| Failure::invalid("session is NULL"))?;
        lightcraft_gpu::set_enabled(false);
        session.suspend()?;
        Ok(UVStatus::UV_OK as i32)
    })
}

/// uv_resume switches GPU rendering back on, clears a GPU failure recorded earlier and accepts
/// previews again. It never waits on the engine thread. That recovers errors that leave the GPU
/// device usable. A lost or never-created device is not recreated in v0: rendering stays on the
/// CPU until the app is relaunched.
///
/// # Safety
/// `session` is NULL or a live pointer from uv_session_new.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_resume(session: *mut UVSession) -> i32 {
    guard(|| {
        // SAFETY: a live session per the contract; it is only borrowed for this call.
        let session = unsafe { session.as_ref() }.ok_or_else(|| Failure::invalid("session is NULL"))?;
        lightcraft_gpu::reset_failures();
        lightcraft_gpu::set_enabled(true);
        session.suspended.store(false, Ordering::Release);
        Ok(UVStatus::UV_OK as i32)
    })
}

/// uv_set_memory_budget sets the process-wide memory budget (at least 64 MiB, the engine clamps
/// it) at once, then has the engine thread resize this session's caches. It does not wait.
///
/// # Safety
/// `session` is NULL or a live pointer from uv_session_new.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_set_memory_budget(session: *mut UVSession, bytes: u64) {
    guard(|| {
        // SAFETY: a live session per the contract; it is only borrowed for this call.
        let session = unsafe { session.as_ref() }.ok_or_else(|| Failure::invalid("session is NULL"))?;
        lightcraft_engine::memory::set_budget(bytes as usize);
        session.tx.send(Msg::SetBudget(bytes)).map_err(|_| stopped())?;
        Ok(())
    })
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
