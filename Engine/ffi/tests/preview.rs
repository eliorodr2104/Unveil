mod common;
use common::*;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use unveil_ffi::*;

type Frame = (u64, bool, u32, u32, u32, usize);

#[derive(Default)]
struct Frames {
    list: Mutex<Vec<Frame>>,
    cv: Condvar,
}

extern "C" fn record(
    ctx: *mut std::ffi::c_void,
    rgba: *const u8,
    w: u32,
    h: u32,
    stride: u32,
    g: u64,
    draft: bool,
) {
    // A panic here aborts the test binary, which is the failure we want for a wrong thread.
    assert_eq!(std::thread::current().name(), Some("unveil-engine"));
    // SAFETY: ctx is the Frames the test keeps alive in an Arc for the whole session.
    let frames = unsafe { &*(ctx as *const Frames) };
    assert!(!rgba.is_null());
    // SAFETY: the engine promises stride * h readable bytes for the duration of the callback.
    let bytes = unsafe { std::slice::from_raw_parts(rgba, (stride * h) as usize) };
    let nonzero = bytes.iter().filter(|b| **b != 0).count();
    frames
        .list
        .lock()
        .unwrap()
        .push((g, draft, w, h, stride, nonzero));
    frames.cv.notify_all();
}

fn wait_for(frames: &Frames, n: usize) -> Vec<Frame> {
    let list = frames.list.lock().unwrap();
    let (list, _) = frames
        .cv
        .wait_timeout_while(list, Duration::from_secs(30), |l| l.len() < n)
        .unwrap();
    list.clone()
}

fn request(s: &TestSession, max: u32, draft: bool, frames: &Arc<Frames>) -> u64 {
    // SAFETY: s.raw is live; frames outlives the session in every test.
    unsafe { uv_request_preview(s.raw, max, draft, Some(record), Arc::as_ptr(frames) as *mut _) }
}

#[test]
fn a_preview_arrives_with_sane_geometry() {
    let s = TestSession::new();
    import_fixture(&s);
    let frames = Arc::new(Frames::default());
    let g = request(&s, 512, false, &frames);
    assert!(g >= 1, "{}", last_error());
    let got = wait_for(&frames, 1);
    let (generation, draft, w, h, stride, nonzero) = got[0];
    assert_eq!((generation, draft), (g, false));
    assert!(w > 0 && h > 0 && w <= 512 && h <= 512);
    assert_eq!(stride, w * 4);
    assert!(nonzero > 0, "image is all zeros");
}

#[test]
fn a_burst_ends_with_the_newest_generation() {
    let s = TestSession::new();
    import_fixture(&s);
    let frames = Arc::new(Frames::default());
    let mut last = 0;
    for i in 0..10 {
        exec_ok(
            &s,
            "develop.set",
            &format!(r#"{{"control":"light.exposure","value":{}}}"#, i as f64 / 10.0),
        );
        last = request(&s, 256, i < 9, &frames);
    }
    std::thread::sleep(Duration::from_secs(3));
    let got = frames.list.lock().unwrap().clone();
    assert!(!got.is_empty());
    assert!(
        got.windows(2).all(|w| w[0].0 < w[1].0),
        "generations must increase: {got:?}"
    );
    let final_frame = got.last().unwrap();
    assert_eq!(
        (final_frame.0, final_frame.1),
        (last, false),
        "last frame must be the final full render"
    );
}

#[test]
fn no_callback_after_free_returns() {
    let frames = Arc::new(Frames::default());
    {
        let s = TestSession::new();
        import_png(&s, &gradient_png(s.dir.path(), "big.png", 3000, 2000));
        request(&s, 2048, false, &frames);
    } // uv_session_free runs here, with the render likely still in flight
    let n = frames.list.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(frames.list.lock().unwrap().len(), n);
}

#[test]
fn preview_without_an_active_photo_is_an_error() {
    let s = TestSession::new();
    // SAFETY: s.raw is live; ctx is never used: the request fails before queueing.
    let g = unsafe { uv_request_preview(s.raw, 256, false, Some(record), std::ptr::null_mut()) };
    assert_eq!(g, 0);
    assert!(last_error().contains("no active photo"), "{}", last_error());
    // The missing photo wins over the missing callback.
    // SAFETY: s.raw is live; ctx is never used: the request fails before queueing.
    let g = unsafe { uv_request_preview(s.raw, 256, false, None, std::ptr::null_mut()) };
    assert_eq!(g, 0);
    assert!(last_error().contains("no active photo"), "{}", last_error());
}

#[test]
fn a_null_callback_is_an_invalid_argument() {
    let s = TestSession::new();
    import_fixture(&s);
    // SAFETY: s.raw is live; ctx is never used: the request fails before queueing.
    let g = unsafe { uv_request_preview(s.raw, 256, false, None, std::ptr::null_mut()) };
    assert_eq!(g, 0);
    assert!(last_error().contains("callback"), "{}", last_error());
}

#[test]
fn a_result_for_a_photo_no_longer_active_is_dropped() {
    let s = TestSession::new();
    let wide = import_png(&s, &gradient_png(s.dir.path(), "wide.png", 3000, 2000));
    let tall = import_png(&s, &gradient_png(s.dir.path(), "tall.png", 64, 96));
    exec_ok(
        &s,
        "library.select",
        &format!(r#"{{"ids":[{wide}],"active":{wide}}}"#),
    );
    let frames = Arc::new(Frames::default());
    // The wide render takes far longer than the select below, so it finishes for an inactive photo.
    request(&s, 1024, false, &frames);
    exec_ok(
        &s,
        "library.select",
        &format!(r#"{{"ids":[{tall}],"active":{tall}}}"#),
    );
    let g = request(&s, 1024, false, &frames);
    let got = wait_for(&frames, 1);
    std::thread::sleep(Duration::from_millis(500));
    let got_later = frames.list.lock().unwrap().clone();
    assert_eq!(got, got_later, "one frame only: {got_later:?}");
    let (generation, _, w, h, _, _) = got[0];
    assert_eq!(generation, g);
    assert!(h > w, "the frame must be the tall photo: {w}x{h}");
}
