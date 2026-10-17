//! UVSession owns the engine thread: one lightcraft_engine::Session, driven by messages.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::files::PreparedSources;
use lightcraft_engine::pipeline::geometry::Frame;
use lightcraft_engine::pipeline::{PixelWindow, Rendered};
use lightcraft_engine::{RenderJob, Session, SourceLevel};
use serde_json::{Value, json};

use crate::diagnostics::{self, Category, Part, Span};
use crate::memory_budget::install_release_hook;
use crate::panic_guard::{guard, panic_message};
use crate::prepared_open;
use crate::preview_scheduler::{PreviewScheduler, Request};
use crate::preview_stages::PreviewStages;
use crate::render_worker::{RenderDone, RenderWorker, Work};
use crate::status::{Failure, UVStatus};

const EXECUTE_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_RENDER_WAIT: Duration = Duration::from_secs(10);
const SUSPEND_TIMEOUT: Duration = Duration::from_secs(2);
const CAMERA_PROFILES: &str = "LIGHTCRAFT_CAMERA_PROFILES";
const QOS_CLASS_USER_INITIATED: u32 = 0x19;

/// HISTOGRAM_PIXELS is the long edge of the small whole frame a window's histogram comes from: a
/// few ms of GPU work next to a window's tens, and plenty of samples for 256 bins.
const HISTOGRAM_PIXELS: usize = 512;

unsafe extern "C" {
    fn pthread_set_qos_class_self_np(qos: u32, rel: i32) -> i32;
}

/// UVFrame of uv.h: one delivered frame. `rgba` holds `height` rows of `stride` bytes and, like
/// the struct itself, is valid only during the callback. A whole-frame preview has `window` false,
/// its own size as the frame size and origin 0; a window (uv_request_window) has `window` true and
/// sits at (x, y) of a `frame_width × frame_height` frame. `fallback` marks the whole frame that
/// answers a window the engine could not render alone. `native_width × native_height` is the
/// whole frame at the photo's own resolution (cropped and straightened): what 100 % means.
/// `status` is UV_OK for a frame. A render that failed is delivered as a frame with `status` set,
/// `error` pointing to its message (valid only during the call), `rgba` NULL and the sizes 0;
/// `generation`, `draft` and `window` still say which request failed. `histogram` describes a
/// whole frame (a preview or a fallback, draft or not) and is NULL for a failure. A window carries
/// one only when its request asked for it (ABI 6) and its small whole frame rendered: the histogram
/// of that frame, rendered with the window's settings, so it still describes the whole photo. If
/// the small frame fails, the window is still delivered, with `histogram` NULL.
/// `display_width` and `display_height` preserve logical coverage when a draft has fewer pixels.
#[repr(C)]
#[derive(Debug)]
pub struct UVFrame {
    pub rgba: *const u8,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub generation: u64,
    pub draft: bool,
    pub window: bool,
    pub fallback: bool,
    pub frame_width: u32,
    pub frame_height: u32,
    pub x: u32,
    pub y: u32,
    pub native_width: u32,
    pub native_height: u32,
    pub status: i32,
    pub error: *const c_char,
    pub histogram: *const UVHistogram,
    pub display_width: u32,
    pub display_height: u32,
}

/// UVHistogram of uv.h: the engine's histogram of a whole frame, 256 bins per channel and for
/// luminance, over at most about 1 MP sampled on a grid; `total` is the pixels sampled. The
/// clipped counts are the sampled pixels at 0 (`clipped_low`) and 255 (`clipped_high`) in R, G, B.
#[repr(C)]
#[derive(Debug)]
pub struct UVHistogram {
    pub r: [u32; 256],
    pub g: [u32; 256],
    pub b: [u32; 256],
    pub luma: [u32; 256],
    pub total: u32,
    pub clipped_low: [u32; 3],
    pub clipped_high: [u32; 3],
}

impl UVHistogram {
    /// of copies the histogram the engine computed with the frame. Upstream's bins are always 256
    /// long; a shorter one would leave its missing bins at 0 rather than fail the frame.
    fn of<I>(rendered: &Rendered<I>) -> UVHistogram {
        let h = &rendered.histogram;
        let bins = |v: &[u32]| {
            let mut out = [0u32; 256];
            let n = v.len().min(256);
            out[..n].copy_from_slice(&v[..n]);
            out
        };
        let (r, g, b) = (bins(&h.r), bins(&h.g), bins(&h.b));
        UVHistogram {
            luma: bins(&h.luma),
            total: h.total,
            clipped_low: [r[0], g[0], b[0]],
            clipped_high: [r[255], g[255], b[255]],
            r,
            g,
            b,
        }
    }
}

/// uv_frame_cb of uv.h. Called on the engine thread; `frame` is valid only during the call.
pub type UvFrameCb = unsafe extern "C" fn(ctx: *mut c_void, frame: *const UVFrame);

/// Placement is where a window's pixels sit: the size of the whole frame as the render fits it,
/// and the window's origin in that frame once clamped inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    pub frame_width: u32,
    pub frame_height: u32,
    pub x: u32,
    pub y: u32,
}

/// Delivery is what a frame carries besides its pixels: a window's placement (None for a whole
/// frame), whether a whole frame answers a window request, and the frame's native size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Delivery {
    pub placement: Option<Placement>,
    pub fallback: bool,
    pub native: (u32, u32),
}

/// FrameCallback is a uv_frame_cb with its ctx, carried from uv_request_preview or
/// uv_request_window to the engine thread, which is the only thread that ever calls it.
#[derive(Clone, Copy)]
pub(crate) struct FrameCallback {
    f: UvFrameCb,
    ctx: *mut c_void,
}

// SAFETY: uv_request_preview's caller guarantees ctx stays valid and usable from another
// thread for as long as the session lives; f is a plain function pointer.
unsafe impl Send for FrameCallback {}

impl FrameCallback {
    /// call borrows the frame's RGBA8 pixels; mapped crops can have a larger row stride.
    /// Without a placement the pixels are the whole frame. `whole_frame` is the small whole frame a
    /// window's histogram comes from, if one was rendered.
    fn call(
        &self,
        rendered: &Rendered<lightcraft_gpu::DisplayImage>,
        whole_frame: Option<&Rendered>,
        generation: u64,
        draft: bool,
        delivery: Delivery,
    ) {
        let image = &rendered.image;
        let result = image.with_pixels(|rgba, stride| {
            let (width, height) = (image.width as u32, image.height as u32);
            let (display_width, display_height) =
                (rendered.display_size.0 as u32, rendered.display_size.1 as u32);
            let whole = Placement { frame_width: display_width, frame_height: display_height, x: 0, y: 0 };
            let at = delivery.placement.unwrap_or(whole);
            // A window's own histogram covers only the window; the app's describes the whole photo.
            let histogram = match delivery.placement {
                None => Some(UVHistogram::of(rendered)),
                Some(_) => whole_frame.map(UVHistogram::of),
            };
            let frame = UVFrame {
                rgba: rgba.as_ptr(),
                width,
                height,
                stride: stride as u32,
                generation,
                draft,
                window: delivery.placement.is_some(),
                fallback: delivery.fallback,
                frame_width: at.frame_width,
                frame_height: at.frame_height,
                x: at.x,
                y: at.y,
                native_width: delivery.native.0,
                native_height: delivery.native.1,
                status: UVStatus::UV_OK as i32,
                error: std::ptr::null(),
                histogram: histogram.as_ref().map_or(std::ptr::null(), |h| h as *const UVHistogram),
                display_width,
                display_height,
            };
            // SAFETY: frame, its pixels (owned by the RenderResult) and histogram outlive the call, which
            // is all uv_frame_cb promises; ctx is valid per the request's contract.
            unsafe { (self.f)(self.ctx, &frame) };
        });
        if let Err(message) = result {
            self.call_failure(generation, draft, delivery, Failure::new(UVStatus::UV_ERR_ENGINE, message));
        }
    }

    /// call_failure tells the callback that a render failed: no pixels, `status` and `message`
    /// instead, for the request's generation. Like call, it runs on the engine thread only.
    fn call_failure(&self, generation: u64, draft: bool, delivery: Delivery, failure: Failure) {
        // A message with a NUL cannot cross as a C string: blank it out rather than lose it.
        let message = CString::new(failure.message.replace('\0', " ")).unwrap_or_default();
        let frame = UVFrame {
            rgba: std::ptr::null(),
            width: 0,
            height: 0,
            stride: 0,
            generation,
            draft,
            window: delivery.placement.is_some(),
            fallback: delivery.fallback,
            frame_width: 0,
            frame_height: 0,
            x: 0,
            y: 0,
            native_width: delivery.native.0,
            native_height: delivery.native.1,
            status: failure.status as i32,
            error: message.as_ptr(),
            histogram: std::ptr::null(),
            display_width: 0,
            display_height: 0,
        };
        // SAFETY: frame and message outlive the call, which is all uv_frame_cb promises; ctx is
        // valid per the request's contract.
        unsafe { (self.f)(self.ctx, &frame) };
    }
}

/// PreviewRequest is one uv_request_preview or uv_request_window. Its job is built only when it
/// starts, so a request that waited behind a render picks up the settings of the moment. `before`
/// asks for the photo's unedited look (its crop kept) instead of the edited one, and `histogram`
/// asks a window to carry the whole photo's histogram.
pub(crate) struct PreviewRequest {
    generation: u64,
    kind: RequestKind,
    draft: bool,
    before: bool,
    histogram: bool,
    callback: FrameCallback,
}

/// RequestKind is what a PreviewRequest renders: the whole frame fitted in `max_pixels`, or
/// `window`'s pixels of the frame fitted in `full_w × full_h`.
#[derive(Clone, Copy)]
enum RequestKind {
    Fit { max_pixels: u32 },
    Window { full_w: u32, full_h: u32, window: PixelWindow },
}

impl Request for PreviewRequest {
    fn generation(&self) -> u64 {
        self.generation
    }

    fn is_window(&self) -> bool {
        matches!(self.kind, RequestKind::Window { .. })
    }

    fn absorb(&mut self, replaced: &Self) {
        self.histogram |= replaced.histogram;
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
    ReleaseWindows,
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
    prepared: Arc<PreparedSources>,
}

impl UVSession {
    /// spawn starts the engine thread and waits until the library at data_dir is open, so a
    /// session that exists is a session that works.
    fn spawn(data_dir: PathBuf, memory_budget: u64) -> Result<UVSession, Failure> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let has_active = Arc::new(AtomicBool::new(false));
        let suspended = Arc::new(AtomicBool::new(false));
        let prepared = Arc::new(PreparedSources::new(128 << 20));
        let flags = Flags {
            has_active: has_active.clone(),
            suspended: suspended.clone(),
            prepared: prepared.clone(),
        };
        let done = tx.clone();
        let thread = thread::Builder::new()
            .name("unveil-engine".into())
            .stack_size(8 << 20)
            .spawn(move || engine_main(data_dir, memory_budget, rx, done, flags, ready_tx))
            .map_err(|e| {
                Failure::new(UVStatus::UV_ERR_ENGINE, format!("cannot start the engine thread: {e}"))
            })?;
        let failure = match ready_rx.recv() {
            Ok(Ok(())) => {
                return Ok(UVSession {
                    tx,
                    thread: Some(thread),
                    generation: AtomicU64::new(0),
                    has_active,
                    suspended,
                    prepared,
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
        self.tx.send(Msg::Execute { command: command.to_owned(), params, reply }).map_err(|_| stopped())?;
        match rx.recv_timeout(EXECUTE_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                Err(Failure::new(UVStatus::UV_ERR_TIMEOUT, format!("`{command}` took longer than 30 s")))
            }
            Err(RecvTimeoutError::Disconnected) => Err(stopped()),
        }
    }

    /// suspend refuses new previews at once, then waits up to 2 s for the render in flight (the
    /// pending one is dropped). On UV_OK nothing renders until resume.
    fn suspend(&self) -> Result<(), Failure> {
        self.suspended.store(true, Ordering::Release);
        self.prepared.clear();
        let (reply, rx) = mpsc::channel();
        self.tx.send(Msg::Suspend { reply }).map_err(|_| stopped())?;
        match rx.recv_timeout(SUSPEND_TIMEOUT) {
            Ok(()) => Ok(()),
            Err(RecvTimeoutError::Timeout) => {
                Err(Failure::new(UVStatus::UV_ERR_TIMEOUT, "a render is still in flight after 2 s"))
            }
            Err(RecvTimeoutError::Disconnected) => Err(stopped()),
        }
    }

    /// request checks only atomics and its arguments, then queues the request: it never waits on
    /// the engine. The order of the checks is the documented order of the errors.
    fn request(
        &self,
        kind: RequestKind,
        draft: bool,
        before: bool,
        histogram: bool,
        callback: Option<UvFrameCb>,
        ctx: *mut c_void,
    ) -> Result<u64, Failure> {
        if self.suspended.load(Ordering::Acquire) {
            return Err(Failure::new(UVStatus::UV_ERR_SUSPENDED, "the session is suspended"));
        }
        if !self.has_active.load(Ordering::Acquire) {
            return Err(Failure::new(UVStatus::UV_ERR_ENGINE, "no active photo"));
        }
        let f = callback.ok_or_else(|| Failure::invalid("callback is NULL"))?;
        match kind {
            RequestKind::Fit { max_pixels: 0 } => return Err(Failure::invalid("max_pixels is 0")),
            RequestKind::Window { full_w, full_h, .. } if full_w == 0 || full_h == 0 => {
                return Err(Failure::invalid("the frame size is 0"));
            }
            RequestKind::Window { window, .. } if window.w == 0 || window.h == 0 => {
                return Err(Failure::invalid("the window size is 0"));
            }
            _ => {}
        }
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let request =
            PreviewRequest { generation, kind, draft, before, histogram, callback: FrameCallback { f, ctx } };
        self.tx.send(Msg::Preview(request)).map_err(|_| stopped())?;
        Ok(generation)
    }
}

impl Drop for UVSession {
    fn drop(&mut self) {
        self.prepared.clear();
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

/// Flags are the atomics and prepared-source owner the engine thread shares with UVSession.
struct Flags {
    has_active: Arc<AtomicBool>,
    suspended: Arc<AtomicBool>,
    prepared: Arc<PreparedSources>,
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
    let mut session = Session::new().with_prepared_fs(flags.prepared.clone());
    diagnostics::instrument_loads(&mut session);
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
        stages: PreviewStages::default(),
        window_stages: PreviewStages::default(),
        preview_pixels: SourceLevel::Preview.max_edge() as u32,
        last_fallback: None,
        is_release_pending: false,
        worker,
        flags,
        suspend_waiter: None,
        pool_idle: crate::pool_idle::PoolIdle::default(),
    };
    engine.publish_active();
    let _ = ready.send(Ok(()));
    loop {
        // Observe the deadline even while read-only diagnostic messages keep arriving.
        engine.trim_idle_pool();
        let timeout =
            [engine.flags.prepared.pending_timeout(), engine.pool_idle.remaining(std::time::Instant::now())]
                .into_iter()
                .flatten()
                .min();
        let received = match timeout {
            Some(timeout) => rx.recv_timeout(timeout),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let msg = match received {
            Ok(msg) => msg,
            Err(RecvTimeoutError::Timeout) => {
                engine.flags.prepared.expire_pending();
                engine.trim_idle_pool();
                lightcraft_engine::memory::release();
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match msg {
            Msg::Shutdown => break,
            Msg::Execute { command, params, reply } => {
                let result = engine.execute(&command, &params);
                engine.publish_active();
                engine.stages.keep_only(engine.session.active());
                engine.window_stages.keep_only(engine.session.active());
                let _ = reply.send(result);
            }
            Msg::Preview(request) => {
                engine.pool_idle.cancel();
                // A request that raced uv_suspend: the caller was told "suspended" or will be.
                if engine.flags.suspended.load(Ordering::Acquire) {
                    continue;
                }
                // A window wanted again keeps what the release would free.
                engine.is_release_pending &= !request.is_window();
                if let Some(request) = engine.scheduler.submit(request) {
                    engine.start(request);
                }
            }
            Msg::SetBudget(bytes) => {
                if let Some(budget) =
                    caught("set_memory_budget", || engine.session.set_memory_budget(bytes as usize))
                {
                    engine.trim_stages(budget);
                }
            }
            Msg::ReleaseWindows => {
                engine.window_stages = PreviewStages::default();
                engine.is_release_pending = true;
                engine.release_windows();
            }
            Msg::Suspend { reply } => {
                engine.pool_idle.cancel();
                engine.scheduler.clear_pending();
                // Drop the stages (171 MB NEF, 153 MB RAF at 2360 px, measured on the Mac at the iPad budget)
                // into the GPU pool, emptied by uv_suspend; a render in flight keeps its copy until it ends.
                engine.stages = PreviewStages::default();
                engine.window_stages = PreviewStages::default();
                engine.session.media.release_device_sources();
                engine.session.media.release_full_source();
                engine.suspend_waiter = Some(reply);
                engine.answer_suspend();
            }
            Msg::RenderDone(done) => {
                engine.finish(*done);
                engine.release_windows();
                engine.answer_suspend();
            }
        }
    }
    engine.shutdown(&rx);
}

/// Engine is the state of the engine thread: the Session, the preview scheduler, the resident
/// stages of the two views (the whole-frame preview and the zoom window) and the render worker it
/// feeds. The views keep separate stages because they read different source levels and sizes:
/// one cache for both would be dropped at every switch between a preview and a window.
/// `preview_pixels` is the newest preview's max_pixels, which picks the source level of a window's
/// histogram frame (see histogram_job). `last_fallback` is the job key of the last fallback started
/// (see job_for), and `is_release_pending` a uv_release_windows waiting for the render in flight
/// (see release_windows).
struct Engine {
    session: Session,
    scheduler: PreviewScheduler<PreviewRequest>,
    stages: PreviewStages,
    window_stages: PreviewStages,
    preview_pixels: u32,
    last_fallback: Option<u64>,
    is_release_pending: bool,
    worker: RenderWorker,
    flags: Flags,
    suspend_waiter: Option<Sender<()>>,
    pool_idle: crate::pool_idle::PoolIdle,
}

impl Engine {
    fn trim_idle_pool(&mut self) {
        if self.scheduler.is_idle() && self.pool_idle.take_due(Instant::now()) {
            lightcraft_engine::files::decode_memory::checkpoint("pool-trim-begin");
            lightcraft_gpu::trim_reusable(0);
            lightcraft_engine::files::decode_memory::checkpoint("pool-trim-end");
            lightcraft_engine::memory::release();
        }
    }

    fn execute(&mut self, command: &str, params: &Value) -> Result<Value, Failure> {
        // Bridge diagnostics; experimental pool controls require an explicitly enabled session.
        match command {
            "unveil.diagnostics" => {
                let enabled = params.get("enabled").and_then(Value::as_bool).unwrap_or(false);
                return Ok(diagnostics::set_enabled(enabled));
            }
            "unveil.loads" => return Ok(diagnostics::loads_json()),
            "unveil.decode-memory" => {
                return Ok(crate::decode_memory::command(params.get("enabled").and_then(Value::as_bool)));
            }
            "unveil.pool" => {
                if diagnostics::is_enabled() {
                    if let Some(limit) = params.get("limitBytes").and_then(Value::as_u64) {
                        lightcraft_gpu::set_pool_limit(limit);
                    }
                    if let Some(keep) = params.get("trimFreeBytes").and_then(Value::as_u64) {
                        lightcraft_gpu::trim_reusable(keep);
                    }
                    if let Some(enabled) = params.get("idleTrim").and_then(Value::as_bool) {
                        self.pool_idle.set_enabled(enabled);
                        if self.scheduler.is_idle() && !self.flags.suspended.load(Ordering::Acquire) {
                            self.pool_idle.completed(Instant::now());
                        }
                    }
                }
                let stats =
                    lightcraft_gpu::pool_stats(params.get("reset").and_then(Value::as_bool).unwrap_or(false));
                return Ok(json!({
                    "limitBytes": lightcraft_gpu::pool_limit(),
                    "allocated": lightcraft_gpu::memory().allocated, "pooled": lightcraft_gpu::memory().pooled,
                    "allocatedPeak": stats.allocated_peak, "pooledPeak": stats.pooled_peak,
                    "sourceUploads": lightcraft_gpu::source_uploads(), "incomplete": stats.incomplete,
                    "buckets": stats.buckets.iter().map(|b| json!({
                        "upperBytes": b.upper_bytes, "usage": b.usage, "allocations": b.allocations,
                        "reuses": b.reuses, "returns": b.returns, "frees": b.frees,
                    })).collect::<Vec<_>>(),
                }));
            }
            "unveil.renders" => {
                let reset = params.get("reset").and_then(Value::as_bool).unwrap_or(false);
                return Ok(diagnostics::renders_json(reset));
            }
            _ => {}
        }
        let previous_path = prepared_open::active_path(&self.session);
        let preparation = prepared_open::before(&self.flags.prepared, command, params);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.session.execute(command, params).map_err(Failure::engine)
        }))
        .unwrap_or_else(|p| {
            Err(Failure::new(UVStatus::UV_ERR_PANIC, format!("panic: {}", panic_message(&*p))))
        });
        prepared_open::after(
            &self.flags.prepared,
            &self.session,
            command,
            previous_path.as_deref(),
            preparation,
            &result,
        );
        if previous_path != prepared_open::active_path(&self.session) {
            self.pool_idle.cancel();
            if self.scheduler.is_idle() {
                self.pool_idle.completed(Instant::now());
            }
        }
        // library.memory also tells what the two views' resident stages hold (host and device).
        match (command, result) {
            ("library.memory", Ok(mut memory)) => {
                memory["unveil"] = json!({
                    "previewStages": { "cpu": self.stages.cpu_bytes(), "gpu": self.stages.gpu_bytes() },
                    "windowStages": { "cpu": self.window_stages.cpu_bytes(), "gpu": self.window_stages.gpu_bytes() },
                    "preparedSource": prepared_open::memory(&self.flags.prepared),
                });
                Ok(memory)
            }
            (_, result) => result,
        }
    }

    /// answer_suspend replies to a waiting uv_suspend once no render is in flight.
    fn answer_suspend(&mut self) {
        if self.scheduler.is_idle()
            && let Some(reply) = self.suspend_waiter.take()
        {
            let _ = reply.send(());
        }
    }

    /// trim_stages applies the shared quarter-budget to both views. The window goes first,
    /// preserving the fit preview for as long as the budget permits.
    fn trim_stages(&mut self, budget: usize) {
        if self.stages.bytes() + self.window_stages.bytes() > budget / 4 {
            self.window_stages = PreviewStages::default();
        }
        self.stages.trim(budget);
    }

    /// release_windows ends a uv_release_windows once no render is in flight, when the dropped
    /// window stages have reached the GPU pool. Release the full original unless the fit preview
    /// still needs it, then return pooled buffers and freed host pages.
    fn release_windows(&mut self) {
        if self.is_release_pending && self.scheduler.is_idle() {
            self.is_release_pending = false;
            self.pool_idle.cancel();
            // A tight crop can use Full even in the fit preview. Keep that active source.
            if !self.stages.uses_full_source() {
                self.session.media.release_full_source();
            }
            lightcraft_gpu::trim_pool(0);
            lightcraft_engine::memory::release();
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
            if let Some((job, histogram_job, delivery)) = self.job_for(&request) {
                let work =
                    Work { job, histogram_job, draft: request.draft, delivery, callback: request.callback };
                if self.worker.run(work) {
                    return;
                }
            }
            match self.scheduler.finished(request.generation) {
                Some(next) => request = next,
                None => {
                    if !self.flags.suspended.load(Ordering::Acquire) {
                        self.pool_idle.completed(Instant::now());
                    }
                    return;
                }
            }
        }
    }

    /// job_for builds request's job for the active photo, with its view's stages attached, and what
    /// its frame will carry. A window the engine cannot render alone (`region_job` gives None when a
    /// spot reads farther than one render holds) becomes a whole-frame preview fitted in the
    /// window's long edge, which the caller sized to fit its buffers, marked as a fallback so the
    /// caller drops the window it shows. The same fallback asked again (a pan within the spot's
    /// reach) would change nothing on screen, so it is skipped: None, as for a vanished photo.
    /// A window that asked for a histogram also gets its histogram job (a fallback has its own).
    fn job_for(&mut self, request: &PreviewRequest) -> Option<(RenderJob, Option<RenderJob>, Delivery)> {
        let active = self.session.active()?;
        let frame = photo_frame(&self.session, active)?;
        let (mut job, placement, fallback) = match request.kind {
            RequestKind::Fit { max_pixels } => {
                self.preview_pixels = max_pixels;
                (self.fit_job(active, max_pixels, request.before)?, None, false)
            }
            RequestKind::Window { full_w, full_h, window } => {
                let (full_w, full_h) = (full_w as usize, full_h as usize);
                let region = caught("region_job", || {
                    if request.before {
                        self.session.region_job_before(active, full_w, full_h, window, true)
                    } else {
                        self.session.region_job(active, full_w, full_h, window, true)
                    }
                })?;
                match region {
                    Some(job) => {
                        let placement = placement_of(&frame, full_w, full_h, window);
                        (job, Some(placement), false)
                    }
                    None => {
                        let long_edge = window.w.max(window.h) as u32;
                        (self.fit_job(active, long_edge, request.before)?, None, true)
                    }
                }
            }
        };
        if request.draft {
            job = job.reduced_draft();
        }
        // Drafts share native processing stages; only their delivered display raster is reduced.
        job = if placement.is_some() { self.window_stages.attach(job) } else { self.stages.attach(job) };
        let is_repeat = fallback && self.last_fallback == Some(job.key);
        self.last_fallback = fallback.then_some(job.key);
        if is_repeat {
            return None;
        }
        let (native_w, native_h) = frame.native_size();
        let native = (native_w.round().max(1.0) as u32, native_h.round().max(1.0) as u32);
        job.request_id = request.generation;
        let histogram_job = if request.histogram && placement.is_some() {
            self.histogram_job(active, request.before).map(|whole| {
                let mut whole = if request.draft { whole.draft() } else { whole };
                whole.request_id = request.generation;
                whole
            })
        } else {
            None
        };
        let delivery = Delivery { placement, fallback, native };
        Some((job, histogram_job, delivery))
    }

    /// histogram_job is the whole frame of `active` fitted in HISTOGRAM_PIXELS that a window's
    /// histogram comes from. It reads the preview's source level (that of the newest preview's
    /// size; a size of its own would pick the thumbnail level and decode the file again) through
    /// the preview's stages: the source is uploaded once, and the stage cache keeps two output
    /// sizes, so the preview's and this one stay warm side by side.
    fn histogram_job(&mut self, active: PhotoId, before: bool) -> Option<RenderJob> {
        let job = self.fit_job(active, self.preview_pixels, before)?;
        let job = self.stages.attach(job);
        Some(histogram_fit(job))
    }

    /// fit_job builds the whole-frame preview before its draft/full stage owner is selected.
    /// `before` renders the unedited look. Stages are keyed by the inputs they depend on, so the looks
    /// share what they have in common and never read a stage of the other's settings; an edit to
    /// white balance, noise reduction, spots or optics makes them replace each other's linear stage,
    /// so the first frame after a toggle recomputes it.
    fn fit_job(&mut self, active: PhotoId, max_pixels: u32, before: bool) -> Option<RenderJob> {
        let max = max_pixels as usize;
        let job = caught("render_job", || self.session.render_job(active, max, max, before, true))??;
        Some(job)
    }

    /// finish caches what the render decoded, delivers the frame, or its failure, if it is still
    /// the newest for the active photo, then starts the pending request, if any.
    fn finish(&mut self, done: RenderDone) {
        let outcome = match &done.result {
            Some(result) => {
                caught("accept", || self.session.accept(result));
                result.rendered.as_ref().map_err(|e| {
                    log::warn!("unveil: preview {} failed: {e}", done.generation);
                    Failure::new(UVStatus::UV_ERR_ENGINE, e.as_str())
                })
            }
            None => Err(Failure::new(UVStatus::UV_ERR_PANIC, "the render panicked")),
        };
        // Without its histogram frame a window is still delivered, with no histogram.
        let whole_frame = done.histogram.as_ref().and_then(|result| {
            caught("accept", || self.session.accept(result));
            result
                .rendered
                .as_ref()
                .map_err(|e| log::warn!("unveil: histogram of {} failed: {e}", done.generation))
                .ok()
        });
        let active = self.session.active().map(|p| p.0);
        if self.scheduler.should_deliver(done.generation, done.photo.0, active) {
            let category = Category::of(done.draft, done.delivery.placement.is_some());
            let span = Span::start();
            match outcome {
                Ok(rendered) => {
                    done.callback.call(rendered, whole_frame, done.generation, done.draft, done.delivery);
                    diagnostics::record(category, Part::Deliver, &span);
                }
                Err(failure) => {
                    done.callback.call_failure(done.generation, done.draft, done.delivery, failure)
                }
            }
        }
        self.trim_stages(lightcraft_engine::memory::budget());
        if let Some(next) = self.scheduler.finished(done.generation) {
            self.start(next);
        } else if self.scheduler.is_idle() && !self.flags.suspended.load(Ordering::Acquire) {
            self.pool_idle.completed(std::time::Instant::now());
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

/// histogram_fit turns a whole-frame preview job into the HISTOGRAM_PIXELS one a window's
/// histogram comes from, keeping the preview's source level, source and stages.
pub(crate) fn histogram_fit(mut job: RenderJob) -> RenderJob {
    let edge = (job.request.max_w.max(job.request.max_h) ^ HISTOGRAM_PIXELS) as u64;
    job.request.max_w = HISTOGRAM_PIXELS;
    job.request.max_h = HISTOGRAM_PIXELS;
    job.request.raster_scale = lightcraft_engine::pipeline::RasterScale::Native;
    // Swaps the fitted size in the key: the result is the key render_job gives a 512 px fit at
    // this level (XOR distributes over the shifts).
    job.key ^= (edge << 40) ^ (edge << 20);
    job
}

/// photo_frame is photo `id`'s output frame as upstream's region_job builds it for a window: the
/// recorded size, oriented, lens-corrected, cropped and straightened by the photo's develop settings
/// (the ones every job of the photo renders). Its native size is the frame at 100 %.
// ponytail: the pipeline frames the decoded source and solves an automatic Upright stored without
// its transform (XMP); either can move the frame by a pixel. Upstream reporting its frame fixes it.
fn photo_frame(session: &Session, id: PhotoId) -> Option<Frame> {
    let p = session.catalog.photo(id)?;
    let (w, h) = (p.width.max(1) as usize, p.height.max(1) as usize);
    Some(Frame::with_lens(w, h, &p.develop, true, p.embedded_lens.as_ref()))
}

/// placement_of is where a window lands: `frame` fitted in `full_w × full_h`, as the pipeline fits
/// it, and the window clamped into that the way the pipeline clamps it.
fn placement_of(frame: &Frame, full_w: usize, full_h: usize, window: PixelWindow) -> Placement {
    let (frame_w, frame_h) = frame.fit(full_w, full_h);
    let clamped = window.clamped(frame_w, frame_h);
    Placement {
        frame_width: frame_w as u32,
        frame_height: frame_h as u32,
        x: clamped.x as u32,
        y: clamped.y as u32,
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
    unsafe { CStr::from_ptr(ptr) }.to_str().map_err(|_| Failure::invalid(format!("{name} is not UTF-8")))
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
/// With `before` the frame is the photo's unedited look (its crop kept), as a render of its
/// import settings (what develop.reset gives), and its histogram is that look's.
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
    before: bool,
    callback: Option<UvFrameCb>,
    ctx: *mut c_void,
) -> u64 {
    guard(|| {
        // SAFETY: a live session per the contract; it is only borrowed for this call.
        let session = unsafe { session.as_ref() }.ok_or_else(|| Failure::invalid("session is NULL"))?;
        session.request(RequestKind::Fit { max_pixels }, draft, before, false, callback, ctx)
    })
}

/// uv_request_window asks for the pixels of one window of the active photo's frame zoomed to
/// `full_w × full_h` (the frame is fitted in that size, like a preview in max_pixels), at draft
/// quality when `draft`, and returns its generation at once. The window is (x, y, w, h) in frame
/// pixels, clamped inside the frame; spatial stages read their neighbours, so ask for a margin.
/// The frame reaches `callback` with `window` true, the frame's size and the window's origin, so
/// the caller can place it over the whole-frame preview. With `before` the window is of the
/// unedited look, like uv_request_preview's. With `histogram` (ABI 6) the window also carries the
/// whole photo's histogram, from a small whole frame rendered with the same settings and quality
/// on the preview's source level and stages (a few ms more per window); a newer window that
/// replaces a pending one which asked for it inherits the wish, so it is never lost to a drag.
///
/// Requests share uv_request_preview's generations and the newest wins, with one refinement for
/// the preview a window is drawn over: a window never replaces a pending preview (the preview
/// renders first), while a preview replaces every older pending window. So frames still arrive in
/// increasing generations. When the engine cannot render the window alone (a spot reads farther
/// than one render holds), the frame delivered is the whole-frame preview fitted in max(w, h),
/// with `window` false and `fallback` true, and the same fallback asked again is not rendered.
/// Errors as uv_request_preview, then a frame or window size of 0 (UV_ERR_INVALID_ARGUMENT).
///
/// # Safety
/// As uv_request_preview.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn uv_request_window(
    session: *mut UVSession,
    full_w: u32,
    full_h: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    draft: bool,
    before: bool,
    histogram: bool,
    callback: Option<UvFrameCb>,
    ctx: *mut c_void,
) -> u64 {
    guard(|| {
        // SAFETY: a live session per the contract; it is only borrowed for this call.
        let session = unsafe { session.as_ref() }.ok_or_else(|| Failure::invalid("session is NULL"))?;
        let window = PixelWindow { x: x as usize, y: y as usize, w: w as usize, h: h as usize };
        let kind = RequestKind::Window { full_w, full_h, window };
        session.request(kind, draft, before, histogram, callback, ctx)
    })
}

/// uv_release_windows frees what zoom windows keep resident, for a view that no longer shows one:
/// its Full and Draft stages at once, then the full source, pool and freed heap pages once idle.
/// A tight-crop preview keeps a Full source it still uses. The call never waits. A window requested
/// before the release completes cancels the remaining release; re-entry after eviction reloads.
///
/// # Safety
/// `session` is NULL or a live pointer from uv_session_new.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uv_release_windows(session: *mut UVSession) {
    guard(|| {
        // SAFETY: a live session per the contract; it is only borrowed for this call.
        let session = unsafe { session.as_ref() }.ok_or_else(|| Failure::invalid("session is NULL"))?;
        session.tx.send(Msg::ReleaseWindows).map_err(|_| stopped())?;
        Ok(())
    })
}

/// uv_suspend stops GPU work for the background: new previews fail with UV_ERR_SUSPENDED, the
/// pending one is dropped, and GPU rendering is switched off, so a uv_execute that renders (an
/// export) runs on the CPU. It returns UV_OK once the render in flight is done (its frame is
/// still delivered, before the return). If that takes over 2 s it returns UV_ERR_TIMEOUT, and
/// the session stays suspended: the late frame is still delivered and uv_resume is still needed.
/// After UV_OK no frame is delivered until uv_resume; uv_execute keeps working. The preview's
/// resident stages are released too, and on UV_OK the GPU buffer pool they return to is emptied,
/// so the background holds no device buffers; the first preview after uv_resume rebuilds them.
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
        // On UV_OK no render runs and the stages' last owner has dropped them into the GPU pool (kept up
        // to budget / 8): empty it. trim_pool may run on any thread outside a render, as this one is.
        lightcraft_gpu::trim_pool(0);
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
        session.prepared.clear();
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
