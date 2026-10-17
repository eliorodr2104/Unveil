//! The bridge brackets interactive preparation without changing the C ABI or retaining thumbnails.

mod common;
use common::*;
use serde_json::json;
use unveil_ffi::*;

fn prepared(s: &TestSession) -> serde_json::Value {
    exec_ok(s, "library.memory", "{}")["unveil"]["preparedSource"].clone()
}

#[test]
fn preparation_diagnostics_exist_and_pressure_does_not_break_selection() {
    let s = TestSession::new();
    import_fixture(&s);
    assert_eq!(prepared(&s)["retainedBytes"], 0);
    // SAFETY: s owns the live session throughout the call.
    unsafe { uv_set_memory_budget(s.raw, 128 << 20) };
    assert_eq!(prepared(&s)["retainedBytes"], 0);
    // SAFETY: s owns the live session throughout the call.
    assert_eq!(unsafe { uv_suspend(s.raw) }, 0);
    assert_eq!(prepared(&s)["retainedBytes"], 0);
    // SAFETY: s owns the live session throughout the call.
    assert_eq!(unsafe { uv_resume(s.raw) }, 0);
    exec_ok(
        &s,
        "develop.set",
        &json!({"control":"light.exposure","value":0.5}).to_string(),
    );
}

#[test]
#[ignore = "requires the E0 NEF fixture; set UNVEIL_E3_NEF"]
fn raw_import_preview_full_and_lifecycle_use_the_prepared_source() {
    use std::sync::Arc;
    let s = TestSession::new();
    let path = std::env::var("UNVEIL_E3_NEF").unwrap();
    let report = exec_ok(&s, "library.import", &json!({"paths":[path]}).to_string());
    let id = photo_id_from_import(&report);
    assert!(prepared(&s)["retainedBytes"].as_u64().unwrap() > 0);
    exec_ok(&s, "library.select", &json!({"ids":[id],"active":id}).to_string());
    let frames = Arc::new(Frames::default());
    let ctx = Arc::as_ptr(&frames).cast_mut().cast();
    // SAFETY: the session and frames outlive both requests and their callbacks.
    unsafe { uv_request_preview(s.raw, 2560, false, false, Some(record), ctx) };
    assert_eq!(wait_for(&frames, 1).len(), 1);
    assert!(prepared(&s)["retainedBytes"].as_u64().unwrap() > 0);
    assert_eq!(prepared(&s)["sampleDecodes"], 1);
    // SAFETY: the session and frames stay alive until the requested window is delivered.
    unsafe {
        uv_request_window(
            s.raw,
            8256,
            5504,
            3000,
            2000,
            128,
            128,
            false,
            false,
            false,
            Some(record),
            ctx,
        )
    };
    assert_eq!(wait_for(&frames, 2).len(), 2);
    assert_eq!(prepared(&s)["sampleDecodes"], 1);
    assert_eq!(prepared(&s)["retainedBytes"], 0);

    // The same path in a warm catalog still begins and confirms a fresh preparation transaction.
    let reopened = exec_ok(&s, "library.import", &json!({"paths":[path]}).to_string());
    assert_eq!(reopened["duplicates"][0]["existing"], id);
    exec_ok(&s, "library.select", &json!({"ids":[id],"active":id}).to_string());
    exec_ok(&s, "photo.reload", &json!({"ids":[id]}).to_string());
    assert_eq!(prepared(&s)["retainedBytes"], 0);
    drop(s);
    drop(frames);
}
