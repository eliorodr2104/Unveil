//! Real-file, opt-in observations of the prepared open and first full development.
mod common;
use common::*;
use serde_json::json;
use std::sync::Arc;
use unveil_ffi::*;

#[test]
#[ignore = "requires UNVEIL_MEMORY_RAW and an actual GPU adapter"]
fn prepared_open_and_first_window_have_a_bounded_allocation_trace() {
    assert!(lightcraft_gpu::available(), "a GPU adapter is required");
    let frames = Arc::new(Frames::default());
    let s = TestSession::new();
    // SAFETY: the session is live for this call and every following request.
    unsafe { uv_set_memory_budget(s.raw, 2_860_979_293) };
    exec_ok(&s, "unveil.diagnostics", r#"{"enabled":true}"#);
    exec_ok(&s, "unveil.decode-memory", r#"{"enabled":true}"#);
    let path = std::env::var("UNVEIL_MEMORY_RAW").unwrap();
    let report = exec_ok(&s, "library.import", &json!({"paths":[path]}).to_string());
    let id = photo_id_from_import(&report);
    exec_ok(&s, "library.select", &json!({"ids":[id],"active":id}).to_string());
    let ctx = Arc::as_ptr(&frames).cast_mut().cast();
    // SAFETY: frames outlives the session and all callbacks; requests borrow the live session.
    unsafe { uv_request_preview(s.raw, 2360, false, false, Some(record), ctx) };
    assert_eq!(wait_for(&frames, 1).len(), 1);
    let (w, h) = if path.ends_with(".RAF") { (6240, 4160) } else { (8256, 5504) };
    // SAFETY: the same session and callback owner remain live through the full window.
    unsafe { uv_request_window(s.raw, w, h, 1000, 1000, 1400, 1000, false, false, true, Some(record), ctx) };
    assert_eq!(wait_for(&frames, 2).len(), 2);
    let trace = exec_ok(&s, "unveil.decode-memory", "{}");
    assert_eq!(trace["incomplete"], false);
    assert_eq!(trace["droppedEvents"], 0);
    assert!(trace["events"].as_array().unwrap().len() > 5);
    println!("DM-ALLOC {trace}");
    println!("DM-LOADS {}", exec_ok(&s, "unveil.loads", "{}"));
    println!("DM-MEMORY {}", exec_ok(&s, "library.memory", "{}"));
    exec_ok(&s, "unveil.decode-memory", r#"{"enabled":false}"#);
}
