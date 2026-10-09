mod common;
use common::*;
use std::sync::{Arc, Mutex, MutexGuard};
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
