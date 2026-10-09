mod common;
use common::*;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;
use unveil_ffi::*;

/// The memory budget is process-global and every TestSession sets it, so these tests take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn suspended_session_refuses_previews_and_resume_restores_them() {
    let _turn = serial();
    let frames = Arc::new(Frames::default());
    let s = TestSession::new();
    import_fixture(&s);
    // SAFETY: s.raw is live.
    assert_eq!(unsafe { uv_suspend(s.raw) }, 0, "{}", last_error());
    // SAFETY: s.raw is live; ctx is never used: the request fails before queueing.
    let g = unsafe { uv_request_preview(s.raw, 256, false, None, std::ptr::null_mut()) };
    assert_eq!(g, 0);
    assert!(last_error().contains("suspended"), "{}", last_error());
    // SAFETY: s.raw is live.
    assert_eq!(unsafe { uv_resume(s.raw) }, 0);
    // SAFETY: s.raw is live; frames is declared before s, so it outlives the session.
    let g = unsafe { uv_request_preview(s.raw, 256, false, Some(record), Arc::as_ptr(&frames) as *mut _) };
    assert!(g > 0, "{}", last_error());
    assert_eq!(wait_for(&frames, 1)[0].0, g);
}

#[test]
fn suspend_waits_for_the_render_in_flight() {
    let _turn = serial();
    let frames = Arc::new(Frames::default());
    let s = TestSession::new();
    import_png(&s, &gradient_png(s.dir.path(), "big.png", 3000, 2000));
    // SAFETY: s.raw is live; frames is declared before s, so it outlives the session.
    let g = unsafe { uv_request_preview(s.raw, 2048, false, Some(record), Arc::as_ptr(&frames) as *mut _) };
    assert!(g > 0, "{}", last_error());
    // The engine thread handles messages in order: once this returns, the render has started.
    exec_ok(&s, "library.info", "{}");
    // SAFETY: s.raw is live.
    assert_eq!(unsafe { uv_suspend(s.raw) }, 0, "{}", last_error());
    // The frame in flight is delivered before suspend returns, and nothing follows it.
    assert_eq!(frames.list.lock().unwrap().len(), 1);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(frames.list.lock().unwrap().len(), 1);
}

#[test]
fn commands_still_work_while_suspended() {
    let _turn = serial();
    let s = TestSession::new();
    import_fixture(&s);
    // SAFETY: s.raw is live.
    assert_eq!(unsafe { uv_suspend(s.raw) }, 0, "{}", last_error());
    exec_ok(&s, "develop.set", r#"{"control":"light.exposure","value":0.3}"#);
}

#[test]
fn memory_budget_is_applied() {
    let _turn = serial();
    let s = TestSession::new();
    // SAFETY: s.raw is live.
    unsafe { uv_set_memory_budget(s.raw, 300 << 20) };
    assert_eq!(lightcraft_engine::memory::budget(), 300 << 20);
}

/// Pixels is the slot `keep_pixels` fills: the generation and RGBA bytes of the last frame.
#[derive(Default)]
struct Pixels {
    frame: Mutex<Option<(u64, Vec<u8>)>>,
    cv: Condvar,
}

extern "C" fn keep_pixels(
    ctx: *mut std::ffi::c_void,
    rgba: *const u8,
    _w: u32,
    h: u32,
    stride: u32,
    g: u64,
    _draft: bool,
) {
    // SAFETY: ctx is the Pixels the test keeps alive in an Arc for the whole session.
    let pixels = unsafe { &*(ctx as *const Pixels) };
    // SAFETY: the engine promises stride * h readable bytes for the duration of the callback.
    let bytes = unsafe { std::slice::from_raw_parts(rgba, (stride * h) as usize) };
    *pixels.frame.lock().unwrap() = Some((g, bytes.to_vec()));
    pixels.cv.notify_all();
}

/// preview_with_exposure sets the exposure and returns the full-quality 2048 px preview's RGBA bytes.
fn preview_with_exposure(s: &TestSession, pixels: &Arc<Pixels>, exposure: f64) -> Vec<u8> {
    preview(s, pixels, exposure, false)
}

/// preview sets the exposure and returns the 2048 px preview's RGBA bytes, draft or full quality.
fn preview(s: &TestSession, pixels: &Arc<Pixels>, exposure: f64, draft: bool) -> Vec<u8> {
    exec_ok(
        s,
        "develop.set",
        &format!(r#"{{"control":"light.exposure","value":{exposure}}}"#),
    );
    let ctx = Arc::as_ptr(pixels) as *mut _;
    // SAFETY: s.raw is live; pixels is declared before s, so it outlives the session.
    let g = unsafe { uv_request_preview(s.raw, 2048, draft, Some(keep_pixels), ctx) };
    assert!(g > 0, "{}", last_error());
    let guard = pixels.frame.lock().unwrap();
    let (guard, _) = pixels
        .cv
        .wait_timeout_while(guard, Duration::from_secs(30), |f| {
            f.as_ref().is_none_or(|f| f.0 < g)
        })
        .unwrap();
    guard.as_ref().expect("no frame within 30 s").1.clone()
}

/// plain_render renders png with `exposure` straight through the engine, with no stage cache.
fn plain_render(png: &Path, exposure: f64) -> Vec<u8> {
    let dir = tempdir_lite::Dir::new();
    let mut session = lightcraft_engine::Session::new().with_fs();
    session.open_library(dir.path(), false).unwrap();
    session.xmp.auto_write = false;
    let mut exec = |command: &str, params: serde_json::Value| session.execute(command, &params).unwrap();
    let id = photo_id_from_import(&exec(
        "library.import",
        serde_json::json!({"paths": [png], "mode": "add"}),
    ));
    exec("library.select", serde_json::json!({"ids": [id], "active": id}));
    exec(
        "develop.set",
        serde_json::json!({"control": "light.exposure", "value": exposure}),
    );
    let photo = lightcraft_engine::catalog::PhotoId(id);
    let rendered = session.render_now(photo, 2048, 2048).unwrap();
    rendered.image.data.as_flattened().to_vec()
}

#[test]
fn previews_after_suspend_and_resume_match_renders_without_stages() {
    let _turn = serial();
    let pixels = Arc::new(Pixels::default());
    let s = TestSession::new();
    // Another test may have left the process-wide GPU switch off; resume turns it back on.
    // SAFETY: s.raw is live.
    assert_eq!(unsafe { uv_resume(s.raw) }, 0);
    let png = gradient_png(s.dir.path(), "stages.png", 640, 427);
    import_png(&s, &png);
    preview_with_exposure(&s, &pixels, -0.7);

    // SAFETY: s.raw is live.
    assert_eq!(unsafe { uv_suspend(s.raw) }, 0, "{}", last_error());
    // SAFETY: s.raw is live.
    assert_eq!(unsafe { uv_resume(s.raw) }, 0);

    // The first preview rebuilds the stages, the second reuses them.
    let rebuilt = preview_with_exposure(&s, &pixels, 1.3);
    let reused = preview_with_exposure(&s, &pixels, 0.4);
    assert!(
        rebuilt == plain_render(&png, 1.3),
        "the first preview after resume differs"
    );
    assert!(
        reused == plain_render(&png, 0.4),
        "the preview reusing stages differs"
    );
    assert!(rebuilt != reused, "the exposure change had no effect");
}

#[test]
fn suspend_releases_every_gpu_buffer() {
    let _turn = serial();
    let pixels = Arc::new(Pixels::default());
    let s = TestSession::new();
    // SAFETY: s.raw is live.
    assert_eq!(unsafe { uv_resume(s.raw) }, 0);
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    import_png(&s, &gradient_png(s.dir.path(), "pool.png", 640, 427));
    preview_with_exposure(&s, &pixels, 0.5);
    let before = lightcraft_gpu::memory();
    assert!(before.allocated > 0, "the preview left no GPU buffer: {before:?}");

    // SAFETY: s.raw is live.
    assert_eq!(unsafe { uv_suspend(s.raw) }, 0, "{}", last_error());
    let after = lightcraft_gpu::memory();
    assert_eq!(after.allocated, 0, "GPU buffers survived uv_suspend: {after:?}");
}

#[test]
fn repeated_draft_previews_keep_gpu_memory_flat() {
    let _turn = serial();
    let pixels = Arc::new(Pixels::default());
    let s = TestSession::new();
    // SAFETY: s.raw is live.
    assert_eq!(unsafe { uv_resume(s.raw) }, 0);
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    import_png(&s, &gradient_png(s.dir.path(), "flat.png", 640, 427));
    // Each frame uploads a 32 KiB LUT block; before the fix every one of them stayed in the
    // pool, so pooled bytes grew by one block per frame up to the pool limit.
    for i in 0..20 {
        preview(&s, &pixels, i as f64 * 0.01, true);
    }
    let plateau = lightcraft_gpu::memory();
    for i in 20..2000 {
        preview(&s, &pixels, i as f64 * 0.001, true);
    }
    let end = lightcraft_gpu::memory();
    eprintln!("plateau {plateau:?}, after 2000 drafts {end:?}");
    assert!(
        end.allocated <= plateau.allocated + (1 << 20),
        "GPU memory grew with the frame count: {plateau:?} -> {end:?}"
    );
}
